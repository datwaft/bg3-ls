//! Conservative type propagation for Thoth expressions.
//!
//! The index stores syntax facts only.  This module derives types for one
//! immutable workspace snapshot and overlay set at query time because the
//! result depends on module precedence and open-document replacement.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bg3_index::{
    ParsedFile, Position, SymbolTarget, TextRange, ThothBinaryOperator, ThothExpressionFact,
    ThothExpressionKind, ThothFile, ThothIfBranchKind, ThothMemberAccessKind, ThothMemberSegment,
    ThothScopeId, ThothStatementId, ThothUnaryOperator, TypeExpression,
};

use crate::{HoverMarkup, OverlaySet, ResolvedThothFunction, ThothTypeSource, WorkspaceSnapshot};

#[derive(Default)]
pub(crate) struct Guards {
    bindings: HashSet<(PathBuf, TextRange)>,
    functions: HashSet<(PathBuf, String)>,
}

impl WorkspaceSnapshot {
    /// Resolves the conservative type of one exact Thoth expression.
    ///
    /// `None` means that the expression is unknown, ambiguous, or not an
    /// expression for which the server has sufficient evidence.  This API
    /// never reports a guessed type.
    pub fn thoth_expression_type(
        &self,
        path: &Path,
        range: TextRange,
        overlays: &OverlaySet,
    ) -> Option<TypeExpression> {
        let (_, file) = self.file(path, overlays)?;
        let thoth = file.thoth.as_ref()?;
        let fact = exactly_one(
            thoth
                .expression_facts
                .iter()
                .filter(|fact| fact.range == range),
        )?;
        let mut guards = Guards::default();
        known(self.thoth_type_at_fact(path, file, fact, overlays, &mut guards))
    }

    pub(crate) fn thoth_flow_hover(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let (_, file) = self.file(path, overlays)?;
        let thoth = file.thoth.as_ref()?;
        let mut facts = thoth
            .expression_facts
            .iter()
            .filter(|fact| contains(fact.range, position))
            .filter(|fact| !matches!(fact.kind, ThothExpressionKind::Literal(_)))
            .collect::<Vec<_>>();
        facts.sort_by_key(|fact| range_size(fact.range));
        let fact = facts.into_iter().next()?;
        let mut guards = Guards::default();
        let ty = known(self.thoth_type_at_fact(path, file, fact, overlays, &mut guards))?;
        Some(
            HoverMarkup::new("Thoth inferred type", &fact.text)
                .fact("Type", &ty.to_string())
                .finish(),
        )
    }

    /// Resolves one indexed fact while retaining its statement identity.
    ///
    /// Member features use this internal entry point when an exact range can
    /// occur in more than one lexical context.  The public range API above is
    /// deliberately silent when that context is ambiguous.
    pub(crate) fn thoth_type_at_fact(
        &self,
        path: &Path,
        file: &ParsedFile,
        fact: &ThothExpressionFact,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let Some(thoth) = file.thoth.as_ref() else {
            return TypeExpression::Unknown;
        };
        self.thoth_type_at_fact_inner(path, file, thoth, fact, overlays, guards)
    }

