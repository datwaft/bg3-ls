use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{
    HoverMarkup, MAX_HOVER_LIST_ENTRIES, OverlaySet, WorkspaceSnapshot, markdown_inline_code,
    range_contains,
};
use bg3_index::{
    Definition, FUNCTIONS, ModuleRole, OSIRIS_CONTRACTS, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND,
    OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND, OsirisContractKind, PackagedOsirisCallableRole,
    PackagedOsirisResolution, PackagedThothApiResolution, PackagedThothApiSymbol,
    PackagedThothApiSymbolKind, PackagedThothCatalog, Position, SchemaDefinition, SchemaField,
    SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAnnotations,
    ThothFunctionContract, context_member, context_members, context_properties, context_property,
    context_side, enum_value, field_documentation, field_kind, function_spec, functor_prefix,
    functor_prefixes, is_lsx_value_field, is_structural_stats_value, member_enumeration,
    osiris_contract, osiris_legacy_signatures,
};
/// The semantic category of one completion result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    Class,
    Field,
    Value,
    Function,
    Reference,
}

/// An editor-neutral completion edit and its supporting metadata.
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub new_text: String,
    /// Optional ranking key. Curated vocabulary sorts ahead of observed
    /// evidence so the completion cap cannot drop it on large projects.
    pub sort_text: Option<String>,
    pub range: TextRange,
    pub kind: CompletionKind,
    pub snippet: bool,
}

/// A capped completion list that reports whether valid results were omitted.
#[derive(Clone, Debug, Default)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    pub incomplete: bool,
}

/// A verified signature and the active zero-based parameter.
#[derive(Clone, Debug)]
pub struct SignatureHelp {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<String>,
    pub active_parameter: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AnnotatedSignature {
    parameters: Vec<String>,
    returns: Vec<String>,
    description: Vec<String>,
}

impl AnnotatedSignature {
    /// Reports whether the annotation supplies any explicit type evidence.
    ///
    /// A prose-only annotation documents behavior without typing it, so
    /// consumers must keep declared parameter names visible instead of
    /// rendering an empty parameter list.
    fn typed(&self) -> bool {
        !self.parameters.is_empty() || !self.returns.is_empty()
    }

    fn label(&self, name: &str) -> String {
        let mut label = format!("{name}({})", self.parameters.join(", "));
        if !self.returns.is_empty() {
            label.push_str(": ");
            label.push_str(&self.returns.join(", "));
        }
        label
    }

    fn documentation(&self, prefix: &str) -> String {
        let mut parts = self.description.clone();
        parts.push(prefix.to_owned());
        let mut documentation = parts.join("\n\n");
        if !self.returns.is_empty() {
            documentation.push_str(&format!("\n\nReturns: `{}`", self.returns.join(", ")));
        }
        documentation
    }
}

fn annotated_signature(contract: &ThothFunctionContract) -> AnnotatedSignature {
    AnnotatedSignature {
        parameters: contract
            .parameters
            .iter()
            .map(|parameter| {
                let variadic = if parameter.variadic { "..." } else { "" };
                format!("{variadic}{}: {}", parameter.name, parameter.ty)
            })
            .collect(),
        returns: contract
            .returns
            .iter()
            .map(|return_value| return_value.ty.to_string())
            .collect(),
        description: contract.description.clone(),
    }
}

fn function_annotation(annotations: &ThothAnnotations, name: &str) -> Option<AnnotatedSignature> {
    annotations
        .functions
        .iter()
        .find(|annotation| annotation.name.as_deref() == Some(name))
        .and_then(|annotation| annotation.contracts.first())
        .map(annotated_signature)
}

fn function_annotation_at(
    annotations: &ThothAnnotations,
    name: &str,
    selection_range: TextRange,
) -> Option<AnnotatedSignature> {
    annotations
        .functions
        .iter()
        .find(|annotation| {
            annotation.name.as_deref() == Some(name)
                && annotation.name_range == Some(selection_range)
        })
        .and_then(|annotation| annotation.contracts.first())
        .map(annotated_signature)
}

/// Adds annotation prose as escaped text and keeps generated metadata as Markdown.
fn annotated_hover_documentation(
    mut markdown: HoverMarkup,
    signature: &AnnotatedSignature,
    prefix: &str,
) -> HoverMarkup {
    if !signature.description.is_empty() {
        markdown = markdown.prose(&signature.description.join("\n\n"));
    }
    markdown = markdown.markdown(prefix);
    if !signature.returns.is_empty() {
        markdown = markdown.markdown(&format!(
            "Returns: {}",
            markdown_inline_code(&signature.returns.join(", "))
        ));
    }
    markdown
}

fn type_annotation_hover(
    annotations: &ThothAnnotations,
    word: &str,
    position: Position,
) -> Option<String> {
    if let Some(class) = annotations
        .classes
        .iter()
        .find(|class| class.name == word && range_contains(class.name_range, position))
    {
        let fields = class
            .fields
            .iter()
            .take(MAX_HOVER_LIST_ENTRIES)
            .map(|field| {
                format!(
                    "- {}: {}",
                    markdown_inline_code(&field.name),
                    markdown_inline_code(&field.ty.to_string())
                )
            })
            .collect::<Vec<_>>();
        let mut markdown = HoverMarkup::new("Thoth class", word);
        if !fields.is_empty() {
            let mut field_list = String::from("Fields:\n");
            field_list.push_str(&fields.join("\n"));
            let omitted = class.fields.len().saturating_sub(MAX_HOVER_LIST_ENTRIES);
            if omitted > 0 {
                field_list.push_str(&format!("\n- … {omitted} additional fields omitted"));
            }
            markdown = markdown.markdown(&field_list);
        }
        return Some(markdown.finish());
    }
    if let Some((class, field)) = annotations.classes.iter().find_map(|class| {
        class
            .fields
            .iter()
            .find(|field| field.name == word && range_contains(field.name_range, position))
            .map(|field| (class, field))
    }) {
        return Some(
            HoverMarkup::new("Thoth field", word)
                .fact("Type", &field.ty.to_string())
                .fact("Class", &class.name)
                .finish(),
        );
    }
    if let Some(alias) = annotations
        .aliases
        .iter()
        .find(|alias| alias.name == word && range_contains(alias.name_range, position))
    {
        return Some(
            HoverMarkup::new("Thoth alias", word)
                .fact("Type", &alias.ty.to_string())
                .finish(),
        );
    }
    annotations
        .variables
        .iter()
        .find(|variable| variable.target == word && range_contains(variable.target_range, position))
        .map(|variable| {
            HoverMarkup::new("Thoth type", word)
                .fact("Type", &variable.ty.to_string())
                .finish()
        })
}

impl WorkspaceSnapshot {
    /// Computes context-aware completion from the current unsaved document text.
    pub fn completion(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
        snippets: bool,
    ) -> CompletionList {
        let Some((_, file)) = self.file(path, overlays) else {
            return CompletionList::default();
        };
        let Some(text) = overlays.get(path).map(|overlay| overlay.text.as_str()) else {
            return CompletionList::default();
        };
        let Some(line) = source_line(text, position.line) else {
            return CompletionList::default();
        };
        let cursor = usize::try_from(position.character)
            .unwrap_or(usize::MAX)
            .min(line.len());
        let before = &line[..cursor];
        if file.source.kind == SourceKind::Osiris {
            let mut items = self.complete_osiris(text, before, position, overlays, snippets);
            items.sort_by(|left, right| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
                    .then(left.detail.cmp(&right.detail))
            });
            let incomplete = items.len() > self.max_completion_items;
            items.truncate(self.max_completion_items);
            return CompletionList { items, incomplete };
        }
        if file.source.kind == bg3_index::SourceKind::Thoth {
            let prefix = identifier_prefix(before);
            let mut items = self
                .thoth_member_completions(path, position, overlays)
                .unwrap_or_else(|| {
                    self.complete_thoth_functions(prefix, position, overlays, snippets)
                });
            items.sort_by(|left, right| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
                    .then(left.detail.cmp(&right.detail))
            });
            let incomplete = items.len() > self.max_completion_items;
            items.truncate(self.max_completion_items);
            return CompletionList { items, incomplete };
        }
        let entry = active_definition(&file.definitions, position);
        let mut items = if let Some(prefix) = quoted_clause_prefix(before, "type") {
            self.complete_entry_types(prefix, position)
        } else if let Some(prefix) = quoted_clause_prefix(before, "using") {
            entry.map_or_else(Vec::new, |entry| {
                self.complete_symbols(&entry.kind, prefix, position, overlays)
            })
        } else if let Some(data) = value_context(before) {
            if data.in_field_name {
                entry.map_or_else(Vec::new, |entry| {
                    self.complete_fields(path, entry, data.prefix, position, file)
                })
            } else {
                entry.map_or_else(Vec::new, |entry| {
                    self.complete_value(
                        path,
                        entry,
                        &data.field,
                        data.value_before_cursor,
                        data.prefix,
                        position,
                        overlays,
                        snippets,
                    )
                })
            }
        } else {
            Vec::new()
        };

