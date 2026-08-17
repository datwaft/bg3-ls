use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{OverlaySet, WorkspaceSnapshot, range_contains};
use bg3_index::{
    Definition, FUNCTIONS, ModuleRole, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND,
    OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND, PackagedThothCatalog, Position, SchemaDefinition,
    SchemaField, SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAnnotations,
    ThothFunctionContract, field_kind, function_spec, is_lsx_value_field,
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
}

impl AnnotatedSignature {
    fn label(&self, name: &str) -> String {
        let mut label = format!("{name}({})", self.parameters.join(", "));
        if !self.returns.is_empty() {
            label.push_str(": ");
            label.push_str(&self.returns.join(", "));
        }
        label
    }

    fn documentation(&self, prefix: &str) -> String {
        if self.returns.is_empty() {
            prefix.to_owned()
        } else {
            format!("{prefix}\n\nReturns: `{}`", self.returns.join(", "))
        }
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
            .map(|field| format!("- `{}`: `{}`", field.name, field.ty))
            .collect::<Vec<_>>();
        let mut markdown = format!("**Thoth class** `{word}`");
        if !fields.is_empty() {
            markdown.push_str("\n\n");
            markdown.push_str(&fields.join("\n"));
        }
        return Some(markdown);
    }
    if let Some((class, field)) = annotations.classes.iter().find_map(|class| {
        class
            .fields
            .iter()
            .find(|field| field.name == word && range_contains(field.name_range, position))
            .map(|field| (class, field))
    }) {
        return Some(format!(
            "**Thoth field** `{word}`\n\nClass: `{}`\n\nType: `{}`",
            class.name, field.ty
        ));
    }
    if let Some(alias) = annotations
        .aliases
        .iter()
        .find(|alias| alias.name == word && range_contains(alias.name_range, position))
    {
        return Some(format!("**Thoth alias** `{word}`\n\nType: `{}`", alias.ty));
    }
    annotations
        .variables
        .iter()
        .find(|variable| variable.target == word && range_contains(variable.target_range, position))
        .map(|variable| format!("**Thoth type** `{word}`\n\nType: `{}`", variable.ty))
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
            let mut items = self.complete_thoth_functions(prefix, position, overlays, snippets);
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
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase())
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
        let line = source_line(text, position.line)?;
        let cursor = usize::try_from(position.character).ok()?.min(line.len());
        let before = &line[..cursor];
        if file.source.kind == SourceKind::Osiris {
            return self.osiris_signature_help(before, overlays);
        }
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
            return Some(SignatureHelp {
                label: signature.label(&context.function),
                documentation: signature.documentation("Explicit Thoth annotation."),
                parameters: signature.parameters,
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
            let mut documentation = signature.documentation("Explicit installed Thoth annotation.");
            documentation.push_str(&format!("\n\nModule: `{module}`"));
            if !entries.is_empty() {
                documentation.push_str(&format!("\n\nPackage entries: `{}`", entries.join("`, `")));
            }
            return Some(SignatureHelp {
                label: signature.label(&context.function),
                documentation,
                parameters: signature.parameters,
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
    pub fn language_hover(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let text = overlays.get(path)?.text.as_str();
        let line = source_line(text, position.line)?;
        let word = word_at(line, usize::try_from(position.character).ok()?)?;
        if let Some(function) = function_spec(word) {
            return Some(format!(
                "**Function** `{}`\n\n{}",
                function.name, function.documentation
            ));
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
                    return Some(format!(
                        "**Enum value** `{word}`\n\nEnumeration: `{enumeration}`\n\nField: `{}`",
                        data.field
                    ));
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
            return Some(format!(
                "**Thoth function** `{word}`\n\nSignature: `{}`\n\n{}",
                signature.label(word),
                signature.documentation("Explicit Thoth annotation.")
            ));
        }
        if let Some(evidence) = self.loose_thoth_hover(word, overlays) {
            return Some(evidence);
        }
        if let Some((signature, module, entries)) = self.packaged_thoth_annotation(word) {
            let mut markdown = format!(
                "**Installed Thoth function** `{word}`\n\nModule: `{module}`\n\nSignature: `{}`\n\n{}",
                signature.label(word),
                signature.documentation("Explicit installed Thoth annotation.")
            );
            if !entries.is_empty() {
                markdown.push_str(&format!("\n\nPackage entries: `{}`", entries.join("`, `")));
            }
            return Some(markdown);
        }
        if let Some(evidence) = self.packaged_thoth_function_evidence(word) {
            return Some(evidence);
        }
        None
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

        if let Some(call) = call_context(value_before_cursor)
            && let Some(function) = function_spec(&call.function)
            && let Some(kind) = function.parameter_kind(
                call.argument,
                call.argument + 1,
                call.first_argument.as_deref(),
            )
        {
            return self.complete_symbols(kind, prefix, position, overlays);
        }
        if let Some(kind) = field_kind(field_name) {
            return self.complete_symbols(kind, prefix, position, overlays);
        }

        let mut items = Vec::new();
        for function in FUNCTIONS {
            if starts_with_case_insensitive(function.name, prefix) {
                let mut item =
                    basic_item(function.name, prefix, position, CompletionKind::Function);
                item.documentation = Some(function.documentation.into());
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
                for parameters in parameter_lists {
                    let mut item = basic_item(&name, prefix, position, CompletionKind::Function);
                    item.detail = Some(if let Some(signature) = &annotation {
                        signature.label(&name)
                    } else if ambiguous {
                        format!("{} (same-rank ambiguity)", layer.spec.name)
                    } else {
                        layer.spec.name.clone()
                    });
                    item.documentation = Some(if annotation.is_some() {
                        "Explicit Thoth annotation.".into()
                    } else {
                        "Declared Thoth helper. Parameter types are not inferred.".into()
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
                    if let Some((signature, installed_module, _)) = &annotation {
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
                item.documentation = Some(
                    if let Some((_, installed_module, entries)) = &annotation {
                        let mut documentation = format!(
                            "Explicit installed Thoth annotation from module {}.",
                            installed_module
                        );
                        if !entries.is_empty() {
                            documentation
                                .push_str(&format!(" Package entries: {}.", entries.join(", ")));
                        }
                        documentation
                    } else {
                        "Installed Thoth declaration from configured package data. Parameter types are not inferred."
                        .into()
                    },
                );
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
        items
    }

    /// Completes visible user declarations only at Osiris call or parent-goal positions.
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
            let mut candidates = BTreeMap::<(String, String, u16), Definition>::new();
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
                if overlays.contains(record.path.as_ref()) {
                    continue;
                }
                add_osiris_completion_candidate(
                    &mut candidates,
                    record.definition(),
                    context.prefix,
                    context.goals,
                );
            }
            for ((namespace, name, arity), definition) in candidates {
                if !seen.insert((namespace, name.clone(), arity)) {
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

    /// Resolves explicit contracts from the effective packaged declarations.
    /// Package entries remain provenance labels; they are never converted to
    /// navigable filesystem locations.
    fn packaged_thoth_annotation(
        &self,
        name: &str,
    ) -> Option<(AnnotatedSignature, String, Vec<String>)> {
        let catalog = self.packaged_thoth();
        let facts = self.packaged_thoth_facts();
        for layer in self
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
        {
            let mut records = Vec::new();
            for record in facts.iter() {
                if record.source().module() != layer.spec.name {
                    continue;
                }
                let Some((priority, _)) = packaged_thoth_entry_priority(
                    &catalog,
                    record.source().module(),
                    record.source().entry(),
                ) else {
                    continue;
                };
                if record.source().priority() != priority
                    || !catalog
                        .sources_for(record.source().module(), record.source().entry())
                        .iter()
                        .any(|candidate| candidate == record.source())
                {
                    continue;
                }
                if record
                    .facts()
                    .declarations
                    .iter()
                    .any(|declaration| declaration.name == name)
                {
                    records.push(record);
                }
            }
            if records.is_empty() {
                continue;
            }
            let signatures = records
                .iter()
                .flat_map(|record| {
                    record
                        .facts()
                        .declarations
                        .iter()
                        .filter(|declaration| declaration.name == name)
                        .map(|declaration| {
                            function_annotation_at(
                                &record.facts().annotations,
                                name,
                                declaration.name_range,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let Some(Some(signature)) = signatures.first() else {
                return None;
            };
            if !signatures
                .iter()
                .all(|candidate| candidate.as_ref() == Some(signature))
            {
                return None;
            }
            let mut entries = records
                .iter()
                .map(|record| record.source().entry().to_owned())
                .collect::<Vec<_>>();
            entries.sort();
            entries.dedup();
            return Some((signature.clone(), layer.spec.name.clone(), entries));
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
        let mut markdown = format!("**Installed Thoth function** `{name}`\n\nModule: `{module}`");
        if !parameters.is_empty() {
            markdown.push_str(&format!(
                "\n\nSignature evidence: `{}({})`",
                name,
                parameters.join(", ")
            ));
        }
        if !calls.is_empty() {
            let arities = calls
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            markdown.push_str(&format!("\n\nObserved call arities: `{arities}`"));
        }
        if ambiguous {
            markdown.push_str("\n\nSame-priority package evidence is ambiguous.");
        }
        if !entries.is_empty() {
            markdown.push_str("\n\nPackage entries: ");
            markdown.push_str(
                &entries
                    .into_iter()
                    .map(|entry| format!("`{entry}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        Some(markdown)
    }

    /// Resolves loose Thoth evidence by configured module precedence.
    fn loose_thoth_hover(&self, name: &str, overlays: &OverlaySet) -> Option<String> {
        for layer in self.layers.iter().rev() {
            for (_, overlay) in overlays.for_module(&layer.spec.name) {
                if let Some(definition) = overlay.parsed.definitions.iter().find(|definition| {
                    definition.kind == THOTH_FUNCTION_KIND && definition.name == name
                }) {
                    let parameters = thoth_parameters(definition);
                    return Some(format!(
                        "**Thoth function** `{name}`\n\nModule: `{}`\n\nSignature: `{}({})`",
                        layer.spec.name,
                        name,
                        parameters.join(", ")
                    ));
                }
            }
            if let Some(definition) =
                layer
                    .definitions_of_kind(THOTH_FUNCTION_KIND)
                    .find(|record| {
                        !overlays.contains(record.path.as_ref()) && record.definition().name == name
                    })
            {
                let definition = definition.definition();
                let parameters = thoth_parameters(definition);
                return Some(format!(
                    "**Thoth function** `{name}`\n\nModule: `{}`\n\nSignature: `{}({})`",
                    layer.spec.name,
                    name,
                    parameters.join(", ")
                ));
            }
        }
        for layer in self.layers.iter().rev() {
            if let Some(function) = layer.functions.get(name) {
                return Some(format!(
                    "**Observed function** `{}`\n\nSeen {} times with {} to {} arguments. No verified signature is available.",
                    function.name, function.count, function.min_arity, function.max_arity
                ));
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
        Some(format!(
            "**Thoth function** `{name}`\n\nSignature: `{}`\n\n{}",
            signature.label(name),
            signature.documentation("Explicit Thoth annotation.")
        ))
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
        let arity = candidates
            .iter()
            .filter_map(|(_, definition)| definition.arity)
            .filter(|arity| *arity == 0 || usize::from(*arity) > context.argument)
            .min()?;
        candidates.retain(|(_, definition)| definition.arity == Some(arity));
        let parameters = if database {
            merge_osiris_database_parameters(
                candidates
                    .iter()
                    .map(|(_, definition)| stored_parameters(definition)),
                arity,
            )
        } else {
            let rank = candidates.iter().map(|(rank, _)| *rank).max()?;
            candidates
                .iter()
                .find(|(candidate_rank, _)| *candidate_rank == rank)
                .map(|(_, definition)| stored_parameters(definition))?
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

#[derive(Clone, Copy, Debug)]
struct OsirisCompletionContext<'a> {
    prefix: &'a str,
    goals: bool,
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
    candidates: &mut BTreeMap<(String, String, u16), Definition>,
    definition: &Definition,
    prefix: &str,
    goals: bool,
) {
    if goals {
        if definition.kind == OSIRIS_GOAL_KIND
            && starts_with_case_insensitive(&definition.name, prefix)
        {
            candidates.insert(
                ("goal".into(), definition.name.clone(), 0),
                definition.clone(),
            );
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
    candidates.insert(
        (namespace.into(), definition.name.clone(), arity),
        definition.clone(),
    );
}

/// Selects one incomplete Osiris call name or parent-goal string.
fn osiris_completion_context<'a>(
    document: &str,
    line: &'a str,
    line_number: u32,
) -> Option<OsirisCompletionContext<'a>> {
    let trimmed = line.trim_start();
    if let Some(prefix) = trimmed.strip_prefix("ParentTargetEdge") {
        let prefix = prefix.trim_start().strip_prefix('"')?;
        return (!prefix.contains('"')).then_some(OsirisCompletionContext {
            prefix,
            goals: true,
        });
    }
    if !inside_osiris_statement_section(document, line_number) {
        return None;
    }

    let mut call = trimmed;
    loop {
        let previous = call;
        for keyword in ["IF", "PROC", "QRY", "AND", "THEN", "NOT"] {
            if let Some(rest) = call.strip_prefix(keyword)
                && rest.chars().next().is_none_or(char::is_whitespace)
            {
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
    Some(OsirisCompletionContext {
        prefix: call,
        goals: false,
    })
}

/// Tests whether the cursor follows an INIT, KB, or EXIT section marker.
fn inside_osiris_statement_section(document: &str, line_number: u32) -> bool {
    let mut inside = false;
    for line in document
        .lines()
        .take(usize::try_from(line_number).unwrap_or(usize::MAX))
    {
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

/// Merges database column evidence without selecting among incompatible aliases.
fn merge_osiris_database_parameters(
    parameter_lists: impl IntoIterator<Item = Vec<String>>,
    arity: u16,
) -> Vec<String> {
    let mut columns = vec![BTreeSet::new(); usize::from(arity)];
    for parameters in parameter_lists {
        for (index, parameter) in parameters.into_iter().enumerate() {
            if parameter != "unknown"
                && let Some(column) = columns.get_mut(index)
            {
                column.insert(parameter);
            }
        }
    }
    columns
        .into_iter()
        .map(|types| {
            if types.len() == 1 {
                types.into_iter().next().unwrap_or_else(|| "unknown".into())
            } else if types.len() > 1 {
                "conflicting".into()
            } else {
                "unknown".into()
            }
        })
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

/// Finds the innermost incomplete function call and active argument.
fn call_context(value: &str) -> Option<CallContext> {
    let bytes = value.as_bytes();
    let mut stack = Vec::<(String, usize, usize)>::new();
    let mut quote = None;
    let mut cursor = 0;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(active) = quote {
            if byte == active && bytes.get(cursor.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => {
                let mut end = cursor;
                while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                    end -= 1;
                }
                let mut start = end;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                if start < end {
                    stack.push((value[start..end].to_owned(), 0, cursor + 1));
                }
            }
            b',' => {
                if let Some((_, argument, _)) = stack.last_mut() {
                    *argument += 1;
                }
            }
            b')' => {
                stack.pop();
            }
            _ => {}
        }
        cursor += 1;
    }
    stack
        .pop()
        .map(|(function, argument, arguments_start)| CallContext {
            function,
            argument,
            first_argument: first_call_argument(&value[arguments_start..]),
        })
}

/// Extracts the first top-level argument from an incomplete call.
fn first_call_argument(arguments: &str) -> Option<String> {
    let bytes = arguments.as_bytes();
    let mut depth = 0_u16;
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if let Some(active) = quote {
            if byte == active && bytes.get(index.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
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
    source.lines().nth(usize::try_from(line).ok()?)
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