    fn thoth_type_at_fact_inner(
        &self,
        path: &Path,
        file: &ParsedFile,
        thoth: &ThothFile,
        fact: &ThothExpressionFact,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        match &fact.kind {
            ThothExpressionKind::Literal(literal) => match literal {
                bg3_index::ThothLiteralKind::Nil => TypeExpression::Nil,
                bg3_index::ThothLiteralKind::Boolean => primitive("boolean"),
                bg3_index::ThothLiteralKind::Number => primitive("number"),
                bg3_index::ThothLiteralKind::String => primitive("string"),
            },
            ThothExpressionKind::Parenthesized { expression } => {
                self.fact_in_range(path, file, *expression, overlays, guards)
            }
            ThothExpressionKind::Unary { operator, operand } => {
                let operand = self.fact_in_range(path, file, *operand, overlays, guards);
                unary_type(*operator, operand)
            }
            ThothExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.fact_in_range(path, file, *left, overlays, guards);
                let right = self.fact_in_range(path, file, *right, overlays, guards);
                binary_type(*operator, left, right)
            }
            ThothExpressionKind::Identifier => {
                self.identifier_type(path, file, thoth, fact, overlays, guards)
            }
            ThothExpressionKind::FunctionCall => self.call_type(path, file, fact, overlays, guards),
            ThothExpressionKind::MemberAccess(segments) => {
                self.member_expression_type(path, file, fact, segments, overlays, guards)
            }
            ThothExpressionKind::Unknown => TypeExpression::Unknown,
        }
    }

    fn member_expression_type(
        &self,
        path: &Path,
        file: &ParsedFile,
        fact: &ThothExpressionFact,
        segments: &[ThothMemberSegment],
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let Some(root) = segments.first() else {
            return TypeExpression::Unknown;
        };
        let Some(thoth) = file.thoth.as_ref() else {
            return TypeExpression::Unknown;
        };
        if segments.len() == 2
            && !has_identifier_binding(thoth, &root.text, fact.statement)
            && let Some(member) = member_name(&segments[1])
            && self
                .schema
                .enumerations
                .get(&root.text)
                .is_some_and(|values| values.iter().any(|value| value == &member))
        {
            return TypeExpression::Name(root.text.clone());
        }

        let root_fact = file
            .thoth
            .as_ref()
            .and_then(|thoth| {
                exactly_one(
                    thoth
                        .expression_facts
                        .iter()
                        .filter(|candidate| candidate.range == root.range),
                )
            })
            .cloned()
            .unwrap_or_else(|| ThothExpressionFact {
                range: root.range,
                text: root.text.clone(),
                kind: ThothExpressionKind::Identifier,
                statement: fact.statement,
            });
        let mut ty = self.thoth_type_at_fact(path, file, &root_fact, overlays, guards);
        for segment in segments.iter().skip(1) {
            let Some(name) = member_name(segment) else {
                return TypeExpression::Unknown;
            };
            ty = self.flow_member_type(&ty, &name, overlays);
            if ty.is_unknown() {
                return ty;
            }
        }
        ty
    }

    fn flow_member_type(
        &self,
        ty: &TypeExpression,
        name: &str,
        overlays: &OverlaySet,
    ) -> TypeExpression {
        let members = match ty {
            TypeExpression::Union(members) => members.as_slice(),
            other => std::slice::from_ref(other),
        };
        let mut field_types = Vec::new();
        for member in members {
            match member {
                TypeExpression::Nil => continue,
                TypeExpression::Unknown => return TypeExpression::Unknown,
                TypeExpression::Name(class) => {
                    if let Some(resolved) = self.resolve_thoth_class(class, overlays) {
                        let Some(field) = exactly_one(
                            resolved
                                .fields
                                .into_iter()
                                .filter(|field| field.name == name),
                        ) else {
                            return TypeExpression::Unknown;
                        };
                        field_types.push(field.ty);
                    } else if class == "ConditionResult" && name == "Result" {
                        field_types.push(primitive("boolean"));
                    } else {
                        return TypeExpression::Unknown;
                    }
                }
                TypeExpression::Primitive(_)
                | TypeExpression::Array(_)
                | TypeExpression::Function { .. } => return TypeExpression::Unknown,
                TypeExpression::Union(_) => unreachable!("top-level union was flattened"),
            }
        }
        join(field_types)
    }

    fn fact_in_range(
        &self,
        path: &Path,
        file: &ParsedFile,
        range: TextRange,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let Some(thoth) = file.thoth.as_ref() else {
            return TypeExpression::Unknown;
        };
        exactly_one(
            thoth
                .expression_facts
                .iter()
                .filter(|fact| fact.range == range),
        )
        .map_or(TypeExpression::Unknown, |fact| {
            self.thoth_type_at_fact_inner(path, file, thoth, fact, overlays, guards)
        })
    }

    fn identifier_type(
        &self,
        path: &Path,
        file: &ParsedFile,
        thoth: &ThothFile,
        fact: &ThothExpressionFact,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let name = fact.text.as_str();
        let parameter = self.flow_parameter_type(path, thoth, fact.statement, name, overlays);

        // Assignment targets are expressions too. Resolve a declaration from
        // its own RHS instead of requiring the target to precede itself.
        if let Some(assignment) = thoth.assignments.iter().find(|assignment| {
            assignment.targets.len() == 1
                && assignment.targets[0].range == fact.range
                && assignment.targets[0].text == name
        }) {
            let binding_key = (path.to_path_buf(), fact.range);
            if !guards.bindings.insert(binding_key.clone()) {
                return TypeExpression::Unknown;
            }
            let result = thoth
                .annotations
                .variables
                .iter()
                .find(|annotation| annotation.target_range == fact.range)
                .map(|annotation| self.resolve_thoth_type(&annotation.ty, overlays))
                .unwrap_or_else(|| {
                    self.assignment_type(path, file, thoth, assignment, overlays, guards)
                });
            guards.bindings.remove(&binding_key);
            return narrow_identifier(thoth, fact, name, result);
        }

        // A write in a nested block can change an outer binding without being
        // a declaration that is lexically visible at this use. The flat flow
        // model cannot prove which path ran, so do not retain the pre-branch
        // type after such a write.
        if has_prior_nested_write(thoth, fact, name) {
            return TypeExpression::Unknown;
        }

        let mut candidates = thoth
            .assignments
            .iter()
            .filter(|assignment| {
                assignment.targets.len() == 1 && assignment.targets[0].text == name
            })
            .filter_map(|assignment| {
                let target = assignment.targets.first()?;
                let target_fact = exactly_one(
                    thoth
                        .expression_facts
                        .iter()
                        .filter(|candidate| candidate.range == target.range),
                )?;
                if !matches!(target_fact.kind, ThothExpressionKind::Identifier)
                    || !visible(thoth, target_fact.statement, fact.statement)
                    || (target_fact.statement.scope == fact.statement.scope
                        && target_fact.statement.order >= fact.statement.order)
                {
                    return None;
                }
                let depth = scope_depth(thoth, fact.statement.scope, target_fact.statement.scope)?;
                Some((
                    depth,
                    target_fact.statement.order,
                    target_fact.statement.scope,
                    assignment,
                    target.range,
                ))
            })
            .collect::<Vec<_>>();

        if parameter.is_some() {
            let function = enclosing_function(thoth, fact.statement.scope);
            candidates.retain(|candidate| enclosing_function(thoth, candidate.2) == function);
            if candidates.is_empty() {
                return parameter.map_or(TypeExpression::Unknown, |ty| {
                    narrow_identifier(thoth, fact, name, ty)
                });
            }
        }

        candidates.sort_by_key(|candidate| (candidate.0, candidate.1));
        let Some(depth) = candidates.first().map(|candidate| candidate.0) else {
            return parameter.map_or(TypeExpression::Unknown, |ty| {
                narrow_identifier(thoth, fact, name, ty)
            });
        };
        let binding_scope = candidates
            .iter()
            .find(|candidate| candidate.0 == depth)
            .map(|candidate| candidate.2)
            .expect("a minimum-depth binding candidate exists");
        let mut types = parameter.into_iter().collect::<Vec<_>>();
        let mut binding_candidates = candidates
            .iter()
            .filter(|candidate| candidate.0 == depth && candidate.2 == binding_scope)
            .collect::<Vec<_>>();
        binding_candidates.sort_by_key(|candidate| candidate.1);
        if let Some(local_index) = binding_candidates
            .iter()
            .rposition(|candidate| candidate.3.local)
        {
            types.clear();
            binding_candidates.drain(..local_index);
        }
        if let Some(annotation) = binding_candidates.first().and_then(|candidate| {
            thoth
                .annotations
                .variables
                .iter()
                .find(|annotation| annotation.target_range == candidate.4)
        }) {
            return narrow_identifier(
                thoth,
                fact,
                name,
                self.resolve_thoth_type(&annotation.ty, overlays),
            );
        }
        for (_, _, _scope, assignment, target_range) in binding_candidates {
            let binding_key = (path.to_path_buf(), *target_range);
            if !guards.bindings.insert(binding_key.clone()) {
                return TypeExpression::Unknown;
            }
            let ty = self.assignment_type(path, file, thoth, assignment, overlays, guards);
            guards.bindings.remove(&binding_key);
            if ty.is_unknown() {
                return TypeExpression::Unknown;
            }
            types.push(ty);
        }
        let result = match types.last() {
            Some(TypeExpression::Nil) => TypeExpression::Nil,
            Some(_) => join(
                types
                    .into_iter()
                    .filter(|ty| !matches!(ty, TypeExpression::Nil)),
            ),
            None => TypeExpression::Unknown,
        };
        narrow_identifier(thoth, fact, name, result)
    }

    fn assignment_type(
        &self,
        path: &Path,
        file: &ParsedFile,
        thoth: &ThothFile,
        assignment: &bg3_index::ThothAssignment,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let Some(value) = assignment.values.first() else {
            return TypeExpression::Nil;
        };
        let Some(fact) = exactly_one(
            thoth
                .expression_facts
                .iter()
                .filter(|fact| fact.range == value.range),
        ) else {
            return TypeExpression::Unknown;
        };
        self.thoth_type_at_fact_inner(path, file, thoth, fact, overlays, guards)
    }

    fn flow_parameter_type(
        &self,
        path: &Path,
        thoth: &ThothFile,
        statement: ThothStatementId,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<TypeExpression> {
        let function_range = enclosing_function(thoth, statement.scope)?;
        let declaration = thoth
            .declarations
            .iter()
            .find(|declaration| declaration.range == function_range)?;
        let resolved = self.resolve_thoth_function(&declaration.name, overlays)?;
        let ThothTypeSource::Loose {
            path: source_path, ..
        } = &resolved.source
        else {
            return None;
        };
        if source_path != path || resolved.name_range != declaration.name_range {
            return None;
        }
        let parameter_types = resolved
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
        Some(join(parameter_types))
    }

    fn call_type(
        &self,
        path: &Path,
        file: &ParsedFile,
        fact: &ThothExpressionFact,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let Some(thoth) = file.thoth.as_ref() else {
            return TypeExpression::Unknown;
        };
        let Some(call) = exactly_one(thoth.calls.iter().filter(|call| call.range == fact.range))
        else {
            return TypeExpression::Unknown;
        };

        if let Some(function) = self.resolve_thoth_function(&call.name, overlays) {
            return function_return_type(&function);
        }

        if thoth
            .declarations
            .iter()
            .any(|declaration| declaration.name == call.name)
        {
            return self.inferred_same_file_return(path, file, &call.name, overlays, guards);
        }

        if call.name == "ConditionResult" {
            return TypeExpression::Name("ConditionResult".into());
        }

        // Reachable unannotated helper inference is intentionally limited to
        // the queried source file.  Cross-file inference would require a
        // workspace-wide call graph and can silently cross module precedence.
        self.inferred_same_file_return(path, file, &call.name, overlays, guards)
    }

    fn inferred_same_file_return(
        &self,
        path: &Path,
        file: &ParsedFile,
        name: &str,
        overlays: &OverlaySet,
        guards: &mut Guards,
    ) -> TypeExpression {
        let key = (path.to_path_buf(), name.to_owned());
        if !guards.functions.insert(key.clone()) {
            return TypeExpression::Unknown;
        }
        let result = self
            .resolve(
                &SymbolTarget::Named {
                    kind: Some(bg3_index::THOTH_FUNCTION_KIND.into()),
                    name: name.into(),
                },
                overlays,
            )
            .first()
            .and_then(|first| {
                let effective = self
                    .resolve(
                        &SymbolTarget::Named {
                            kind: Some(bg3_index::THOTH_FUNCTION_KIND.into()),
                            name: name.into(),
                        },
                        overlays,
                    )
                    .into_iter()
                    .take_while(|definition| definition.rank == first.rank)
                    .collect::<Vec<_>>();
                if effective.len() != 1 || effective[0].ambiguous || effective[0].path != path {
                    return None;
                }
                let declaration = file
                    .thoth
                    .as_ref()?
                    .declarations
                    .iter()
                    .find(|declaration| declaration.name == name)?;
                let mut returns = file
                    .thoth
                    .as_ref()?
                    .returns
                    .iter()
                    .filter(|return_value| {
                        return_value.owner.as_ref().is_some_and(|owner| {
                            owner.name == name && owner.range == declaration.range
                        })
                    })
                    .collect::<Vec<_>>();
                returns.sort_by_key(|return_value| {
                    (
                        return_value.range.start.line,
                        return_value.range.start.character,
                    )
                });
                let has_scope_return = returns.iter().any(|return_value| {
                    return_value.statement.is_some_and(|statement| {
                        file.thoth
                            .as_ref()
                            .expect("Thoth file exists")
                            .scopes
                            .iter()
                            .find(|scope| scope.id == statement.scope)
                            .and_then(|scope| scope.parent)
                            .is_some_and(|parent| matches!(parent, ThothScopeId::Function { .. }))
                    })
                });
                let mut returned_scopes = HashSet::new();
                let mut types = Vec::new();
                for return_value in returns {
                    let Some(statement) = return_value.statement else {
                        continue;
                    };
                    if !returned_scopes.insert(statement.scope) {
                        continue;
                    }
                    if return_value.expressions.is_empty() {
                        types.push(TypeExpression::Nil);
                        continue;
                    }
                    let expression = &return_value.expressions[0];
                    let ty = exactly_one(
                        file.thoth
                            .as_ref()
                            .expect("Thoth file exists")
                            .expression_facts
                            .iter()
                            .filter(|fact| fact.range == expression.range),
                    )
                    .map_or(TypeExpression::Unknown, |fact| {
                        self.thoth_type_at_fact(path, file, fact, overlays, guards)
                    });
                    types.push(ty);
                }
                if !has_scope_return {
                    types.push(TypeExpression::Nil);
                }
                (!types.is_empty()).then(|| join(types))
            })
            .unwrap_or(TypeExpression::Unknown);
        guards.functions.remove(&key);
        result
    }
}

