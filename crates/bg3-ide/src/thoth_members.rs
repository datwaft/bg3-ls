use std::collections::BTreeMap;
use std::path::Path;

use bg3_index::{
    ParsedFile, Position, PrimitiveType, TextRange, ThothExpressionFact, ThothExpressionKind,
    ThothMemberAccessKind, ThothMemberSegment, ThothScopeId, ThothStatementId, TypeExpression,
};

use crate::{
    CompletionItem, CompletionKind, OverlaySet, SourceLocation, WorkspaceSnapshot,
    thoth_flow::Guards,
};

#[derive(Clone, Debug)]
struct FieldEvidence {
    name: String,
    ty: TypeExpression,
    definitions: Vec<SourceLocation>,
    class_names: Vec<String>,
    provenance: Vec<String>,
}

#[derive(Clone, Debug)]
struct MemberContext<'a> {
    path: &'a Path,
    file: &'a ParsedFile,
    fact: &'a ThothExpressionFact,
    segments: Vec<ThothMemberSegment>,
    target: usize,
}

struct VariableEvidence {
    ty: Option<TypeExpression>,
    scope: ThothScopeId,
    local: bool,
}

impl WorkspaceSnapshot {
    /// Returns explicit fields available at a member-expression position.
    pub(crate) fn thoth_member_completions(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<Vec<CompletionItem>> {
        let context = self.member_context(path, position, overlays, false)?;
        let fields = self
            .member_owner_type(&context, overlays)
            .and_then(|ty| self.fields_for_type(&ty, overlays))
            .unwrap_or_default();
        let prefix = member_name(&context.segments[context.target]).unwrap_or_default();
        Some(
            fields
                .into_values()
                .filter(|field| field.name.starts_with(&prefix))
                .map(|field| {
                    let kind = if matches!(field.ty, TypeExpression::Function { .. }) {
                        CompletionKind::Function
                    } else {
                        CompletionKind::Field
                    };
                    CompletionItem {
                        label: field.name.clone(),
                        detail: Some(field.ty.to_string()),
                        documentation: Some(field.provenance.join("\n\n")),
                        new_text: field.name,
                        sort_text: None,
                        range: context.segments[context.target].range,
                        kind,
                        snippet: false,
                    }
                })
                .collect(),
        )
    }

    /// Returns explicit hover data for a field or method segment.
    pub(crate) fn thoth_member_hover(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let context = self.member_context(path, position, overlays, true)?;
        let owner_ty = self.member_owner_type(&context, overlays)?;
        let name = member_name(&context.segments[context.target])?;
        let field = self.fields_for_type(&owner_ty, overlays)?.remove(&name)?;
        let classes = field.class_names.join("`, `");
        Some(format!(
            "**Thoth field** `{name}`\n\nClass: `{classes}`\n\nType: `{}`\n\n{}",
            field.ty,
            field.provenance.join("\n\n")
        ))
    }

    /// Returns loose field declaration locations for a member segment.
    pub(crate) fn thoth_member_definition_locations(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<Vec<SourceLocation>> {
        let context = self.member_context(path, position, overlays, true)?;
        let owner_ty = self.member_owner_type(&context, overlays);
        let name = member_name(&context.segments[context.target]);
        Some(
            owner_ty
                .and_then(|ty| self.fields_for_type(&ty, overlays))
                .and_then(|mut fields| name.and_then(|name| fields.remove(&name)))
                .map(|field| field.definitions)
                .unwrap_or_default(),
        )
    }

    fn member_context<'a>(
        &'a self,
        path: &'a Path,
        position: Position,
        overlays: &'a OverlaySet,
        hover: bool,
    ) -> Option<MemberContext<'a>> {
        let (module, file) = self.current_file(path, overlays)?;
        if !self.layers.iter().any(|layer| layer.spec.name == module) {
            return None;
        }
        let thoth = file.thoth.as_ref()?;
        let mut facts = thoth
            .expression_facts
            .iter()
            .filter_map(|fact| {
                let ThothExpressionKind::MemberAccess(segments) = &fact.kind else {
                    return None;
                };
                if segments.len() < 2 || !contains(fact.range, position) {
                    return None;
                }
                let target = if hover {
                    segments
                        .iter()
                        .enumerate()
                        .skip(1)
                        .find(|(_, segment)| contains(segment.range, position))
                        .map(|(index, _)| index)?
                } else {
                    segments
                        .iter()
                        .enumerate()
                        .skip(1)
                        .find(|(_, segment)| contains(segment.range, position))
                        .map(|(index, _)| index)
                        .or_else(|| (position == fact.range.end).then_some(segments.len() - 1))?
                };
                Some((fact, segments.clone(), target))
            })
            .collect::<Vec<_>>();
        facts.sort_by_key(|(fact, _, _)| range_size(fact.range));
        let (fact, segments, target) = facts.into_iter().next()?;
        Some(MemberContext {
            path,
            file,
            fact,
            segments,
            target,
        })
    }