        items.sort_by(|left, right| {
            let left_key = left.sort_text.as_ref().unwrap_or(&left.label);
            let right_key = right.sort_text.as_ref().unwrap_or(&right.label);
            left_key
                .to_ascii_lowercase()
                .cmp(&right_key.to_ascii_lowercase())
                .then(left.detail.cmp(&right.detail))
        });
        let incomplete = items.len() > self.max_completion_items;
        items.truncate(self.max_completion_items);
        CompletionList { items, incomplete }
    }

    /// Returns verified signatures for curated functions and declared Thoth helpers.
    pub fn signature_help(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<SignatureHelp> {
        let (_, file) = self.file(path, overlays)?;
        let text = overlays.get(path)?.text.as_str();
        if file.source.kind == SourceKind::Osiris {
            let before = source_prefix(text, position)?;
            return self.osiris_signature_help(before, overlays);
        }
        let line = source_line(text, position.line)?;
        let cursor = usize::try_from(position.character).ok()?.min(line.len());
        let before = &line[..cursor];
        // A Stats value starts with an unmatched document quote while the user edits it.
        // Remove the data-clause prefix before balancing expression quotes and calls.
        let expression = value_context(before)
            .map(|context| context.value_before_cursor)
            .unwrap_or(before);
        let context = call_context(expression)?;
        if let Some(function) = function_spec(&context.function) {
            let form =
                function.form_for_call(context.argument + 1, context.first_argument.as_deref());
            let mut label = format!("{}(", function.name);
            label.push_str(
                &form
                    .parameters
                    .iter()
                    .map(|parameter| parameter.label)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            if form.variadic {
                if !form.parameters.is_empty() {
                    label.push_str(", ");
                }
                label.push_str("...");
            }
            label.push(')');
            return Some(SignatureHelp {
                label,
                documentation: function.documentation.into(),
                parameters: form
                    .parameters
                    .iter()
                    .map(|parameter| parameter.label.into())
                    .collect(),
                active_parameter: context.argument,
            });
        }

        let target = SymbolTarget::Named {
            kind: Some(THOTH_FUNCTION_KIND.into()),
            name: context.function.clone(),
        };
        let resolved = self.resolve(&target, overlays);
        if let Some(signature) =
            self.loose_thoth_annotation_signature(&context.function, &resolved, overlays)
        {
            let typed = signature.typed();
            let (label, parameters) = if typed {
                (
                    signature.label(&context.function),
                    signature.parameters.clone(),
                )
            } else {
                let declared = resolved
                    .first()
                    .map(|definition| thoth_parameters(&definition.definition))
                    .unwrap_or_default();
                (
                    format!("{}({})", context.function, declared.join(", ")),
                    declared,
                )
            };
            return Some(SignatureHelp {
                label,
                documentation: signature.documentation(if typed {
                    "Explicit Thoth annotation."
                } else {
                    "Parameter types are not inferred."
                }),
                parameters,
                active_parameter: context.argument,
            });
        }
        if let Some(definition) = resolved.into_iter().next() {
            let parameters = thoth_parameters(&definition.definition);
            return Some(SignatureHelp {
                label: format!("{}({})", definition.definition.name, parameters.join(", ")),
                documentation: format!(
                    "Declared Thoth helper from module `{}`. Parameter types are not inferred.",
                    definition.module
                ),
                parameters,
                active_parameter: context.argument,
            });
        }
        let (parameters, ambiguous, module) = self.packaged_thoth_signature(&context.function)?;
        if let Some((signature, module, entries)) =
            self.packaged_thoth_annotation(&context.function)
        {
            let typed = signature.typed();
            let (label, parameters) = if typed {
                (
                    signature.label(&context.function),
                    signature.parameters.clone(),
                )
            } else {
                (
                    format!("{}({})", context.function, parameters.join(", ")),
                    parameters,
                )
            };
            let fallback = format!(
                "Installed Thoth evidence from module `{module}`. Parameter types are not inferred."
            );
            let mut documentation = signature.documentation(if typed {
                "Explicit installed Thoth annotation."
            } else {
                &fallback
            });
            documentation.push_str(&format!("\n\nModule: `{module}`"));
            if !entries.is_empty() {
                documentation.push_str(&format!("\n\nPackage entries: `{}`", entries.join("`, `")));
            }
            return Some(SignatureHelp {
                label,
                documentation,
                parameters,
                active_parameter: context.argument,
            });
        }
        Some(SignatureHelp {
            label: format!("{}({})", context.function, parameters.join(", ")),
            documentation: if ambiguous {
                format!(
                    "Installed Thoth evidence from module `{module}` has same-priority ambiguity. Parameter types are not inferred."
                )
            } else {
                format!(
                    "Installed Thoth evidence from module `{module}`. Parameter types are not inferred."
                )
            },
            parameters,
            active_parameter: context.argument,
        })
    }

    /// Adds function documentation when normal symbol hover has no result.
    pub(crate) fn language_hover_markdown(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        if let Some(hover) = self.thoth_member_hover(path, position, overlays) {
            return Some(hover);
        }
        if self
            .thoth_member_completions(path, position, overlays)
            .is_some()
        {
            return None;
        }
        let text = overlays.get(path)?.text.as_str();
        let line = source_line(text, position.line)?;
        if let Some(markdown) = self.stats_property_hover(path, line, &position, overlays) {
            return Some(markdown);
        }
        let word = word_at(line, usize::try_from(position.character).ok()?)?;
        if let Some(markdown) = member_word_context(line, usize::try_from(position.character).ok()?)
            .and_then(|member| self.stats_member_hover(member, word))
        {
            return Some(markdown);
        }
        if let Some(function) = function_spec(word) {
            return Some(
                HoverMarkup::new("Stats function", function.name)
                    .markdown(function.documentation)
                    .finish(),
            );
        }
        if let Some(property) = context_property(word) {
            return Some(
                HoverMarkup::new("Context property", &property.name)
                    .fact("Kind", &property.kind)
                    .markdown(&property.documentation)
                    .finish(),
            );
        }
        if let Some(value) = enum_value(word) {
            return Some(
                HoverMarkup::new("Enum value", value.name)
                    .fact("Parameter", value.parameter)
                    .fact("Function", value.function)
                    .markdown(value.documentation)
                    .finish(),
            );
        }
        if let Some(prefix) = functor_prefix(word) {
            return Some(
                HoverMarkup::new("Functor prefix", &format!("{}:", prefix.name))
                    .fact("Kind", prefix.kind)
                    .markdown(prefix.documentation)
                    .finish(),
            );
        }
        if let Some(data) = data_context(line)
            && let Some((_, file)) = self.file(path, overlays)
            && let Some(entry) = active_definition(&file.definitions, position)
        {
            for schema in schemas_for_definition(self, path, entry) {
                let Some(field) = schema.field(&data.field) else {
                    continue;
                };
                let Some(enumeration) = field.enumeration_type_name.as_ref() else {
                    continue;
                };
                if self
                    .schema
                    .enumerations
                    .get(enumeration)
                    .is_some_and(|values| values.iter().any(|value| value == word))
                {
                    return Some(
                        HoverMarkup::new("Enum value", word)
                            .fact("Enumeration", enumeration)
                            .fact("Field", &data.field)
                            .finish(),
                    );
                }
            }
        }
        if let Some((_, file)) = self.file(path, overlays)
            && let Some(thoth) = &file.thoth
            && let Some(hover) = type_annotation_hover(
                &thoth.annotations,
                word,
                Position {
                    line: position.line,
                    character: position.character,
                },
            )
        {
            return Some(hover);
        }
        if let Some((_, file)) = self.file(path, overlays)
            && let Some(thoth) = &file.thoth
            && let Some(signature) = function_annotation(&thoth.annotations, word)
        {
            return Some(
                annotated_hover_documentation(
                    HoverMarkup::new("Thoth function", word)
                        .fact("Signature", &signature.label(word)),
                    &signature,
                    "Explicit Thoth annotation.",
                )
                .finish(),
            );
        }
        if let Some(evidence) = self.loose_thoth_hover(word, overlays) {
            return Some(evidence);
        }
        if let Some((signature, module, entries)) = self.packaged_thoth_annotation(word) {
            let mut markdown = annotated_hover_documentation(
                HoverMarkup::new("Installed Thoth function", word)
                    .fact("Module", &module)
                    .fact("Signature", &signature.label(word)),
                &signature,
                "Explicit installed Thoth annotation.",
            );
            if !entries.is_empty() {
                markdown = markdown.fact("Package entries", &bounded_package_entries(&entries));
            }
            return Some(markdown.finish());
        }
        if let Some(evidence) = self.packaged_thoth_function_evidence(word) {
            return Some(evidence);
        }
        if let Some(hover) = self.thoth_flow_hover(path, position, overlays) {
            return Some(hover);
        }
        None
    }

    /// Renders hover for the property name of a legacy Stats `data` clause.
    ///
    /// Shows schema types from the effective inheritance chain, curated
    /// documentation when the name is cataloged, and a fenced expression
    /// preview when the value parses as structural Stats-value syntax.
    fn stats_property_hover(
        &self,
        path: &Path,
        line: &str,
        position: &Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let column = usize::try_from(position.character).ok()?;
        let (name, value) = data_clause_spans(line, column)?;
        let mut markdown = HoverMarkup::new("Stats property", name);

        let types: BTreeSet<String> = self
            .file(path, overlays)
            .and_then(|(_, file)| active_definition(&file.definitions, *position))
            .map_or_else(BTreeSet::new, |entry| {
                schemas_for_definition(self, path, entry)
                    .into_iter()
                    .filter_map(|schema| schema.field(name))
                    .flat_map(|field| {
                        [
                            field.field_type.clone(),
                            field.object_type.clone(),
                            field.enumeration_type_name.clone(),
                        ]
                        .into_iter()
                        .flatten()
                    })
                    .collect()
            });
        if !types.is_empty() {
            let listed = types.into_iter().collect::<Vec<_>>().join(", ");
            markdown = markdown.fact("Types", &listed);
        }
        if let Some(documentation) = field_documentation(name) {
            markdown = markdown.markdown(documentation);
        }
        if let Some(preview) =
            value.filter(|value| !value.is_empty() && is_structural_stats_value(value))
        {
            let preview = format!("```bg3_stats_value\n{}\n```", format_value_preview(preview));
            markdown = markdown.markdown(&preview);
        }
        Some(markdown.finish())
    }

    /// Completes schema export types and categories.
    fn complete_entry_types(&self, prefix: &str, position: Position) -> Vec<CompletionItem> {
        let names: BTreeSet<_> = self
            .schema
            .by_id
            .values()
            .flat_map(|schema| {
                [
                    schema.export_type.as_deref(),
                    schema.category.as_deref(),
                    Some(schema.name.as_str()),
                ]
            })
            .flatten()
            .filter(|name| starts_with_case_insensitive(name, prefix))
            .collect();
        names
            .into_iter()
            .map(|name| basic_item(name, prefix, position, CompletionKind::Class))
            .collect()
    }

    /// Completes valid fields from the union of inferred schemas.
    fn complete_fields(
        &self,
        path: &Path,
        entry: &Definition,
        prefix: &str,
        position: Position,
        file: &bg3_index::ParsedFile,
    ) -> Vec<CompletionItem> {
        let schemas = schemas_for_definition(self, path, entry);
        let present: BTreeSet<_> = file
            .definitions
            .iter()
            .find(|candidate| candidate.name == entry.name)
            .map(|definition| definition.fields.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let mut fields = BTreeMap::<&str, &SchemaField>::new();
        for schema in schemas {
            for field in schema.fields.values() {
                let name = field.legacy_name();
                if !field.is_internal
                    && !field.auto_generated
                    && !present.contains(name)
                    && !present.contains(field.name.as_str())
                    && starts_with_case_insensitive(name, prefix)
                {
                    fields.entry(name).or_insert(field);
                }
            }
        }
        fields
            .into_iter()
            .map(|(name, field)| {
                let mut item = basic_item(name, prefix, position, CompletionKind::Field);
                item.detail = field.field_type.clone();
                item.documentation = field.description.clone();
                item
            })
            .collect()
    }

    /// Renders hover for one word in member position inside a Stats value.
    ///
    /// Schema enumerations prove their own values; context members and side
    /// selectors come from the curated catalog.
    fn stats_member_hover(&self, member: MemberWord<'_>, word: &str) -> Option<String> {
        if let Some(object) = member.object {
            if let Some(values) = self.schema.enumerations.get(object) {
                if values.iter().any(|value| value == word) {
                    return Some(
                        HoverMarkup::new("Enum value", word)
                            .fact("Enumeration", object)
                            .finish(),
                    );
                }
                return None;
            }
            if let Some(values) = member_enumeration(object)
                && values.contains(&word)
            {
                return Some(
                    HoverMarkup::new("Enum value", word)
                        .fact("Enumeration", object)
                        .finish(),
                );
            }
            if object == "context"
                && let Some(member) = context_member(word)
            {
                let kind = if member.function {
                    "Context function"
                } else {
                    "Context member"
                };
                return Some(
                    HoverMarkup::new(kind, member.name)
                        .fact("Object", "context")
                        .markdown(member.documentation)
                        .finish(),
                );
            }
            return None;
        }
        if !member.is_object_position {
            return None;
        }
        if let Some(values) = self.schema.enumerations.get(word) {
            return Some(
                HoverMarkup::new("Enumeration", word)
                    .fact("Documented values", &values.len().to_string())
                    .finish(),
            );
        }
        if let Some(values) = member_enumeration(word) {
            return Some(
                HoverMarkup::new("Enumeration", word)
                    .fact("Documented values", &values.len().to_string())
                    .finish(),
            );
        }
        if word == "context" {
            return Some(
                HoverMarkup::new("Context object", "context")
                    .markdown("The evaluation context. Fetch data from the causing character with `context.Source` or the affected character with `context.Target`.")
                    .finish(),
            );
        }
        if let Some(documentation) = context_side(word) {
            return Some(
                HoverMarkup::new("Context side", word)
                    .markdown(documentation)
                    .finish(),
            );
        }
        None
    }

    /// Completes enum, reference, localization, and expression values.
    #[allow(clippy::too_many_arguments)]
    fn complete_value(
        &self,
        path: &Path,
        entry: &Definition,
        field_name: &str,
        value_before_cursor: &str,
        prefix: &str,
        position: Position,
        overlays: &OverlaySet,
        snippets: bool,
    ) -> Vec<CompletionItem> {
        let schema_fields: Vec<_> = schemas_for_definition(self, path, entry)
            .into_iter()
            .filter_map(|schema| schema.field(field_name))
            .collect();
        for field in &schema_fields {
            if let Some(enumeration) = field.enumeration_type_name.as_ref()
                && let Some(values) = self.schema.enumerations.get(enumeration)
            {
                return values
                    .iter()
                    .filter(|value| starts_with_case_insensitive(value, prefix))
                    .map(|value| basic_item(value, prefix, position, CompletionKind::Value))
                    .collect();
            }
            if field
                .field_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("boolean"))
            {
                return ["true", "false"]
                    .into_iter()
                    .filter(|value| starts_with_case_insensitive(value, prefix))
                    .map(|value| basic_item(value, prefix, position, CompletionKind::Value))
                    .collect();
            }
            if field
                .field_type
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("translatedstring"))
            {
                return self.complete_localization(prefix, position, overlays);
            }
            if let Some(kind) = field.object_type.as_deref() {
                return self.complete_symbols_with_insertion(
                    kind,
                    prefix,
                    position,
                    overlays,
                    SymbolInsertion::Uuid,
                );
            }
        }

        if let Some(object) = member_object_before_partial(value_before_cursor) {
            if let Some(values) = self.schema.enumerations.get(object) {
                return values
                    .iter()
                    .filter(|value| starts_with_case_insensitive(value, prefix))
                    .map(|value| {
                        let mut item = basic_item(value, prefix, position, CompletionKind::Value);
                        item.detail = Some("enum value".into());
                        item.documentation = Some(format!("Value of enumeration `{object}`."));
                        item
                    })
                    .collect();
            }
            if object == "context" {
                return context_members()
                    .filter(|member| starts_with_case_insensitive(member.name, prefix))
                    .map(|member| {
                        let kind = if member.function {
                            CompletionKind::Function
                        } else {
                            CompletionKind::Field
                        };
                        let mut item = basic_item(member.name, prefix, position, kind);
                        item.detail = Some("context member".into());
                        item.documentation = Some(member.documentation.to_owned());
                        item
                    })
                    .collect();
            }
            if object == "Target" {
                return context_properties()
                    .filter(|property| starts_with_case_insensitive(&property.name, prefix))
                    .map(|property| {
                        let mut item =
                            basic_item(&property.name, prefix, position, CompletionKind::Value);
                        item.detail = Some(property.kind.clone());
                        item.documentation = Some(property.documentation.clone());
                        item
                    })
                    .collect();
            }
        }
        if let Some(call) = call_context(value_before_cursor)
            && let Some(function) = function_spec(&call.function)
        {
            if let Some(kind) = function.parameter_kind(
                call.argument,
                call.argument + 1,
                call.first_argument.as_deref(),
            ) {
                return self.complete_symbols(kind, prefix, position, overlays);
            }
            if let Some(values) = function.parameter_enum_values(
                call.argument,
                call.argument + 1,
                call.first_argument.as_deref(),
            ) {
                return values
                    .iter()
                    .filter(|value| starts_with_case_insensitive(value, prefix))
                    .map(|value| {
                        let mut item = basic_item(value, prefix, position, CompletionKind::Value);
                        item.detail = Some("enum value".into());
                        item.documentation = Some(format!(
                            "Parameter of curated function `{}`.",
                            function.name
                        ));
                        item
                    })
                    .collect();
            }
        }
        if let Some(kind) = field_kind(field_name) {
            return self.complete_symbols(kind, prefix, position, overlays);
        }

        let mut items = Vec::new();
        if is_functor_statement_head(
            &value_before_cursor[..value_before_cursor.len() - prefix.len()],
        ) {
            for prefix_spec in functor_prefixes() {
                if starts_with_case_insensitive(prefix_spec.name, prefix) {
                    let mut item =
                        basic_item(prefix_spec.name, prefix, position, CompletionKind::Value);
                    item.detail = Some(prefix_spec.kind.to_owned());
                    item.documentation = Some(prefix_spec.documentation.to_owned());
                    item.sort_text = Some(curated_sort_text(prefix_spec.name));
                    items.push(item);
                }
            }
        }
        for function in FUNCTIONS {
            if starts_with_case_insensitive(function.name, prefix) {
                let mut item =
                    basic_item(function.name, prefix, position, CompletionKind::Function);
                item.documentation = Some(function.documentation.into());
                item.sort_text = Some(curated_sort_text(function.name));
                let form = function.default_form;
                if snippets && !form.parameters.is_empty() {
                    let parameters = form
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(index, parameter)| format!("${{{}:{}}}", index + 1, parameter.label))
                        .collect::<Vec<_>>()
                        .join(",");
                    item.new_text = format!("{}({parameters})", function.name);
                    item.snippet = true;
                }
                items.push(item);
            }
        }
        for item in self.complete_thoth_functions(prefix, position, overlays, snippets) {
            let ambiguous = item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("same-rank ambiguity"));
            let installed_variant = items.iter().any(|existing| {
                existing.label == item.label
                    && existing.detail == item.detail
                    && existing.documentation == item.documentation
                    && existing.new_text != item.new_text
            });
            if ambiguous
                || installed_variant
                || !items.iter().any(|existing| existing.label == item.label)
            {
                items.push(item);
            }
        }
        for property in context_properties() {
            if starts_with_case_insensitive(&property.name, prefix) {
                let mut item = basic_item(&property.name, prefix, position, CompletionKind::Value);
                item.detail = Some(property.kind.clone());
                item.documentation = Some(property.documentation.clone());
                item.sort_text = Some(curated_sort_text(&property.name));
                items.push(item);
            }
        }
        let curated: BTreeSet<_> = FUNCTIONS.iter().map(|function| function.name).collect();
        for layer in self.layers.iter().rev() {
            for function in layer.functions.values() {
                if !curated.contains(function.name.as_str())
                    && starts_with_case_insensitive(&function.name, prefix)
                    && !items.iter().any(|item| item.label == function.name)
                {
                    let mut item =
                        basic_item(&function.name, prefix, position, CompletionKind::Function);
                    item.detail = Some(format!(
                        "observed {} times; {}-{} arguments",
                        function.count, function.min_arity, function.max_arity
                    ));
                    items.push(item);
                }
            }
        }
        items
    }

    /// Completes effective helper declarations and keeps same-rank ambiguity visible.
    fn complete_thoth_functions(
        &self,
        prefix: &str,
        position: Position,
        overlays: &OverlaySet,
        snippets: bool,
    ) -> Vec<CompletionItem> {
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for layer in self.layers.iter().rev() {
            let mut candidates = BTreeMap::<String, Vec<String>>::new();
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                for definition in &overlay.parsed.definitions {
                    add_thoth_candidate(&mut candidates, definition, prefix);
                }
            }
            for record in layer.definitions_of_kind(THOTH_FUNCTION_KIND) {
                if !overlays.contains(record.path.as_ref()) {
                    add_thoth_candidate(&mut candidates, record.definition(), prefix);
                }
            }
            for (name, parameter_lists) in candidates {
                if !seen.insert(name.clone()) {
                    continue;
                }
                let ambiguous = parameter_lists.len() > 1;
                let annotation =
                    self.loose_thoth_completion_annotation(&layer.spec.name, &name, overlays);
                let typed = annotation.as_ref().is_some_and(AnnotatedSignature::typed);
                for parameters in parameter_lists {
                    let mut item = basic_item(&name, prefix, position, CompletionKind::Function);
                    item.detail = Some(
                        if let Some(signature) = annotation.as_ref().filter(|s| s.typed()) {
                            signature.label(&name)
                        } else if ambiguous {
                            format!("{} (same-rank ambiguity)", layer.spec.name)
                        } else {
                            layer.spec.name.clone()
                        },
                    );
                    item.documentation = Some(match annotation.as_ref() {
                        Some(signature) => {
                            let mut documentation = if typed {
                                "Explicit Thoth annotation.".to_owned()
                            } else {
                                "Declared Thoth helper. Parameter types are not inferred."
                                    .to_owned()
                            };
                            if !signature.description.is_empty() {
                                documentation.push_str("\n\n");
                                documentation.push_str(&signature.description.join("\n\n"));
                            }
                            documentation
                        }
                        None => {
                            "Declared Thoth helper. Parameter types are not inferred.".to_owned()
                        }
                    });
                    if snippets {
                        let parameters = split_parameters(&parameters);
                        if parameters.is_empty() {
                            item.new_text = format!("{name}()");
                        } else {
                            let placeholders = parameters
                                .iter()
                                .enumerate()
                                .map(|(index, parameter)| format!("${{{}:{parameter}}}", index + 1))
                                .collect::<Vec<_>>()
                                .join(", ");
                            item.new_text = format!("{name}({placeholders})");
                            item.snippet = true;
                        }
                    }
                    items.push(item);
                }
            }
        }
        for (name, parameter_lists, module, ambiguous) in self.packaged_thoth_candidates(prefix) {
            if !seen.insert(name.clone()) {
                continue;
            }
            for parameters in parameter_lists {
                let mut item = basic_item(&name, prefix, position, CompletionKind::Function);
                let annotation = self.packaged_thoth_annotation(&name);
                item.detail = Some(
                    if let Some((signature, installed_module, _)) = annotation
                        .as_ref()
                        .filter(|(signature, _, _)| signature.typed())
                    {
                        format!(
                            "{} (installed {})",
                            signature.label(&name),
                            installed_module
                        )
                    } else if ambiguous {
                        format!("installed {module} (same-rank ambiguity)")
                    } else {
                        format!("installed {module}")
                    },
                );
                item.documentation = Some(match annotation.as_ref() {
                    Some((signature, installed_module, entries)) => {
                        let mut documentation = if signature.typed() {
                            format!(
                                "Explicit installed Thoth annotation from module {installed_module}."
                            )
                        } else {
                            "Installed Thoth declaration from configured package data. Parameter types are not inferred."
                                .to_owned()
                        };
                        if !signature.description.is_empty() {
                            documentation.push_str("\n\n");
                            documentation.push_str(&signature.description.join("\n\n"));
                        }
                        if !entries.is_empty() {
                            documentation
                                .push_str(&format!(" Package entries: {}.", entries.join(", ")));
                        }
                        documentation
                    }
                    None => "Installed Thoth declaration from configured package data. Parameter types are not inferred."
                        .to_owned(),
                });
                if snippets {
                    if parameters.is_empty() {
                        item.new_text = format!("{name}()");
                    } else {
                        let placeholders = parameters
                            .iter()
                            .enumerate()
                            .map(|(index, parameter)| format!("${{{}:{parameter}}}", index + 1))
                            .collect::<Vec<_>>()
                            .join(", ");
                        item.new_text = format!("{name}({placeholders})");
                        item.snippet = true;
                    }
                }
                items.push(item);
            }
        }
        items
    }

    /// Completes effective symbols of one semantic kind.
    fn complete_symbols(
        &self,
        kind: &str,
        prefix: &str,
        position: Position,
        overlays: &OverlaySet,
    ) -> Vec<CompletionItem> {
        let insertion = if kind == "ActionResource" {
            SymbolInsertion::Alias
        } else {
            SymbolInsertion::Name
        };
        self.complete_symbols_with_insertion(kind, prefix, position, overlays, insertion)
    }

    /// Collapses override chains while preserving duplicates at the effective module rank.
    fn complete_symbols_with_insertion(
        &self,
        kind: &str,
        prefix: &str,
        position: Position,
        overlays: &OverlaySet,
        insertion: SymbolInsertion,
    ) -> Vec<CompletionItem> {
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for layer in self.layers.iter().rev() {
            let mut candidates = BTreeMap::<String, Vec<String>>::new();
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                for definition in &overlay.parsed.definitions {
                    add_symbol_candidate(&mut candidates, definition, kind, prefix, insertion);
                }
            }
            for record in layer.definitions_of_kind(kind) {
                if overlays.contains(record.path.as_ref()) {
                    continue;
                }
                let definition = record.definition();
                add_symbol_candidate(&mut candidates, definition, kind, prefix, insertion);
            }
            for (label, insertions) in candidates {
                if !seen.insert(label.clone()) {
                    continue;
                }
                let ambiguous = insertions.len() > 1;
                for new_text in insertions {
                    let mut item = basic_item(&label, prefix, position, CompletionKind::Reference);
                    item.new_text = new_text;
                    item.detail = Some(if ambiguous {
                        format!("{} (same-rank ambiguity)", layer.spec.name)
                    } else {
                        layer.spec.name.clone()
                    });
                    items.push(item);
                }
            }
        }
        for candidate in self.packaged_stats.definitions_of_kind(kind) {
            let definition = candidate.definition();
            let label = match insertion {
                SymbolInsertion::Name => definition.name.clone(),
                SymbolInsertion::Alias | SymbolInsertion::Uuid => definition
                    .aliases
                    .iter()
                    .find(|alias| starts_with_case_insensitive(alias, prefix))
                    .cloned()
                    .unwrap_or_else(|| definition.name.clone()),
            };
            if !starts_with_case_insensitive(&label, prefix) || !seen.insert(label.clone()) {
                continue;
            }
            let mut item = basic_item(&label, prefix, position, CompletionKind::Reference);
            item.new_text = match insertion {
                SymbolInsertion::Name | SymbolInsertion::Alias => label,
                SymbolInsertion::Uuid => definition
                    .uuid
                    .map_or_else(|| definition.name.clone(), |uuid| uuid.to_string()),
            };
            item.detail = Some(format!("{} (packaged)", candidate.source().module()));
            items.push(item);
        }
        items
    }

    /// Completes Osiris declarations and generated engine contracts in their
    /// valid statement positions.
    fn complete_osiris(
        &self,
        document: &str,
        before: &str,
        position: Position,
        overlays: &OverlaySet,
        snippets: bool,
    ) -> Vec<CompletionItem> {
        let context = match osiris_completion_context(document, before, position.line) {
            Some(context) => context,
            None => return Vec::new(),
        };
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for layer in self.layers.iter().rev() {
            let mut candidates = BTreeMap::<(String, String, u16), Vec<Definition>>::new();
            let incomplete_overlay_paths = overlays
                .for_module(&layer.spec.name)
                .filter(|(_, overlay)| {
                    overlay.parsed.source.kind == SourceKind::Osiris
                        && overlay.parsed.definitions.is_empty()
                        && !overlay.parsed.issues.is_empty()
                })
                .map(|(path, _)| path.clone())
                .collect::<BTreeSet<_>>();
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                for definition in &overlay.parsed.definitions {
                    add_osiris_completion_candidate(
                        &mut candidates,
                        definition,
                        context.prefix,
                        context.goals,
                    );
                }
            }
            for record in &layer.definitions {
                if overlays.contains(record.path.as_ref())
                    && !incomplete_overlay_paths.contains(record.path.as_ref())
                {
                    continue;
                }
                add_osiris_completion_candidate(
                    &mut candidates,
                    record.definition(),
                    context.prefix,
                    context.goals,
                );
            }
            for ((namespace, name, arity), definitions) in candidates {
                if !seen.insert((namespace.clone(), name.clone(), arity)) {
                    continue;
                }
                // A same-rank PROC/QRY disagreement cannot be placed safely.
                // Keep the key shadowed while suppressing both declarations;
                // otherwise iteration order would choose one role by accident.
                let mixed_callable_roles = namespace == "callable"
                    && definitions
                        .iter()
                        .any(|definition| definition.kind == OSIRIS_PROCEDURE_KIND)
                    && definitions
                        .iter()
                        .any(|definition| definition.kind == OSIRIS_QUERY_KIND);
                if mixed_callable_roles {
                    continue;
                }
                for definition in definitions {
                    let callable_kind = match definition.kind.as_str() {
                        OSIRIS_PROCEDURE_KIND => Some(OsirisContractKind::Call),
                        OSIRIS_QUERY_KIND => Some(OsirisContractKind::Query),
                        _ => None,
                    };
                    if !osiris_callable_allowed(callable_kind, context.position) {
                        continue;
                    }
                    let mut item = basic_item(
                        &name,
                        context.prefix,
                        position,
                        if context.goals {
                            CompletionKind::Reference
                        } else {
                            CompletionKind::Function
                        },
                    );
                    item.detail = Some(if context.goals {
                        format!("Osiris goal — {}", layer.spec.name)
                    } else {
                        format!(
                            "{} /{arity} — {}",
                            osiris_kind_label(&definition.kind),
                            layer.spec.name
                        )
                    });
                    if snippets && !context.goals {
                        let parameters = stored_parameters(&definition);
                        item.new_text = osiris_call_snippet(&name, arity, &parameters);
                        item.snippet = arity > 0;
                    }
                    items.push(item);
                }
            }
        }
        if !context.goals {
            for layer in self
                .layers
                .iter()
                .rev()
                .filter(|layer| layer.spec.role == ModuleRole::Base)
            {
                let module = layer.spec.name.clone();
                for (name, arity, callable) in self
                    .packaged_osiris
                    .completions_for_module(&module, context.prefix)
                {
                    if !seen.insert(("callable".into(), name.clone(), arity)) {
                        continue;
                    }
                    let Some(kind) = osiris_contract_kind_from_packaged_role(callable.role) else {
                        continue;
                    };
                    if !osiris_callable_allowed(Some(kind), context.position) {
                        continue;
                    }
                    let mut item =
                        basic_item(&name, context.prefix, position, CompletionKind::Function);
                    item.detail = Some(format!(
                        "{} /{arity} — installed {module}",
                        osiris_contract_kind_label(kind)
                    ));
                    if snippets {
                        item.new_text = osiris_call_snippet(&name, arity, &callable.parameters);
                        item.snippet = arity > 0;
                    }
                    items.push(item);
                }
            }

            for contract in OSIRIS_CONTRACTS {
                let arity = u16::try_from(contract.parameters.len()).unwrap_or(u16::MAX);
                if !starts_with_case_insensitive(contract.name, context.prefix)
                    || !osiris_callable_allowed(Some(contract.kind), context.position)
                    || osiris_contract(OSIRIS_CONTRACTS, contract.name, arity).is_none()
                    || !seen.insert(("callable".into(), contract.name.to_owned(), arity))
                {
                    continue;
                }
                let mut item = basic_item(
                    contract.name,
                    context.prefix,
                    position,
                    CompletionKind::Function,
                );
                item.detail = Some(format!(
                    "{} /{arity} — generated BG3 engine catalog",
                    osiris_contract_kind_label(contract.kind)
                ));
                if let Some(description) =
                    bg3_index::osiris_callable_description(contract.name, arity)
                {
                    item.documentation = Some(description.to_owned());
                }
                if snippets {
                    let parameters = contract
                        .parameters
                        .iter()
                        .map(|parameter| {
                            format!(
                                "{} {} {}",
                                osiris_parameter_direction(parameter.direction),
                                parameter.type_name,
                                parameter.name
                            )
                        })
                        .collect::<Vec<_>>();
                    item.new_text = osiris_call_snippet(contract.name, arity, &parameters);
                    item.snippet = arity > 0;
                }
                items.push(item);
            }

            // Keep the small legacy event table discoverable for contracts
            // that are not present in the generated story header. Generated
            // entries win through the same completion key, while aliases
            // that remain legacy-only still get a conservative event item.
            for (name, aliases) in osiris_legacy_signatures() {
                let arity = u16::try_from(aliases.len()).unwrap_or(u16::MAX);
                if !starts_with_case_insensitive(name, context.prefix)
                    || !osiris_callable_allowed(Some(OsirisContractKind::Event), context.position)
                    || !seen.insert(("callable".into(), (*name).to_owned(), arity))
                {
                    continue;
                }
                let mut item = basic_item(name, context.prefix, position, CompletionKind::Function);
                item.detail = Some(format!(
                    "Osiris engine event /{arity} — legacy event catalog"
                ));
                if snippets {
                    let parameters = aliases
                        .iter()
                        .map(|alias| (*alias).to_owned())
                        .collect::<Vec<_>>();
                    item.new_text = osiris_call_snippet(name, arity, &parameters);
                    item.snippet = arity > 0;
                }
                items.push(item);
            }
        }
        items
    }

    /// Returns installed declarations and call observations by effective base rank.
    fn packaged_thoth_candidates(
        &self,
        prefix: &str,
    ) -> Vec<(String, Vec<Vec<String>>, String, bool)> {
        let catalog = self.packaged_thoth();
        let facts = self.packaged_thoth_facts();
        let mut results = Vec::new();
        for layer in self
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
        {
            let mut by_entry = BTreeMap::<String, (u8, bool, Vec<_>)>::new();
            for source in catalog
                .sources()
                .filter(|source| source.module() == layer.spec.name)
            {
                let Some((priority, ambiguous)) =
                    packaged_thoth_entry_priority(&catalog, source.module(), source.entry())
                else {
                    continue;
                };
                by_entry
                    .entry(source.entry().to_owned())
                    .or_insert_with(|| (priority, ambiguous, Vec::new()));
            }
            for record in facts.iter() {
                if record.source().module() != layer.spec.name {
                    continue;
                }
                let entry = record.source().entry().to_owned();
                let Some((priority, _, records)) = by_entry.get_mut(&entry) else {
                    continue;
                };
                let source_is_catalog_candidate = catalog
                    .sources_for(record.source().module(), record.source().entry())
                    .iter()
                    .any(|candidate| candidate == record.source());
                if source_is_catalog_candidate && record.source().priority() == *priority {
                    records.push(record);
                }
            }
            let mut candidates = BTreeMap::<String, (Vec<Vec<String>>, bool)>::new();
            for (_, entry_ambiguous, records) in by_entry.values() {
                for record in records {
                    for declaration in &record.facts().declarations {
                        if starts_with_case_insensitive(&declaration.name, prefix) {
                            let candidate = candidates.entry(declaration.name.clone()).or_default();
                            candidate.1 |= *entry_ambiguous || !candidate.0.is_empty();
                            candidate.0.push(
                                declaration
                                    .parameters
                                    .iter()
                                    .map(|parameter| parameter.name.clone())
                                    .collect(),
                            );
                        }
                    }
                }
            }
            let declared = candidates.keys().cloned().collect::<BTreeSet<_>>();
            for (_, entry_ambiguous, records) in by_entry.values() {
                for record in records {
                    for call in &record.facts().calls {
                        if starts_with_case_insensitive(&call.name, prefix)
                            && !declared.contains(&call.name)
                        {
                            let parameters = vec!["unknown".to_owned(); usize::from(call.arity)];
                            let candidate = candidates.entry(call.name.clone()).or_default();
                            candidate.1 |= *entry_ambiguous;
                            if !candidate.0.iter().any(|existing| existing == &parameters) {
                                candidate.0.push(parameters);
                            }
                        }
                    }
                }
            }
            results.extend(candidates.into_iter().map(
                |(name, (parameter_lists, entry_ambiguous))| {
                    (
                        name,
                        parameter_lists,
                        layer.spec.name.clone(),
                        entry_ambiguous,
                    )
                },
            ));
        }
        results
    }

    /// Returns the highest-priority installed declaration signature.
    fn packaged_thoth_signature(&self, name: &str) -> Option<(Vec<String>, bool, String)> {
        let (_, candidates, module, ambiguous) = self
            .packaged_thoth_candidates(name)
            .into_iter()
            .find(|(candidate, _, _, _)| candidate == name)?;
        let width = candidates.iter().map(Vec::len).max().unwrap_or_default();
        let parameters = (0..width)
            .map(|index| {
                let mut values = candidates
                    .iter()
                    .filter_map(|parameters| parameters.get(index))
                    .collect::<BTreeSet<_>>();
                if values.len() == 1 {
                    values
                        .pop_first()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into())
                } else {
                    "unknown".into()
                }
            })
            .collect();
        Some((parameters, ambiguous, module))
    }

    /// Resolves explicit contracts from the configured packaged API index.
    /// Package entries remain provenance labels; they are never converted to
    /// navigable filesystem locations.
    fn packaged_thoth_annotation(
        &self,
        name: &str,
    ) -> Option<(AnnotatedSignature, String, Vec<String>)> {
        let api = self.packaged_thoth_api();
        for layer in self
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
        {
            match api.resolve(&layer.spec.name, PackagedThothApiSymbolKind::Function, name) {
                PackagedThothApiResolution::Missing => continue,
                PackagedThothApiResolution::Ambiguous(_) => return None,
                PackagedThothApiResolution::Unique(candidate) => {
                    let PackagedThothApiSymbol::Function {
                        annotation: Some(annotation),
                        ..
                    } = candidate.symbol()
                    else {
                        return None;
                    };
                    let signature = annotation.contracts.first().map(annotated_signature)?;
                    return Some((
                        signature,
                        layer.spec.name.clone(),
                        vec![candidate.source().entry().to_owned()],
                    ));
                }
            }
        }
        None
    }

    /// Formats installed declaration and call evidence without a source path.
    fn packaged_thoth_function_evidence(&self, name: &str) -> Option<String> {
        let (parameters, ambiguous, module) = self.packaged_thoth_signature(name)?;
        let mut calls = BTreeSet::new();
        let mut entries = BTreeSet::new();
        let catalog = self.packaged_thoth();
        let base_modules: BTreeSet<_> = self
            .layers
            .iter()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
            .map(|layer| layer.spec.name.as_str())
            .collect();
        for record in self.packaged_thoth_facts().iter() {
            if record.source().module() != module || !base_modules.contains(module.as_str()) {
                continue;
            }
            let Some((top_priority, _)) = packaged_thoth_entry_priority(
                &catalog,
                record.source().module(),
                record.source().entry(),
            ) else {
                continue;
            };
            let source_is_catalog_candidate = catalog
                .sources_for(record.source().module(), record.source().entry())
                .iter()
                .any(|candidate| candidate == record.source());
            if record.source().priority() != top_priority {
                continue;
            }
            if !source_is_catalog_candidate {
                continue;
            }
            let has_name = record
                .facts()
                .declarations
                .iter()
                .any(|declaration| declaration.name == name)
                || record.facts().calls.iter().any(|call| call.name == name);
            if !has_name {
                continue;
            }
            entries.insert(record.source().entry().to_owned());
            for call in &record.facts().calls {
                if call.name == name {
                    calls.insert(call.arity);
                }
            }
        }
        let mut markdown =
            HoverMarkup::new("Installed Thoth function", name).fact("Module", &module);
        if !parameters.is_empty() {
            markdown = markdown.fact("Signature", &format!("{}({})", name, parameters.join(", ")));
        }
        if !calls.is_empty() {
            let arities = calls
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            markdown = markdown.fact("Observed call arities", &arities);
        }
        if ambiguous {
            markdown = markdown.markdown("Same-priority package evidence is ambiguous.");
        }
        if !entries.is_empty() {
            markdown = markdown.fact(
                "Package entries",
                &bounded_package_entries(&entries.into_iter().collect::<Vec<_>>()),
            );
        }
        Some(markdown.finish())
    }

    /// Resolves loose Thoth evidence by configured module precedence.
    fn loose_thoth_hover(&self, name: &str, overlays: &OverlaySet) -> Option<String> {
        for layer in self.layers.iter().rev() {
            let mut candidates = Vec::new();
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                candidates.extend(
                    overlay
                        .parsed
                        .definitions
                        .iter()
                        .filter(|definition| {
                            definition.kind == THOTH_FUNCTION_KIND && definition.name == name
                        })
                        .map(|definition| thoth_parameters(definition).join(", ")),
                );
            }
            candidates.extend(
                layer
                    .definitions_of_kind(THOTH_FUNCTION_KIND)
                    .filter(|record| {
                        !overlays.contains(record.path.as_ref()) && record.definition().name == name
                    })
                    .map(|record| thoth_parameters(record.definition()).join(", ")),
            );
            if candidates.len() == 1 {
                return Some(
                    HoverMarkup::new("Thoth function", name)
                        .fact("Signature", &format!("{}({})", name, candidates[0]))
                        .fact("Module", &layer.spec.name)
                        .finish(),
                );
            }
            if !candidates.is_empty() {
                return Some(
                    HoverMarkup::new("Thoth function", name)
                        .fact("Module", &layer.spec.name)
                        .fact("Declarations", &candidates.len().to_string())
                        .markdown(
                            "Same-rank Thoth declarations are ambiguous. The signature is not verified.",
                        )
                        .finish(),
                );
            }
        }
        for layer in self.layers.iter().rev() {
            if let Some(function) = layer.functions.get(name) {
                return Some(
                    HoverMarkup::new("Observed Thoth function", &function.name)
                        .fact("Observed calls", &function.count.to_string())
                        .fact(
                            "Observed arity",
                            &format!("{} to {}", function.min_arity, function.max_arity),
                        )
                        .markdown("No verified signature is available.")
                        .finish(),
                );
            }
        }
        None
    }

    /// Returns explicit annotation evidence for the effective loose declaration.
    ///
    /// Resolution has already applied module precedence and overlay replacement.
    /// If several declarations share the winning rank, every declaration must
    /// carry the same contract before typed metadata is exposed.
    fn loose_thoth_annotation_signature(
        &self,
        name: &str,
        definitions: &[crate::ResolvedDefinition],
        overlays: &OverlaySet,
    ) -> Option<AnnotatedSignature> {
        let first = definitions.first()?;
        let top_rank = first.rank;
        let candidates = definitions
            .iter()
            .take_while(|definition| definition.rank == top_rank)
            .collect::<Vec<_>>();
        let signatures = candidates
            .iter()
            .map(|definition| {
                self.thoth_annotation_for_path(
                    &definition.path,
                    name,
                    definition.definition.selection_range,
                    overlays,
                )
            })
            .collect::<Vec<_>>();
        let Some(Some(signature)) = signatures.first() else {
            return None;
        };
        if signatures
            .iter()
            .all(|candidate| candidate.as_ref() == Some(signature))
        {
            Some(signature.clone())
        } else {
            None
        }
    }

    /// Renders explicit metadata for a loose function before generic hover.
    pub(crate) fn annotated_thoth_hover(
        &self,
        name: &str,
        definitions: &[crate::ResolvedDefinition],
        overlays: &OverlaySet,
    ) -> Option<String> {
        let signature = self.loose_thoth_annotation_signature(name, definitions, overlays)?;
        let label = if signature.typed() {
            signature.label(name)
        } else {
            let parameters = definitions
                .first()
                .map(|definition| thoth_parameters(&definition.definition).join(", "))
                .unwrap_or_default();
            format!("{name}({parameters})")
        };
        let effective = definitions.first()?;
        Some(
            annotated_hover_documentation(
                HoverMarkup::new("Thoth function", name).fact("Signature", &label),
                &signature,
                "Explicit Thoth annotation.",
            )
            .fact("Module", &effective.module)
            .fact("Source", &effective.path.display().to_string())
            .finish(),
        )
    }

    fn thoth_annotation_for_path(
        &self,
        path: &Path,
        name: &str,
        selection_range: TextRange,
        overlays: &OverlaySet,
    ) -> Option<AnnotatedSignature> {
        if let Some(overlay) = overlays.get(path) {
            return overlay.parsed.thoth.as_ref().and_then(|thoth| {
                function_annotation_at(&thoth.annotations, name, selection_range)
            });
        }
        let (_, file) = self.layers.iter().find_map(|layer| {
            layer
                .file(path)
                .map(|file| (layer.spec.name.as_str(), file))
        })?;
        file.thoth
            .as_ref()
            .and_then(|thoth| function_annotation_at(&thoth.annotations, name, selection_range))
    }

    fn loose_thoth_completion_annotation(
        &self,
        module: &str,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<AnnotatedSignature> {
        let layer = self.layers.iter().find(|layer| layer.spec.name == module)?;
        let mut signatures = Vec::new();
        for (_, overlay) in overlays.for_module(module) {
            for definition in overlay.parsed.definitions.iter().filter(|definition| {
                definition.kind == THOTH_FUNCTION_KIND && definition.name == name
            }) {
                signatures.push(overlay.parsed.thoth.as_ref().and_then(|thoth| {
                    function_annotation_at(&thoth.annotations, name, definition.selection_range)
                }));
            }
        }
        for (path, file) in &layer.files {
            if overlays.contains(path) {
                continue;
            }
            for definition in file.definitions.iter().filter(|definition| {
                definition.kind == THOTH_FUNCTION_KIND && definition.name == name
            }) {
                signatures.push(file.thoth.as_ref().and_then(|thoth| {
                    function_annotation_at(&thoth.annotations, name, definition.selection_range)
                }));
            }
        }
        let Some(Some(signature)) = signatures.first() else {
            return None;
        };
        signatures
            .iter()
            .all(|candidate| candidate.as_ref() == Some(signature))
            .then(|| signature.clone())
    }

    /// Returns source-backed signatures for visible user callables and databases.
    fn osiris_signature_help(&self, before: &str, overlays: &OverlaySet) -> Option<SignatureHelp> {
        let context = call_context(before)?;
        let database = context.function.starts_with("DB_");
        let database_schemas = database.then(|| self.osiris_database_schemas(overlays));
        let mut candidates = Vec::<(usize, Definition)>::new();
        for (rank, layer) in self.layers.iter().enumerate() {
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                for definition in &overlay.parsed.definitions {
                    if osiris_signature_candidate(definition, &context.function, database) {
                        candidates.push((rank, definition.clone()));
                    }
                }
            }
            for record in &layer.definitions {
                if !overlays.contains(record.path.as_ref())
                    && osiris_signature_candidate(record.definition(), &context.function, database)
                {
                    candidates.push((rank, record.definition().clone()));
                }
            }
        }
        let arity = if database {
            database_schemas.as_ref().and_then(|schemas| {
                schemas
                    .keys()
                    .filter(|(name, arity)| {
                        name == &context.function
                            && (*arity == 0 || usize::from(*arity) > context.argument)
                    })
                    .map(|(_, arity)| *arity)
                    .min()
            })
        } else {
            candidates
                .iter()
                .filter_map(|(_, definition)| definition.arity)
                .filter(|arity| *arity == 0 || usize::from(*arity) > context.argument)
                .min()
        };
        // A visible loose declaration always outranks installed evidence.
        let Some(arity) = arity else {
            if database {
                return None;
            }
            if let Some(signature) =
                self.packaged_osiris_signature_help(&context.function, context.argument)
            {
                return Some(signature);
            }
            return self.generated_osiris_signature_help(&context.function, context.argument);
        };
        candidates.retain(|(_, definition)| definition.arity == Some(arity));
        let parameters = if database {
            database_schemas
                .as_ref()
                .and_then(|schemas| schemas.get(&(context.function.clone(), arity)))
                .map(crate::osiris_database_parameter_types)
                .unwrap_or_else(|| positional_parameters(arity))
        } else {
            osiris_declared_parameters(&candidates, arity)
        };
        Some(SignatureHelp {
            label: format!("{}({})", context.function, parameters.join(", ")),
            documentation: if database {
                "Signature evidence from configured loose Osiris goals. Unknown columns remain explicit."
                    .into()
            } else {
                "Declared in a configured loose Osiris goal. Untyped parameters remain explicit."
                    .into()
            },
            parameters,
            active_parameter: context.argument,
        })
    }

    /// Renders installed procedure and query declarations for signature help.
    ///
    /// Loose declarations always win. Same-priority installed disagreements
    /// keep positional placeholders instead of choosing one declaration.
    fn packaged_osiris_signature_help(&self, name: &str, argument: usize) -> Option<SignatureHelp> {
        // The active parameter needs a declared position, mirroring the loose
        // arity selection.
        let needed = u16::try_from(argument + 1).ok()?;
        for layer in self
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
        {
            let Some(arity) = self
                .packaged_osiris
                .effective_arities(&layer.spec.name, name)
                .into_iter()
                .filter(|arity| *arity >= needed)
                .min()
            else {
                continue;
            };
            match self.packaged_osiris.resolve(&layer.spec.name, name, arity) {
                PackagedOsirisResolution::Missing => continue,
                PackagedOsirisResolution::Ambiguous(_) => {
                    return Some(SignatureHelp {
                        label: format!("{name}({})", positional_parameters(arity).join(", ")),
                        documentation:
                            "Same-priority installed package declarations disagree on this callable. Parameter types stay untyped."
                                .into(),
                        parameters: positional_parameters(arity),
                        active_parameter: argument,
                    });
                }
                PackagedOsirisResolution::Unique(candidate) => {
                    let callable = candidate.callable();
                    return Some(SignatureHelp {
                        label: format!("{name}({})", callable.parameters.join(", ")),
                        documentation: format!(
                            "Declared in an installed package goal for module `{}`.",
                            candidate.source().module()
                        ),
                        parameters: callable.parameters.clone(),
                        active_parameter: argument,
                    });
                }
            }
        }
        None
    }

    /// Returns signature help from the versioned engine contract catalog.
    fn generated_osiris_signature_help(
        &self,
        name: &str,
        argument: usize,
    ) -> Option<SignatureHelp> {
        let needed = u16::try_from(argument + 1).ok()?;
        let arity = OSIRIS_CONTRACTS
            .iter()
            .filter(|contract| {
                contract.name == name
                    && (contract.parameters.is_empty()
                        || contract.parameters.len() >= usize::from(needed))
            })
            .map(|contract| contract.parameters.len())
            .min();
        let Some(arity) = arity else {
            let aliases = osiris_legacy_signatures()
                .iter()
                .filter(|(candidate, aliases)| {
                    *candidate == name
                        && (aliases.is_empty() || aliases.len() >= usize::from(needed))
                })
                .min_by_key(|(_, aliases)| aliases.len())
                .map(|(_, aliases)| *aliases)?;
            let parameters = aliases
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect::<Vec<_>>();
            return Some(SignatureHelp {
                label: format!("{name}({})", parameters.join(", ")),
                documentation: "Verified against the curated legacy engine event catalog.".into(),
                parameters,
                active_parameter: argument,
            });
        };
        let contract = osiris_contract(OSIRIS_CONTRACTS, name, u16::try_from(arity).ok()?)?;
        let parameters = contract
            .parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{} {} {}",
                    crate::osiris_parameter_direction(parameter.direction),
                    parameter.type_name,
                    parameter.name
                )
            })
            .collect::<Vec<_>>();
        let mut documentation = format!(
            "Verified against the generated BG3 engine contract catalog (build `{}`).",
            bg3_index::OSIRIS_CATALOG_SOURCE_VERSION
        );
        if let Some(description) = bg3_index::osiris_callable_description(name, arity as u16) {
            documentation = format!("{description}\n\n{documentation}");
        }
        Some(SignatureHelp {
            label: format!("{name}({})", parameters.join(", ")),
            documentation,
            parameters,
            active_parameter: argument,
        })
    }

    /// Completes localization handles with their known version suffix.
    fn complete_localization(
        &self,
        prefix: &str,
        position: Position,
        overlays: &OverlaySet,
    ) -> Vec<CompletionItem> {
        self.complete_symbols("Localization", prefix, position, overlays)
            .into_iter()
            .map(|mut item| {
                let target = SymbolTarget::Named {
                    kind: Some("Localization".into()),
                    name: item.label.clone(),
                };
                if let Some(version) = self
                    .resolve(&target, overlays)
                    .first()
                    .and_then(|definition| definition.definition.fields.get("Version"))
                {
                    item.new_text = format!("{};{version}", item.label);
                }
                item
            })
            .collect()
    }
}