fn function_return_type(function: &ResolvedThothFunction) -> TypeExpression {
    let types = function
        .contracts
        .iter()
        .map(|contract| {
            contract
                .returns
                .first()
                .map_or(TypeExpression::Unknown, |return_value| {
                    return_value.ty.clone()
                })
        })
        .collect::<Vec<_>>();
    join(types)
}

fn primitive(name: &str) -> TypeExpression {
    TypeExpression::Primitive(match name {
        "boolean" => bg3_index::PrimitiveType::Boolean,
        "number" => bg3_index::PrimitiveType::Number,
        "string" => bg3_index::PrimitiveType::String,
        _ => unreachable!("flow primitive is fixed"),
    })
}

/// Applies only exact `x == nil`/`x ~= nil` evidence.
///
/// The branch facts intentionally do not treat truthiness as non-nil: Lua
/// false and nil are both falsy.  A statement after a direct-return branch is
/// narrowed only when the return is directly in that branch body.
fn narrow_identifier(
    file: &ThothFile,
    fact: &ThothExpressionFact,
    name: &str,
    ty: TypeExpression,
) -> TypeExpression {
    for control in &file.control_flow {
        for branch in &control.branches {
            if branch.scope.is_some_and(|scope| {
                scope_depth(file, fact.statement.scope, scope).is_some()
                    && enclosing_function(file, fact.statement.scope)
                        == enclosing_function(file, scope)
                    && !assigned_in_guarded_path(file, name, scope, fact)
            }) && let Some(condition) = branch.condition
                && let Some((condition_name, is_nil)) = nil_condition(file, condition)
                && condition_name == name
            {
                let is_nil = match branch.kind {
                    ThothIfBranchKind::Consequence => is_nil,
                    // Else-if and else branches do not have enough facts to
                    // prove the complement of all preceding conditions.
                    ThothIfBranchKind::ElseIf | ThothIfBranchKind::Else => return ty,
                };
                return if is_nil {
                    TypeExpression::Nil
                } else {
                    without_nil(ty)
                };
            }

            if control.statement.scope == fact.statement.scope
                && control.statement.order < fact.statement.order
                && control.branches.len() == 1
                && branch.kind == ThothIfBranchKind::Consequence
                && branch.scope.is_some_and(|scope| direct_return(file, scope))
                && !assigned_between(file, name, control.statement, fact.statement)
                && let Some(condition) = branch.condition
                && let Some((condition_name, is_nil)) = nil_condition(file, condition)
                && condition_name == name
            {
                // A direct return in the matching branch leaves its
                // complement as the only reachable remainder.
                return if is_nil {
                    without_nil(ty)
                } else {
                    TypeExpression::Nil
                };
            }
        }
    }
    ty
}