    fn member_owner_type(
        &self,
        context: &MemberContext<'_>,
        overlays: &OverlaySet,
    ) -> Option<TypeExpression> {
        let root = context.segments.first()?;
        let mut ty = self.root_type(context, root, overlays)?;
        for segment in context.segments.iter().take(context.target).skip(1) {
            let name = member_name(segment)?;
            ty = self.fields_for_type(&ty, overlays)?.remove(&name)?.ty;
        }
        Some(ty)
    }

    fn root_type(
        &self,
        context: &MemberContext<'_>,
        root: &ThothMemberSegment,
        overlays: &OverlaySet,
    ) -> Option<TypeExpression> {
        let fact = context
            .file
            .thoth
            .as_ref()?
            .expression_facts
            .iter()
            .find(|fact| fact.range == root.range)
            .or_else(|| (context.fact.range == root.range).then_some(context.fact));
        let inferred = fact.map_or_else(
            || {
                let root_fact = ThothExpressionFact {
                    range: root.range,
                    text: root.text.clone(),
                    kind: ThothExpressionKind::Identifier,
                    statement: context.fact.statement,
                };
                self.thoth_type_at_fact(
                    context.path,
                    context.file,
                    &root_fact,
                    overlays,
                    &mut Guards::default(),
                )
            },
            |fact| {
                self.thoth_type_at_fact(
                    context.path,
                    context.file,
                    fact,
                    overlays,
                    &mut Guards::default(),
                )
            },
        );
        if !inferred.is_unknown() {
            return Some(inferred);
        }
        if let Some(fact) =
            fact.filter(|fact| matches!(fact.kind, ThothExpressionKind::FunctionCall))
        {
            let name = context
                .file
                .thoth
                .as_ref()?
                .calls
                .iter()
                .find(|call| call.range == fact.range)?
                .name
                .clone();
            return self.function_return_type(&name, overlays);
        }
        let name = root.text.as_str();
        let variable = self.variable_evidence(context, name, overlays);
        let parameter = self.parameter_type(context, name, overlays);
        if let Some(parameter) = parameter {
            let thoth = context.file.thoth.as_ref()?;
            let function = enclosing_function_scope(thoth, context.fact.statement.scope)?;
            if let Some(variable) = variable
                && variable.local
                && enclosing_function_scope(thoth, variable.scope) == Some(function)
            {
                return variable.ty;
            }
            return Some(parameter);
        }
        variable.and_then(|variable| variable.ty)
    }