#[derive(Debug)]
struct DataContext<'a> {
    field: String,
    prefix: &'a str,
    value_before_cursor: &'a str,
    in_field_name: bool,
}

#[derive(Debug)]
struct CallContext {
    function: String,
    argument: usize,
    first_argument: Option<String>,
}

/// One parenthesis pair while scanning an incomplete call.
///
/// Osiris casts use grouping parentheses, such as `(CHARACTER)_Caster`, so
/// the scanner must keep them separate from callable parentheses.  Otherwise
/// the cast's closing parenthesis can close the surrounding call.
enum CallParen {
    Call {
        function: String,
        argument: usize,
        arguments_start: usize,
    },
    Group,
}

#[derive(Clone, Copy, Debug)]
struct OsirisCompletionContext<'a> {
    prefix: &'a str,
    goals: bool,
    position: OsirisCompletionPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OsirisCompletionPosition {
    RuleHead,
    Condition,
    Action,
    Declaration,
}

#[derive(Clone, Copy, Debug)]
enum SymbolInsertion {
    Name,
    Alias,
    Uuid,
}

/// Adds one helper declaration to its module-rank completion group.
fn add_thoth_candidate(
    candidates: &mut BTreeMap<String, Vec<String>>,
    definition: &Definition,
    prefix: &str,
) {
    if definition.kind == THOTH_FUNCTION_KIND
        && starts_with_case_insensitive(&definition.name, prefix)
    {
        candidates.entry(definition.name.clone()).or_default().push(
            definition
                .fields
                .get("Parameters")
                .cloned()
                .unwrap_or_default(),
        );
    }
}