fn nil_condition(file: &ThothFile, range: TextRange) -> Option<(String, bool)> {
    let mut fact = exactly_one(
        file.expression_facts
            .iter()
            .filter(|fact| fact.range == range),
    )?;
    while let ThothExpressionKind::Parenthesized { expression } = &fact.kind {
        fact = exactly_one(
            file.expression_facts
                .iter()
                .filter(|candidate| candidate.range == *expression),
        )?;
    }
    let ThothExpressionKind::Binary {
        operator,
        left,
        right,
    } = &fact.kind
    else {
        return None;
    };
    let is_nil = match operator {
        ThothBinaryOperator::Equal => true,
        ThothBinaryOperator::NotEqual => false,
        _ => return None,
    };
    let left_fact = exactly_one(
        file.expression_facts
            .iter()
            .filter(|fact| fact.range == *left),
    );
    let right_fact = exactly_one(
        file.expression_facts
            .iter()
            .filter(|fact| fact.range == *right),
    );
    match (left_fact, right_fact) {
        (Some(identifier), Some(nil))
            if matches!(identifier.kind, ThothExpressionKind::Identifier)
                && matches!(
                    nil.kind,
                    ThothExpressionKind::Literal(bg3_index::ThothLiteralKind::Nil)
                ) =>
        {
            Some((identifier.text.clone(), is_nil))
        }
        (Some(nil), Some(identifier))
            if matches!(
                nil.kind,
                ThothExpressionKind::Literal(bg3_index::ThothLiteralKind::Nil)
            ) && matches!(identifier.kind, ThothExpressionKind::Identifier) =>
        {
            Some((identifier.text.clone(), is_nil))
        }
        _ => None,
    }
}