    fn variable_evidence(
        &self,
        context: &MemberContext<'_>,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<VariableEvidence> {
        let thoth = context.file.thoth.as_ref()?;
        let use_statement = context.fact.statement;
        let mut candidates = thoth
            .assignments
            .iter()
            .filter(|assignment| {
                assignment.targets.len() == 1 && assignment.targets[0].text == name
            })
            .filter_map(|assignment| {
                let target = &assignment.targets[0];
                let fact = thoth.expression_facts.iter().find(|fact| {
                    fact.range == target.range
                        && matches!(fact.kind, ThothExpressionKind::Identifier)
                })?;
                if !scope_visible(thoth, fact.statement, use_statement)
                    || (fact.statement.scope == use_statement.scope
                        && fact.statement.order > use_statement.order)
                    || fact.statement == use_statement
                {
                    return None;
                }
                let depth = scope_depth(thoth, use_statement.scope, fact.statement.scope)?;
                Some((
                    depth,
                    fact.statement.order,
                    fact.statement.scope,
                    target.range,
                    assignment.local,
                ))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|candidate| (candidate.0, std::cmp::Reverse(candidate.1)));
        let (depth, order, scope, target_range, local) = *candidates.first()?;
        if candidates
            .iter()
            .filter(|candidate| candidate.0 == depth && candidate.1 == order)
            .count()
            != 1
        {
            return None;
        }
        let mut annotations = thoth
            .annotations
            .variables
            .iter()
            .filter(|annotation| annotation.target_range == target_range);
        let ty = annotations
            .next()
            .filter(|_| annotations.next().is_none())
            .map(|annotation| self.resolve_thoth_type(&annotation.ty, overlays));
        Some(VariableEvidence { ty, scope, local })
    }

    fn parameter_type(
        &self,
        context: &MemberContext<'_>,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<TypeExpression> {
        let thoth = context.file.thoth.as_ref()?;
        let scope = enclosing_function_scope(thoth, context.fact.statement.scope)?;
        let declaration = thoth
            .declarations
            .iter()
            .find(|declaration| declaration.range == scope)?;
        let function = self.resolve_thoth_function(&declaration.name, overlays)?;
        let crate::ThothTypeSource::Loose { path, .. } = &function.source else {
            return None;
        };
        if path != &context.file.source.path || function.name_range != declaration.name_range {
            return None;
        }
        let types = function
            .contracts
            .iter()
            .map(|contract| {
                contract
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == name)
                    .map(|parameter| parameter.ty.clone())
            })
            .collect::<Option<Vec<_>>>()?;
        (!types.is_empty()).then(|| TypeExpression::union(types))
    }

    fn function_return_type(&self, name: &str, overlays: &OverlaySet) -> Option<TypeExpression> {
        let function = self.resolve_thoth_function(name, overlays)?;
        let returns = function
            .contracts
            .iter()
            .map(|contract| contract.returns.first().map(|value| value.ty.clone()))
            .collect::<Option<Vec<_>>>()?;
        (!returns.is_empty()).then(|| TypeExpression::union(returns))
    }