/// Joins package-entry provenance while keeping one hover section bounded.
fn bounded_package_entries(entries: &[String]) -> String {
    let omitted = entries.len().saturating_sub(MAX_HOVER_LIST_ENTRIES);
    let mut visible = entries
        .iter()
        .take(MAX_HOVER_LIST_ENTRIES)
        .cloned()
        .collect::<Vec<_>>();
    if omitted > 0 {
        visible.push(format!("… {omitted} additional entries omitted"));
    }
    visible.join(", ")
}

/// Returns parameter labels stored by the Thoth syntax extractor.
fn thoth_parameters(definition: &Definition) -> Vec<String> {
    definition
        .fields
        .get("Parameters")
        .map_or_else(Vec::new, |parameters| split_parameters(parameters))
}

/// Splits a stored parameter list while preserving declared names.
fn split_parameters(parameters: &str) -> Vec<String> {
    parameters
        .split(',')
        .map(str::trim)
        .filter(|parameter| !parameter.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Adds one logical Osiris declaration to a module-rank completion group.
fn add_osiris_completion_candidate(
    candidates: &mut BTreeMap<(String, String, u16), Vec<Definition>>,
    definition: &Definition,
    prefix: &str,
    goals: bool,
) {
    if goals {
        if definition.kind == OSIRIS_GOAL_KIND
            && starts_with_case_insensitive(&definition.name, prefix)
        {
            candidates
                .entry(("goal".into(), definition.name.clone(), 0))
                .or_default()
                .push(definition.clone());
        }
        return;
    }
    let namespace = match definition.kind.as_str() {
        OSIRIS_DATABASE_KIND => "database",
        OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND => "callable",
        _ => return,
    };
    if !starts_with_case_insensitive(&definition.name, prefix) {
        return;
    }
    let Some(arity) = definition.arity else {
        return;
    };
    candidates
        .entry((namespace.into(), definition.name.clone(), arity))
        .or_default()
        .push(definition.clone());
}

/// Selects one incomplete Osiris call name or parent-goal string.
fn osiris_completion_context<'a>(
    document: &str,
    line: &'a str,
    line_number: u32,
) -> Option<OsirisCompletionContext<'a>> {
    if osiris_prefix_is_in_comment(document, line_number, line) {
        return None;
    }
    let trimmed = line.trim_start();
    if let Some(prefix) = trimmed.strip_prefix("ParentTargetEdge") {
        let prefix = prefix.trim_start().strip_prefix('"')?;
        return (!prefix.contains('"')).then_some(OsirisCompletionContext {
            prefix,
            goals: true,
            position: OsirisCompletionPosition::Declaration,
        });
    }
    if !inside_osiris_statement_section(document, line_number) {
        return None;
    }

    let mut call = trimmed;
    let mut position = None;
    loop {
        let previous = call;
        for keyword in ["IF", "PROC", "QRY", "AND", "THEN", "NOT"] {
            if let Some(rest) = call.strip_prefix(keyword)
                && rest.chars().next().is_none_or(char::is_whitespace)
            {
                position = match keyword {
                    "IF" => Some(OsirisCompletionPosition::RuleHead),
                    "PROC" | "QRY" => Some(OsirisCompletionPosition::Declaration),
                    "AND" => Some(OsirisCompletionPosition::Condition),
                    "THEN" => Some(OsirisCompletionPosition::Action),
                    "NOT" => position,
                    _ => None,
                };
                call = rest.trim_start();
                break;
            }
        }
        if call == previous {
            break;
        }
    }
    if call
        .chars()
        .any(|character| !character.is_ascii_alphanumeric() && character != '_')
    {
        return None;
    }
    let position = position.or_else(|| osiris_completion_position_before(document, line_number))?;
    Some(OsirisCompletionContext {
        prefix: call,
        goals: false,
        position,
    })
}