fn direct_return(file: &ThothFile, scope: ThothScopeId) -> bool {
    file.returns.iter().any(|return_fact| {
        return_fact
            .statement
            .is_some_and(|statement| statement.scope == scope && statement.order == 0)
    })
}

fn assigned_in_guarded_path(
    file: &ThothFile,
    name: &str,
    branch_scope: ThothScopeId,
    usage: &ThothExpressionFact,
) -> bool {
    file.assignments.iter().any(|assignment| {
        assignment.targets.len() == 1
            && assignment.targets[0].text == name
            && file.expression_facts.iter().any(|fact| {
                fact.range == assignment.targets[0].range
                    && scope_depth(file, fact.statement.scope, branch_scope).is_some()
                    && scope_depth(file, usage.statement.scope, fact.statement.scope).is_some()
                    && (fact.statement.scope != branch_scope
                        || position_before(assignment.targets[0].range.start, usage.range.start))
            })
    })
}

fn assigned_between(
    file: &ThothFile,
    name: &str,
    lower: ThothStatementId,
    upper: ThothStatementId,
) -> bool {
    file.assignments.iter().any(|assignment| {
        assignment.targets.len() == 1
            && assignment.targets[0].text == name
            && file.expression_facts.iter().any(|fact| {
                fact.range == assignment.targets[0].range
                    && fact.statement.scope == lower.scope
                    && fact.statement.order > lower.order
                    && fact.statement.order < upper.order
            })
    })
}