    fn fields_for_type(
        &self,
        ty: &TypeExpression,
        overlays: &OverlaySet,
    ) -> Option<BTreeMap<String, FieldEvidence>> {
        let members = match ty {
            TypeExpression::Union(members) => members,
            other => std::slice::from_ref(other),
        };
        let mut classes = Vec::new();
        for member in members {
            if matches!(member, TypeExpression::Nil) {
                continue;
            }
            if matches!(member, TypeExpression::Unknown) {
                return None;
            }
            let mut fields = BTreeMap::new();
            match member {
                TypeExpression::Name(name) => {
                    let Some(class) = self.resolve_thoth_class(name, overlays) else {
                        if name == "ConditionResult" {
                            fields.insert(
                                "Result".into(),
                                FieldEvidence {
                                    name: "Result".into(),
                                    ty: TypeExpression::Primitive(PrimitiveType::Boolean),
                                    definitions: Vec::new(),
                                    class_names: vec!["ConditionResult".into()],
                                    provenance: vec![
                                        "Curated BG3 Thoth `ConditionResult` contract.".into(),
                                    ],
                                },
                            );
                            classes.push(fields);
                            continue;
                        }
                        return None;
                    };
                    let (definitions, provenance) = match &class.source {
                        crate::ThothTypeSource::Loose { module, path, .. } => (
                            Some((module, path)),
                            format!("Explicit Thoth field from module `{module}`."),
                        ),
                        crate::ThothTypeSource::Packaged { module, entry, .. } => (
                            None,
                            format!(
                                "Explicit installed Thoth field from module `{module}` and virtual package entry `{entry}`."
                            ),
                        ),
                    };
                    for field in class.fields {
                        if fields.contains_key(&field.name) {
                            return None;
                        }
                        let definitions = definitions.map_or_else(Vec::new, |(_, path)| {
                            vec![SourceLocation {
                                path: path.clone(),
                                range: field.name_range,
                            }]
                        });
                        fields.insert(
                            field.name.clone(),
                            FieldEvidence {
                                name: field.name,
                                ty: field.ty,
                                definitions,
                                class_names: vec![class.name.clone()],
                                provenance: vec![provenance.clone()],
                            },
                        );
                    }
                }
                TypeExpression::Primitive(_)
                | TypeExpression::Array(_)
                | TypeExpression::Function { .. } => {}
                TypeExpression::Nil | TypeExpression::Unknown | TypeExpression::Union(_) => {
                    continue;
                }
            }
            classes.push(fields);
        }
        let mut fields = classes.into_iter();
        let mut common = fields.next()?;
        for candidate in fields {
            common.retain(|name, field| {
                let Some(other) = candidate.get(name) else {
                    return false;
                };
                field.ty = TypeExpression::union([field.ty.clone(), other.ty.clone()]);
                field.definitions.extend(other.definitions.clone());
                field.class_names.extend(other.class_names.clone());
                field.provenance.extend(other.provenance.clone());
                true
            });
        }
        for field in common.values_mut() {
            field.definitions.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then(left.range.start.line.cmp(&right.range.start.line))
                    .then(left.range.start.character.cmp(&right.range.start.character))
            });
            field.definitions.dedup();
            field.class_names.sort();
            field.class_names.dedup();
            field.provenance.sort();
            field.provenance.dedup();
        }
        Some(common)
    }

    fn current_file<'a>(
        &'a self,
        path: &Path,
        overlays: &'a OverlaySet,
    ) -> Option<(&'a str, &'a ParsedFile)> {
        if let Some(overlay) = overlays.get(path) {
            return Some((&overlay.module, &overlay.parsed));
        }
        self.layers.iter().find_map(|layer| {
            layer
                .file(path)
                .map(|file| (layer.spec.name.as_str(), file.as_ref()))
        })
    }
}

fn member_name(segment: &ThothMemberSegment) -> Option<String> {
    match segment.access {
        ThothMemberAccessKind::Dot | ThothMemberAccessKind::Method => {
            (!segment.text.is_empty()).then_some(segment.text.clone())
        }
        ThothMemberAccessKind::Bracket => {
            let quote = segment.text.chars().next()?;
            if quote != '\'' && quote != '"' {
                return None;
            }
            Some(
                segment
                    .text
                    .strip_prefix(quote)?
                    .strip_suffix(quote)?
                    .to_owned(),
            )
        }
        ThothMemberAccessKind::Root => None,
    }
}

fn scope_visible(
    file: &bg3_index::ThothFile,
    declaration: ThothStatementId,
    usage: ThothStatementId,
) -> bool {
    scope_depth(file, usage.scope, declaration.scope).is_some()
}

fn enclosing_function_scope(
    file: &bg3_index::ThothFile,
    mut scope: ThothScopeId,
) -> Option<TextRange> {
    loop {
        if let ThothScopeId::Function { range } = scope {
            return Some(range);
        }
        scope = file
            .scopes
            .iter()
            .find(|candidate| candidate.id == scope)?
            .parent?;
    }
}

fn scope_depth(
    file: &bg3_index::ThothFile,
    mut from: ThothScopeId,
    target: ThothScopeId,
) -> Option<usize> {
    let mut depth = 0;
    loop {
        if from == target {
            return Some(depth);
        }
        let parent = file.scopes.iter().find(|scope| scope.id == from)?.parent?;
        from = parent;
        depth += 1;
    }
}

fn contains(range: TextRange, position: Position) -> bool {
    (position.line, position.character) >= (range.start.line, range.start.character)
        && (position.line, position.character) <= (range.end.line, range.end.character)
}

fn range_size(range: TextRange) -> (u32, u32, u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
        range.start.line,
        range.start.character,
    )
}