/// Finds the statement role established by the complete lines before the
/// cursor. Osiris commonly puts each rule keyword on its own line, so the
/// current line alone is not enough to distinguish a head from a condition
/// or action.
fn osiris_completion_position_before(
    document: &str,
    line_number: u32,
) -> Option<OsirisCompletionPosition> {
    let mut position = None;
    let mut block_comment = false;
    for line in document
        .split('\n')
        .take(usize::try_from(line_number).unwrap_or(usize::MAX))
    {
        let line = strip_osiris_comments(line, &mut block_comment);
        match line.trim() {
            "IF" => position = Some(OsirisCompletionPosition::RuleHead),
            "PROC" | "QRY" => position = Some(OsirisCompletionPosition::Declaration),
            "AND" => position = Some(OsirisCompletionPosition::Condition),
            "THEN" | "INITSECTION" | "EXITSECTION" => {
                position = Some(OsirisCompletionPosition::Action)
            }
            "KBSECTION" => position = Some(OsirisCompletionPosition::RuleHead),
            "ENDEXITSECTION" => position = None,
            _ => {}
        }
    }
    position
}

/// Tests whether one callable contract is valid in the current statement
/// role. Database facts are handled separately because they are valid as
/// reads and writes, while engine and user callables have fixed roles.
fn osiris_callable_allowed(
    kind: Option<OsirisContractKind>,
    position: OsirisCompletionPosition,
) -> bool {
    match (kind, position) {
        (_, OsirisCompletionPosition::Declaration) => false,
        (Some(OsirisContractKind::Event), OsirisCompletionPosition::RuleHead) => true,
        (
            Some(OsirisContractKind::Query | OsirisContractKind::Sysquery),
            OsirisCompletionPosition::Condition,
        ) => true,
        (
            Some(OsirisContractKind::Call | OsirisContractKind::Syscall),
            OsirisCompletionPosition::Action,
        ) => true,
        (None, _) => true,
        _ => false,
    }
}