fn has_prior_nested_write(file: &ThothFile, usage: &ThothExpressionFact, name: &str) -> bool {
    let usage_function = enclosing_function(file, usage.statement.scope);
    file.assignments.iter().any(|assignment| {
        !assignment.local
            && assignment.targets.len() == 1
            && assignment.targets[0].text == name
            && position_before(assignment.targets[0].range.start, usage.range.start)
            && file.expression_facts.iter().any(|fact| {
                fact.range == assignment.targets[0].range
                    && fact.statement.scope != usage.statement.scope
                    && enclosing_function(file, fact.statement.scope) == usage_function
                    && scope_depth(file, usage.statement.scope, fact.statement.scope).is_none()
            })
    })
}

fn position_before(left: Position, right: Position) -> bool {
    (left.line, left.character) < (right.line, right.character)
}

fn without_nil(ty: TypeExpression) -> TypeExpression {
    match ty {
        TypeExpression::Nil => TypeExpression::Unknown,
        TypeExpression::Union(members) => TypeExpression::union(
            members
                .into_iter()
                .filter(|member| !matches!(member, TypeExpression::Nil)),
        ),
        other => other,
    }
}

fn unary_type(operator: ThothUnaryOperator, operand: TypeExpression) -> TypeExpression {
    match operator {
        ThothUnaryOperator::Not => primitive("boolean"),
        ThothUnaryOperator::Length => match operand {
            TypeExpression::Primitive(bg3_index::PrimitiveType::String)
            | TypeExpression::Array(_) => primitive("number"),
            _ => TypeExpression::Unknown,
        },
        ThothUnaryOperator::Negate => {
            if is_number(&operand) {
                primitive("number")
            } else {
                TypeExpression::Unknown
            }
        }
        ThothUnaryOperator::BitNot => {
            if is_condition_result(&operand) {
                TypeExpression::Name("ConditionResult".into())
            } else if is_integer(&operand) {
                TypeExpression::Primitive(bg3_index::PrimitiveType::Integer)
            } else {
                TypeExpression::Unknown
            }
        }
    }
}

