use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bg3_index::{Definition, Position, SchemaDefinition, SchemaField, SymbolTarget, TextRange};

use crate::catalog::{FUNCTIONS, field_kind, function_spec};
use crate::{OverlaySet, WorkspaceSnapshot, range_contains};

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
        let entry = active_definition(&file.definitions, position);
        let mut items = if let Some(prefix) = quoted_clause_prefix(before, "type") {
            self.complete_entry_types(prefix, position)
        } else if let Some(prefix) = quoted_clause_prefix(before, "using") {
            entry.map_or_else(Vec::new, |entry| {
                self.complete_symbols(&entry.kind, prefix, position, overlays)
            })
        } else if let Some(data) = data_context(before) {
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

    /// Returns signature help only for curated functions with verified parameters.
    pub fn signature_help(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<SignatureHelp> {
        let text = overlays.get(path)?.text.as_str();
        let line = source_line(text, position.line)?;
        let cursor = usize::try_from(position.character).ok()?.min(line.len());
        let before = &line[..cursor];
        // A Stats value starts with an unmatched document quote while the user edits it.
        // Remove the data-clause prefix before balancing expression quotes and calls.
        let expression = data_context(before)
            .map(|context| context.value_before_cursor)
            .unwrap_or(before);
        let context = call_context(expression)?;
        let function = function_spec(&context.function)?;
        let mut label = format!("{}(", function.name);
        label.push_str(
            &function
                .parameters
                .iter()
                .map(|parameter| parameter.label)
                .collect::<Vec<_>>()
                .join(", "),
        );
        if function.variadic {
            if !function.parameters.is_empty() {
                label.push_str(", ");
            }
            label.push_str("...");
        }
        label.push(')');
        Some(SignatureHelp {
            label,
            documentation: function.documentation.into(),
            parameters: function
                .parameters
                .iter()
                .map(|parameter| parameter.label.into())
                .collect(),
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
        for layer in &self.layers {
            if let Some(function) = layer.functions.get(word) {
                return Some(format!(
                    "**Observed function** `{}`\n\nSeen {} times with {} to {} arguments. No verified signature is available.",
                    function.name, function.count, function.min_arity, function.max_arity
                ));
            }
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
            && let Some(kind) = function
                .parameters
                .get(call.argument)
                .and_then(|parameter| parameter.kind)
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
                if snippets && !function.parameters.is_empty() {
                    let parameters = function
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
}

#[derive(Clone, Copy, Debug)]
enum SymbolInsertion {
    Name,
    Alias,
    Uuid,
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

/// Finds the innermost incomplete function call and active argument.
fn call_context(value: &str) -> Option<CallContext> {
    let bytes = value.as_bytes();
    let mut stack = Vec::<(String, usize)>::new();
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
                    stack.push((value[start..end].to_owned(), 0));
                }
            }
            b',' => {
                if let Some((_, argument)) = stack.last_mut() {
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
        .map(|(function, argument)| CallContext { function, argument })
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
        workspace.schema.infer(path, Some(&definition.kind))
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

/// Performs an ASCII-insensitive prefix comparison used for BG3 identifiers.
fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
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