fn osiris_contract_kind_from_packaged_role(
    role: PackagedOsirisCallableRole,
) -> Option<OsirisContractKind> {
    match role {
        PackagedOsirisCallableRole::Procedure => Some(OsirisContractKind::Call),
        PackagedOsirisCallableRole::Query => Some(OsirisContractKind::Query),
        PackagedOsirisCallableRole::Unknown => None,
    }
}

fn osiris_contract_kind_label(kind: OsirisContractKind) -> &'static str {
    match kind {
        OsirisContractKind::Call => "Osiris engine call",
        OsirisContractKind::Event => "Osiris engine event",
        OsirisContractKind::Query => "Osiris engine query",
        OsirisContractKind::Syscall => "Osiris engine syscall",
        OsirisContractKind::Sysquery => "Osiris engine sysquery",
    }
}

fn osiris_parameter_direction(direction: bg3_index::OsirisParameterDirection) -> &'static str {
    match direction {
        bg3_index::OsirisParameterDirection::In => "[in]",
        bg3_index::OsirisParameterDirection::InOut => "[inout]",
        bg3_index::OsirisParameterDirection::Out => "[out]",
    }
}

/// Tests whether the cursor follows an INIT, KB, or EXIT section marker.
fn inside_osiris_statement_section(document: &str, line_number: u32) -> bool {
    let mut inside = false;
    let mut block_comment = false;
    for line in document
        .split('\n')
        .take(usize::try_from(line_number).unwrap_or(usize::MAX))
    {
        let line = strip_osiris_comments(line, &mut block_comment);
        match line.trim() {
            "INITSECTION" | "KBSECTION" | "EXITSECTION" => inside = true,
            "ENDEXITSECTION" => inside = false,
            _ => {}
        }
    }
    inside
}