fn binary_type(
    operator: ThothBinaryOperator,
    left: TypeExpression,
    right: TypeExpression,
) -> TypeExpression {
    use ThothBinaryOperator::*;
    match operator {
        Or | And => join([left, right]),
        Equal | NotEqual => primitive("boolean"),
        Less | LessOrEqual | Greater | GreaterOrEqual => {
            if (is_number(&left) && is_number(&right)) || (is_string(&left) && is_string(&right)) {
                primitive("boolean")
            } else {
                TypeExpression::Unknown
            }
        }
        BitAnd | BitOr | BitXor => {
            if is_condition_result(&left) && is_condition_result(&right) {
                TypeExpression::Name("ConditionResult".into())
            } else if is_integer(&left) && is_integer(&right) {
                TypeExpression::Primitive(bg3_index::PrimitiveType::Integer)
            } else {
                TypeExpression::Unknown
            }
        }
        ShiftLeft | ShiftRight => {
            if is_integer(&left) && is_integer(&right) {
                TypeExpression::Primitive(bg3_index::PrimitiveType::Integer)
            } else {
                TypeExpression::Unknown
            }
        }
        Add | Subtract | Multiply | Divide | FloorDivide | Modulo | Power => {
            if is_number(&left) && is_number(&right) {
                primitive("number")
            } else {
                TypeExpression::Unknown
            }
        }
        Concatenate => {
            if is_string(&left) && is_string(&right) {
                primitive("string")
            } else {
                TypeExpression::Unknown
            }
        }
    }
}

fn member_name(segment: &ThothMemberSegment) -> Option<String> {
    match segment.access {
        ThothMemberAccessKind::Dot | ThothMemberAccessKind::Method => {
            (!segment.text.is_empty()).then(|| segment.text.clone())
        }
        ThothMemberAccessKind::Bracket => {
            let quote = segment.text.chars().next()?;
            matches!(quote, '\'' | '"').then_some(())?;
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

fn is_condition_result(ty: &TypeExpression) -> bool {
    matches!(ty, TypeExpression::Name(name) if name == "ConditionResult")
}

fn is_integer(ty: &TypeExpression) -> bool {
    matches!(
        ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Integer)
    )
}

fn is_number(ty: &TypeExpression) -> bool {
    matches!(
        ty,
        TypeExpression::Primitive(
            bg3_index::PrimitiveType::Integer | bg3_index::PrimitiveType::Number
        )
    )
}

fn is_string(ty: &TypeExpression) -> bool {
    matches!(
        ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::String)
    )
}

fn join(types: impl IntoIterator<Item = TypeExpression>) -> TypeExpression {
    let types = types.into_iter().collect::<Vec<_>>();
    if types.iter().any(TypeExpression::is_unknown) {
        TypeExpression::Unknown
    } else {
        TypeExpression::union(types)
    }
}

fn known(ty: TypeExpression) -> Option<TypeExpression> {
    (!ty.is_unknown()).then_some(ty)
}

fn exactly_one<T>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut values = values.into_iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn visible(file: &ThothFile, declaration: ThothStatementId, usage: ThothStatementId) -> bool {
    scope_depth(file, usage.scope, declaration.scope).is_some()
        && (declaration.scope != usage.scope || declaration.order < usage.order)
}

fn has_identifier_binding(file: &ThothFile, name: &str, usage: ThothStatementId) -> bool {
    let parameter = enclosing_function(file, usage.scope).is_some_and(|function| {
        file.declarations
            .iter()
            .find(|declaration| declaration.range == function)
            .is_some_and(|declaration| {
                declaration
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == name)
            })
    });
    parameter
        || file.assignments.iter().any(|assignment| {
            assignment.targets.len() == 1
                && assignment.targets[0].text == name
                && file.expression_facts.iter().any(|fact| {
                    fact.range == assignment.targets[0].range
                        && visible(file, fact.statement, usage)
                })
        })
}

fn enclosing_function(file: &ThothFile, mut scope: ThothScopeId) -> Option<TextRange> {
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

fn scope_depth(file: &ThothFile, mut from: ThothScopeId, target: ThothScopeId) -> Option<usize> {
    let mut depth = 0;
    loop {
        if from == target {
            return Some(depth);
        }
        from = file.scopes.iter().find(|scope| scope.id == from)?.parent?;
        depth += 1;
    }
}

fn contains(range: TextRange, position: Position) -> bool {
    (position.line, position.character) >= (range.start.line, range.start.character)
        && (position.line, position.character) < (range.end.line, range.end.character)
}

fn range_size(range: TextRange) -> (u32, u32, u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
        range.start.line,
        range.start.character,
    )
}