/// Tests whether one declaration can provide the requested Osiris signature.
fn osiris_signature_candidate(definition: &Definition, name: &str, database: bool) -> bool {
    definition.name == name
        && if database {
            definition.kind == OSIRIS_DATABASE_KIND
        } else {
            matches!(
                definition.kind.as_str(),
                OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND
            )
        }
}

/// Returns the stored source parameter list for one callable declaration.
fn stored_parameters(definition: &Definition) -> Vec<String> {
    definition
        .fields
        .get("Parameters")
        .map_or_else(Vec::new, |parameters| split_parameters(parameters))
}

/// Picks one declared parameter list for a loose callable.
///
/// Same-rank declarations must agree before typed metadata is exposed; a
/// disagreement falls back to positional placeholders instead of choosing an
/// arbitrary declaration.
fn osiris_declared_parameters(candidates: &[(usize, Definition)], arity: u16) -> Vec<String> {
    let Some(rank) = candidates.iter().map(|(rank, _)| *rank).max() else {
        return positional_parameters(arity);
    };
    let lists: Vec<Vec<String>> = candidates
        .iter()
        .filter(|(candidate_rank, _)| *candidate_rank == rank)
        .map(|(_, definition)| stored_parameters(definition))
        .collect();
    let Some(first) = lists.first() else {
        return positional_parameters(arity);
    };
    if lists.iter().all(|list| list == first) {
        first.clone()
    } else {
        positional_parameters(arity)
    }
}

/// Returns generic positional labels for untyped call arguments.
fn positional_parameters(count: u16) -> Vec<String> {
    (0..usize::from(count))
        .map(|index| format!("value{}", index + 1))
        .collect()
}

/// Builds a call snippet from source parameter names and known database columns.
fn osiris_call_snippet(name: &str, arity: u16, parameters: &[String]) -> String {
    let placeholders = (0..usize::from(arity))
        .map(|index| {
            let stored = parameters.get(index).map_or("unknown", String::as_str);
            let label = stored
                .split_whitespace()
                .last()
                .filter(|label| label.starts_with('_') && *label != "_" && *label != "unknown")
                .map_or_else(|| format!("column{}", index + 1), str::to_owned);
            format!("${{{}:{label}}}", index + 1)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({placeholders})")
}

/// Returns a user-facing Osiris declaration category.
fn osiris_kind_label(kind: &str) -> &'static str {
    match kind {
        OSIRIS_DATABASE_KIND => "database",
        OSIRIS_PROCEDURE_KIND => "procedure",
        OSIRIS_QUERY_KIND => "query",
        OSIRIS_GOAL_KIND => "goal",
        _ => "symbol",
    }
}

/// Adds one matching declaration to its same-rank completion group.
fn add_symbol_candidate(
    candidates: &mut BTreeMap<String, Vec<String>>,
    definition: &Definition,
    kind: &str,
    prefix: &str,
    insertion: SymbolInsertion,
) {
    if bg3_index::canonical_kind(&definition.kind) != bg3_index::canonical_kind(kind) {
        return;
    }
    let label = match insertion {
        SymbolInsertion::Name => &definition.name,
        SymbolInsertion::Uuid => definition
            .aliases
            .iter()
            .find(|alias| starts_with_case_insensitive(alias, prefix))
            .unwrap_or(&definition.name),
        SymbolInsertion::Alias => definition
            .aliases
            .iter()
            .find(|alias| starts_with_case_insensitive(alias, prefix))
            .unwrap_or(&definition.name),
    };
    if !starts_with_case_insensitive(label, prefix) {
        return;
    }
    let new_text = match insertion {
        SymbolInsertion::Name | SymbolInsertion::Alias => label.clone(),
        SymbolInsertion::Uuid => definition
            .uuid
            .map_or_else(|| label.clone(), |uuid| uuid.to_string()),
    };
    candidates.entry(label.clone()).or_default().push(new_text);
}

/// Finds a quoted type or parent prefix in incomplete syntax.
fn quoted_clause_prefix<'a>(line: &'a str, clause: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(clause)?.trim_start();
    let prefix = rest.strip_prefix('"')?;
    (!prefix.contains('"')).then_some(prefix)
}

/// Finds the field and value portions of an incomplete data clause.
fn data_context(line: &str) -> Option<DataContext<'_>> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("data")?
        .trim_start()
        .strip_prefix('"')?;
    let Some(field_end) = rest.find('"') else {
        return Some(DataContext {
            field: String::new(),
            prefix: rest,
            value_before_cursor: "",
            in_field_name: true,
        });
    };
    let field = rest[..field_end].to_owned();
    let value = rest[field_end + 1..].trim_start().strip_prefix('"')?;
    let token_start = value
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    Some(DataContext {
        field,
        prefix: &value[token_start..],
        value_before_cursor: value,
        in_field_name: false,
    })
}

/// Finds a supported legacy Stats or LSX value at the cursor.
fn value_context(line: &str) -> Option<DataContext<'_>> {
    data_context(line).or_else(|| lsx_value_context(line))
}

/// Recovers one incomplete LSX `value` attribute without parsing incomplete XML.
fn lsx_value_context(line: &str) -> Option<DataContext<'_>> {
    let element = line.rsplit_once("<attribute")?.1;
    let field = closed_xml_attribute(element, "id")?;
    if !is_lsx_value_field(field) {
        return None;
    }
    let value = open_xml_attribute(element, "value")?;
    let token_start = value
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    Some(DataContext {
        field: field.to_owned(),
        prefix: &value[token_start..],
        value_before_cursor: value,
        in_field_name: false,
    })
}

/// Returns a complete quoted XML attribute from the current start tag.
fn closed_xml_attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let value = xml_attribute_start(element, name)?;
    let quote = value.as_bytes().first().copied()?;
    let content = &value[1..];
    let end = content.bytes().position(|byte| byte == quote)?;
    Some(&content[..end])
}

/// Returns an XML attribute value only while the cursor is inside its quotes.
fn open_xml_attribute<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let value = xml_attribute_start(element, name)?;
    let quote = value.as_bytes().first().copied()?;
    let content = &value[1..];
    (!content.bytes().any(|byte| byte == quote)).then_some(content)
}

/// Finds the opening quote for one XML attribute with required name boundaries.
fn xml_attribute_start<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let bytes = element.as_bytes();
    let mut cursor = 0;
    while cursor + name.len() < bytes.len() {
        let relative = element[cursor..].find(name)?;
        let start = cursor + relative;
        let before = start.checked_sub(1).and_then(|index| bytes.get(index));
        let after = bytes.get(start + name.len());
        if before.is_none_or(|byte| byte.is_ascii_whitespace())
            && after.is_some_and(|byte| *byte == b'=')
            && bytes
                .get(start + name.len() + 1)
                .is_some_and(|byte| matches!(*byte, b'\'' | b'"'))
        {
            return element.get(start + name.len() + 1..);
        }
        cursor = start + name.len();
    }
    None
}

/// Returns the document prefix ending at an internal UTF-8 position.
///
/// Signature help scans from the start of an Osiris document because calls may
/// span lines.  The public LSP layer converts its negotiated UTF-16 position
/// to this byte-oriented position before calling the IDE layer.
fn source_prefix(source: &str, position: Position) -> Option<&str> {
    let requested_line = usize::try_from(position.line).ok()?;
    let mut line = 0;
    let mut line_start = 0;
    for (index, byte) in source.bytes().enumerate() {
        if line == requested_line {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    if line != requested_line {
        return None;
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |offset| line_start + offset);
    let current_line = &source[line_start..line_end];
    let requested_column = usize::try_from(position.character)
        .unwrap_or(usize::MAX)
        .min(current_line.len());
    let byte_column = current_line
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(current_line.len()))
        .take_while(|index| *index <= requested_column)
        .last()
        .unwrap_or(0);
    Some(&source[..line_start + byte_column])
}

/// Finds the innermost incomplete function call and active argument.
fn call_context(value: &str) -> Option<CallContext> {
    let bytes = value.as_bytes();
    let mut stack = Vec::<CallParen>::new();
    let mut quote = None;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut previous_identifier = None;
    let mut cursor = 0;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            cursor += 1;
            continue;
        }
        if block_comment {
            if byte == b'*' && bytes.get(cursor + 1) == Some(&b'/') {
                block_comment = false;
                cursor += 2;
            } else {
                cursor += 1;
            }
            continue;
        }
        if let Some((active, escaped)) = quote {
            if escaped {
                quote = Some((active, false));
            } else if byte == b'\\' {
                quote = Some((active, true));
            } else if byte == active {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        match byte {
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                line_comment = true;
                cursor += 2;
                continue;
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                block_comment = true;
                cursor += 2;
                continue;
            }
            b'\'' | b'"' => {
                previous_identifier = None;
                quote = Some((byte, false));
            }
            b'(' => {
                if let Some((start, end)) = previous_identifier.take() {
                    stack.push(CallParen::Call {
                        function: value[start..end].to_owned(),
                        argument: 0,
                        arguments_start: cursor + 1,
                    });
                } else {
                    stack.push(CallParen::Group);
                }
            }
            b',' => {
                if let Some(CallParen::Call { argument, .. }) = stack.last_mut() {
                    *argument += 1;
                }
                previous_identifier = None;
            }
            b')' => {
                stack.pop();
                previous_identifier = None;
            }
            byte if byte.is_ascii_alphanumeric() || byte == b'_' => {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len()
                    && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
                {
                    cursor += 1;
                }
                previous_identifier = Some((start, cursor));
                continue;
            }
            byte if byte.is_ascii_whitespace() => {}
            _ => {
                previous_identifier = None;
            }
        }
        cursor += 1;
    }
    if line_comment || block_comment {
        return None;
    }
    stack.into_iter().rev().find_map(|frame| match frame {
        CallParen::Call {
            function,
            argument,
            arguments_start,
        } => Some(CallContext {
            function,
            argument,
            first_argument: first_call_argument(&value[arguments_start..]),
        }),
        CallParen::Group => None,
    })
}

/// Extracts the first top-level argument from an incomplete call.
fn first_call_argument(arguments: &str) -> Option<String> {
    let bytes = arguments.as_bytes();
    let mut depth = 0_usize;
    let mut quote = None;
    let mut line_comment = false;
    let mut block_comment = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                block_comment = false;
            }
            continue;
        }
        if let Some((active, escaped)) = quote {
            if escaped {
                quote = Some((active, false));
            } else if byte == b'\\' {
                quote = Some((active, true));
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                line_comment = true;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                block_comment = true;
            }
            b'\'' | b'"' => quote = Some((byte, false)),
            b'(' => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b',' | b')' if depth == 0 => {
                let value = arguments[..index].trim();
                return (!value.is_empty()).then(|| value.to_owned());
            }
            _ => {}
        }
    }
    let value = arguments.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Returns the last declaration that starts before the cursor and still contains it.
fn active_definition(definitions: &[Definition], position: Position) -> Option<&Definition> {
    definitions.iter().rev().find(|definition| {
        range_contains(definition.range, position) || definition.range.start.line <= position.line
    })
}

/// Returns schema candidates for exact Toolkit IDs or inferred legacy entries.
fn schemas_for_definition<'a>(
    workspace: &'a WorkspaceSnapshot,
    path: &Path,
    definition: &Definition,
) -> Vec<&'a SchemaDefinition> {
    if let Some(schema_id) = &definition.schema_id {
        workspace.schema.by_id.get(schema_id).into_iter().collect()
    } else {
        workspace
            .schema
            .infer_legacy(path, Some(&definition.kind), &definition.fields)
    }
}

/// Creates one replacement edit for the current token prefix.
fn basic_item(
    label: &str,
    prefix: &str,
    position: Position,
    kind: CompletionKind,
) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        detail: None,
        documentation: None,
        new_text: label.into(),
        sort_text: None,
        range: TextRange {
            start: Position {
                line: position.line,
                character: position
                    .character
                    .saturating_sub(u32::try_from(prefix.len()).unwrap_or(u32::MAX)),
            },
            end: position,
        },
        kind,
        snippet: false,
    }
}

/// Returns one zero-based source line without allocating.
fn source_line(source: &str, line: u32) -> Option<&str> {
    source
        .split('\n')
        .nth(usize::try_from(line).ok()?)
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// Tests whether the text before the cursor is inside an Osiris block
/// comment. Completion must not treat comment contents as structural markers.
fn osiris_prefix_is_in_comment(document: &str, line_number: u32, before: &str) -> bool {
    let mut block_comment = false;
    for line in document
        .split('\n')
        .take(usize::try_from(line_number).unwrap_or(usize::MAX))
    {
        strip_osiris_comments(line, &mut block_comment);
    }
    strip_osiris_comments(before, &mut block_comment);
    block_comment
}

/// Removes Osiris line and block comments while retaining code on the same
/// line. The state is carried across lines for the block-comment form.
fn strip_osiris_comments(line: &str, block_comment: &mut bool) -> String {
    let mut code = String::with_capacity(line.len());
    let mut characters = line.chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    while let Some(character) = characters.next() {
        if *block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                *block_comment = false;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            code.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        if character == '"' {
            quote = Some(character);
            code.push(character);
            continue;
        }
        if character == '/' {
            match characters.peek() {
                Some('/') => break,
                Some('*') => {
                    characters.next();
                    *block_comment = true;
                    continue;
                }
                _ => {}
            }
        }
        code.push(character);
    }
    code
}

/// Returns the incomplete identifier immediately before the cursor.
fn identifier_prefix(source: &str) -> &str {
    let start = source
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    &source[start..]
}

/// Performs an ASCII-insensitive prefix comparison used for BG3 identifiers.
fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns the effective priority and ambiguity for one complete package entry.
///
/// Fact extraction can reject a source. Resolution must still use the source
/// catalog, so a rejected higher-priority entry suppresses lower-priority
/// facts while equal-priority ambiguity remains visible.
fn packaged_thoth_entry_priority(
    catalog: &PackagedThothCatalog,
    module: &str,
    entry: &str,
) -> Option<(u8, bool)> {
    let candidates = catalog.sources_for(module, entry);
    let priority = candidates.first()?.priority();
    let ambiguous = candidates
        .iter()
        .take_while(|candidate| candidate.priority() == priority)
        .count()
        > 1;
    Some((priority, ambiguous))
}

/// Returns the field name and value spans of a `data` clause when the cursor
/// sits inside the quoted property name.
///
/// Legacy Stats clauses are single lines, so scanning one line is complete.
/// The closing quote may be absent while the name is still being typed.
fn data_clause_spans(line: &str, column: usize) -> Option<(&str, Option<&str>)> {
    let trimmed_start = line.len() - line.trim_start().len();
    if !line[trimmed_start..].starts_with("data") {
        return None;
    }
    let keyword_end = trimmed_start + "data".len();
    if !line[keyword_end..]
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_whitespace())
    {
        return None;
    }
    let open = line.find('"')?;
    let close = line[open + 1..]
        .find('"')
        .map_or(line.len(), |index| open + 1 + index);
    if column <= open || column > close {
        return None;
    }
    let name = &line[open + 1..close];
    let value = line[close + 1..].trim_start();
    let value = value.strip_prefix('"').map(|rest| match rest.find('"') {
        Some(end) => &rest[..end],
        None => rest,
    });
    Some((name, value))
}

/// Formats one Stats value as preview lines by splitting on top-level `;`.
fn format_value_preview(value: &str) -> String {
    let mut statements = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut statement_start = 0;
    for (index, character) in value.char_indices() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == open {
                    quote = None;
                }
            }
            None => match character {
                '\'' | '"' => quote = Some(character),
                ';' => {
                    statements.push(value[statement_start..index].trim());
                    statement_start = index + 1;
                }
                _ => {}
            },
        }
    }
    statements.push(value[statement_start..].trim());
    statements
        .iter()
        .filter(|statement| !statement.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// The member object before the cursor and whether the cursor's identifier
/// itself is in object position.
struct MemberWord<'a> {
    object: Option<&'a str>,
    is_object_position: bool,
}

/// Splits the identifier under the cursor and reads the `Object.` prefix.
///
/// The object is only recognized when the dot sits directly before the word,
/// and the object position only when the dot sits directly after it, so
/// ordinary identifiers stay unaffected.
fn member_word_context(line: &str, column: usize) -> Option<MemberWord<'_>> {
    let bytes = line.as_bytes();
    let mut start = column.min(bytes.len());
    let mut end = start;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    if start >= end {
        return None;
    }
    let object = if start >= 2 && bytes[start - 1] == b'.' {
        let mut begin = start - 1;
        while begin > 0 && (bytes[begin - 1].is_ascii_alphanumeric() || bytes[begin - 1] == b'_') {
            begin -= 1;
        }
        (begin < start - 1).then_some(&line[begin..start - 1])
    } else {
        None
    };
    let is_object_position = end < bytes.len() && bytes[end] == b'.';
    Some(MemberWord {
        object,
        is_object_position,
    })
}

/// Returns the member object before the cursor when a Stats value ends with
/// `Object.Partial`, tolerating a closing quote and any partial identifier.
fn member_object_before_partial(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    let mut end = bytes.len();
    while end > 0
        && (bytes[end - 1] == b'"'
            || bytes[end - 1] == b'\''
            || bytes[end - 1].is_ascii_alphanumeric()
            || bytes[end - 1] == b'_')
    {
        end -= 1;
    }
    if end == 0 || !value[..end].ends_with('.') {
        return None;
    }
    let head = &value[..end - 1];
    let start = head
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    (start < head.len()).then_some(&head[start..])
}

/// Returns the identifier under a byte column.
fn word_at(line: &str, column: usize) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut start = column.min(bytes.len());
    let mut end = start;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    (start < end).then_some(&line[start..end])
}

/// Returns the ranking key that keeps curated vocabulary ahead of observed
/// evidence in capped completion lists.
fn curated_sort_text(name: &str) -> String {
    format!("0{name}")
}

/// Returns whether only whitespace precedes the cursor since the last
/// top-level functor statement boundary, so an execution-position prefix can
/// be completed.
///
/// The scan tracks quotes because quoted localization handles and status names
/// may contain semicolons that do not separate functor statements.
fn is_functor_statement_head(head: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut tail_start = 0;
    for (index, character) in head.char_indices() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == open {
                    quote = None;
                }
            }
            None => match character {
                '\'' | '"' => quote = Some(character),
                ';' => tail_start = index + 1,
                _ => {}
            },
        }
    }
    head[tail_start..]
        .chars()
        .all(|character| character.is_ascii_whitespace())
}
#[cfg(test)]
mod tests {
    use super::{call_context, source_prefix};
    use bg3_index::Position;

    #[test]
    fn call_context_tracks_multiline_arguments_and_lexical_noise() {
        let value = concat!(
            "UsingSpell(\n",
            "    /* FakeCall(\"ignored, comma\") */ (CHARACTER)_Caster,\n",
            "    \"Target, (not a call)\" /* comma, and FakeCall(1, 2) */,\n",
        );
        let context = call_context(value).expect("open call");
        assert_eq!(context.function, "UsingSpell");
        assert_eq!(context.argument, 2);
    }

    #[test]
    fn call_context_does_not_offer_help_inside_comments() {
        assert!(call_context("UsingSpell( // FakeCall(1, 2)").is_none());
        assert!(call_context("UsingSpell(/* FakeCall(1, 2)").is_none());
    }

    #[test]
    fn call_context_skips_comments_between_name_and_parenthesis() {
        for value in [
            "UsingSpell /* block comment */ (",
            "UsingSpell // line comment\n(",
        ] {
            let context = call_context(value).expect("call after comment");
            assert_eq!(context.function, "UsingSpell");
            assert_eq!(context.argument, 0);
        }
    }

    #[test]
    fn call_context_ignores_escaped_quotes_and_nested_groups() {
        let value = "UsingSpell(((CHARACTER)_Caster), \"Target\\\"Name, (not a call)\"";
        let context = call_context(value).expect("open call");
        assert!(matches!(
            context,
            super::CallContext {
                function,
                argument: 1,
                ..
            } if function == "UsingSpell"
        ));

        let context = call_context("UsingSpell(Inner(1, 2), ").expect("outer call");
        assert!(matches!(
            context,
            super::CallContext {
                function,
                argument: 1,
                ..
            } if function == "UsingSpell"
        ));
    }

    #[test]
    fn source_prefix_preserves_document_context_at_utf8_position() {
        let source = "prefix 😄\nUsingSpell(\n  _Caster,\n";
        let prefix = source_prefix(
            source,
            Position {
                line: 2,
                character: 10,
            },
        )
        .expect("line prefix");
        assert_eq!(prefix, "prefix 😄\nUsingSpell(\n  _Caster,");
        assert!(matches!(
            call_context(prefix),
            Some(super::CallContext { argument: 1, .. })
        ));
    }
}
#[cfg(test)]
mod osiris_comment_tests {
    use super::strip_osiris_comments;

    #[test]
    fn osiris_comment_markers_inside_escaped_strings_are_preserved() {
        let mut block_comment = false;
        let line = r#"Call("url // /* escaped \" quote"); // trailing"#;
        assert_eq!(
            strip_osiris_comments(line, &mut block_comment),
            r#"Call("url // /* escaped \" quote"); "#
        );
        assert!(!block_comment);
    }

    #[test]
    fn osiris_block_comment_state_ignores_string_markers() {
        let mut block_comment = false;
        let line = r#"Call("/* escaped \" still string */"); /* block"#;
        assert_eq!(
            strip_osiris_comments(line, &mut block_comment),
            r#"Call("/* escaped \" still string */"); "#
        );
        assert!(block_comment);

        assert_eq!(
            strip_osiris_comments("comment */ THEN", &mut block_comment),
            " THEN"
        );
        assert!(!block_comment);
    }
}
