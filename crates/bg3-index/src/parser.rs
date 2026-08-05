use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use tree_sitter::{Node, Parser};
use uuid::Uuid;

use crate::Error;
use crate::domain::{
    Definition, LineMap, ObservedFunction, ParsedFile, Position, Reference, SourceFile,
    SourceIssue, SourceKind, SymbolTarget, TextRange,
};
use crate::schema::{SchemaCatalog, SchemaDefinition};
use crate::xml::{attribute_range, attributes};

const NAME_FIELDS: [&str; 5] = ["Name", "NameFS", "FSName", "RaceName", "TechnicalName"];

/// Parses one supported source file into a cacheable semantic record.
pub fn parse_source(
    source: SourceFile,
    text: &str,
    schema: &SchemaCatalog,
    language: &str,
) -> Result<ParsedFile, Error> {
    match source.kind {
        SourceKind::PlainStats => parse_plain(source, text, schema),
        SourceKind::ToolkitStats | SourceKind::Table => parse_toolkit(source, text, schema),
        SourceKind::Lsx => parse_lsx(source, text),
        SourceKind::Localization => parse_localization(source, text, language),
    }
}

/// Parses legacy plain-text Stats with the generated Tree-sitter grammar.
fn parse_plain(
    source: SourceFile,
    text: &str,
    schema: &SchemaCatalog,
) -> Result<ParsedFile, Error> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bg3::BG3_STATS_LANGUAGE.into())?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| Error::Parse("the Stats parser returned no tree".into()))?;
    let root = tree.root_node();
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    let mut issues = Vec::new();
    collect_syntax_issues(root, &mut issues);

    let mut value_parser = Parser::new();
    value_parser.set_language(&tree_sitter_bg3::BG3_STATS_VALUE_LANGUAGE.into())?;
    let mut observed = HashMap::<String, ObservedFunction>::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "stat_entry" => {
                let mut entry = parse_stat_entry(node, text, &mut value_parser)?;
                apply_schema_reference_kinds(
                    &source.path,
                    &entry.definition,
                    &mut entry.references,
                    schema,
                );
                definitions.push(entry.definition);
                references.append(&mut entry.references);
                merge_functions(&mut observed, entry.functions);
                issues.append(&mut entry.issues);
            }
            "treasure_table" | "equipment_entry" | "named_block" => {
                if let Some(definition) = parse_named_block(node, text) {
                    definitions.push(definition);
                }
            }
            _ => {}
        }
    }

    Ok(ParsedFile {
        source,
        definitions,
        references,
        observed_functions: sorted_functions(observed),
        issues,
    })
}

/// Intermediate products that belong to one legacy Stats entry.
struct ParsedStatEntry {
    definition: Definition,
    references: Vec<Reference>,
    functions: Vec<ObservedFunction>,
    issues: Vec<SourceIssue>,
}

/// Extracts one standard Stats entry and its typed references.
fn parse_stat_entry(
    node: Node<'_>,
    text: &str,
    value_parser: &mut Parser,
) -> Result<ParsedStatEntry, Error> {
    let header = direct_child(node, "entry_header")
        .ok_or_else(|| Error::Parse("a Stats entry has no header".into()))?;
    let name_node = field(header, "name")
        .ok_or_else(|| Error::Parse("a Stats entry header has no name".into()))?;
    let (name, selection_range) = quoted(name_node, text);
    let mut kind = None;
    let mut parent = None;
    let mut fields = BTreeMap::new();
    let mut field_ranges = BTreeMap::new();
    let mut references = Vec::new();
    let mut functions = Vec::new();
    let mut issues = Vec::new();

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "type_clause" => {
                kind = field(child, "value").map(|value| quoted(value, text).0);
            }
            "using_clause" => {
                if let Some(value) = field(child, "value") {
                    let (using, range) = quoted(value, text);
                    parent = Some(using.clone());
                    references.push(Reference {
                        target: SymbolTarget::Named {
                            kind: kind.clone(),
                            name: using,
                        },
                        range,
                        context: "using".into(),
                    });
                }
            }
            "data_clause" => {
                let Some(name_node) = field(child, "name") else {
                    continue;
                };
                let Some(value_node) = field(child, "value") else {
                    continue;
                };
                let (field_name, field_range) = quoted(name_node, text);
                let (value, value_range) = quoted(value_node, text);
                if fields.contains_key(&field_name) {
                    issues.push(SourceIssue {
                        code: "duplicate-field".into(),
                        message: format!("Field `{field_name}` occurs more than once."),
                        range: field_range,
                    });
                }
                fields.insert(field_name.clone(), value.clone());
                field_ranges.insert(field_name.clone(), field_range);
                let (mut value_references, value_functions) =
                    parse_value(value_parser, &value, value_range, &field_name)?;
                references.append(&mut value_references);
                functions.extend(value_functions);
            }
            _ => {}
        }
    }

    let uuid = fields
        .get("UUID")
        .and_then(|value| Uuid::parse_str(value).ok());
    Ok(ParsedStatEntry {
        definition: Definition {
            kind: kind.unwrap_or_else(|| "StatEntry".into()),
            name,
            range: node_range(node),
            selection_range,
            fields,
            field_ranges,
            aliases: Vec::new(),
            uuid,
            parent,
            schema_id: None,
        },
        references,
        functions,
        issues,
    })
}

/// Extracts top-level treasure, equipment, and named collection declarations.
fn parse_named_block(node: Node<'_>, text: &str) -> Option<Definition> {
    let (header_kind, kind) = match node.kind() {
        "treasure_table" => ("treasure_table_header", "TreasureTable".to_owned()),
        "equipment_entry" => ("equipment_header", "Equipment".to_owned()),
        "named_block" => {
            let header = direct_child(node, "named_block_header")?;
            let raw_kind = field(header, "kind")?.utf8_text(text.as_bytes()).ok()?;
            let kind = match raw_kind {
                "spellset" => "SpellSet",
                "itemgroup" => "ItemGroup",
                "namegroup" => "NameGroup",
                other => other,
            };
            ("named_block_header", kind.to_owned())
        }
        _ => return None,
    };
    let header = direct_child(node, header_kind)?;
    let (name, selection_range) = quoted(field(header, "name")?, text);
    Some(Definition {
        kind,
        name,
        range: node_range(node),
        selection_range,
        fields: BTreeMap::new(),
        field_ranges: BTreeMap::new(),
        aliases: Vec::new(),
        uuid: None,
        parent: None,
        schema_id: None,
    })
}

/// Parses a Toolkit Stats or UUID-object table with streaming XML events.
fn parse_toolkit(
    source: SourceFile,
    text: &str,
    catalog: &SchemaCatalog,
) -> Result<ParsedFile, Error> {
    let lines = LineMap::new(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut schema_id = None;
    let mut current: Option<XmlRecord> = None;
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    let mut observed = HashMap::new();
    let mut value_parser = Parser::new();
    value_parser.set_language(&tree_sitter_bg3::BG3_STATS_VALUE_LANGUAGE.into())?;

    loop {
        let start = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let event = reader.read_event()?;
        let end = usize::try_from(reader.buffer_position()).unwrap_or(text.len());
        match event {
            Event::Start(event) if event.name().as_ref() == b"stats" => {
                schema_id = attributes(&event)?
                    .get("stat_object_definition_id")
                    .cloned();
            }
            Event::Start(event) if event.name().as_ref() == b"stat_object" => {
                current = Some(XmlRecord::new(lines.range(start, end)));
            }
            Event::Empty(event) if event.name().as_ref() == b"field" => {
                if let Some(record) = current.as_mut() {
                    let values = attributes(&event)?;
                    if let Some(name) = values.get("name") {
                        let value = values
                            .get("value")
                            .or_else(|| values.get("handle"))
                            .cloned()
                            .unwrap_or_default();
                        let range = attribute_range(
                            text,
                            &lines,
                            start,
                            end,
                            if values.contains_key("value") {
                                "value"
                            } else {
                                "handle"
                            },
                        )
                        .unwrap_or_else(|| lines.range(start, end));
                        record.fields.insert(name.clone(), value);
                        record.ranges.insert(name.clone(), range);
                    }
                }
            }
            Event::End(event) if event.name().as_ref() == b"stat_object" => {
                if let Some(mut record) = current.take() {
                    record.range.end = lines.position(end);
                    let definition_schema = schema_id.as_ref().and_then(|id| catalog.by_id.get(id));
                    let definition =
                        definition_from_xml_record(record, schema_id.clone(), definition_schema);
                    for (field_name, value) in &definition.fields {
                        if let Some(range) = definition.field_ranges.get(field_name) {
                            let (mut found, functions) =
                                parse_value(&mut value_parser, value, *range, field_name)?;
                            references.append(&mut found);
                            merge_functions(&mut observed, functions);
                        }
                    }
                    definitions.push(definition);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ParsedFile {
        source,
        definitions,
        references,
        observed_functions: sorted_functions(observed),
        issues: Vec::new(),
    })
}

/// Parses UUID-bearing resource declarations from relevant loose LSX files.
fn parse_lsx(source: SourceFile, text: &str) -> Result<ParsedFile, Error> {
    let lines = LineMap::new(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<ResourceRecord> = Vec::new();
    let mut definitions = Vec::new();

    loop {
        let start = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let event = reader.read_event()?;
        let end = usize::try_from(reader.buffer_position()).unwrap_or(text.len());
        match event {
            Event::Start(event) if event.name().as_ref() == b"node" => {
                let values = attributes(&event)?;
                stack.push(ResourceRecord {
                    node_kind: values
                        .get("id")
                        .cloned()
                        .unwrap_or_else(|| "Resource".into()),
                    record: XmlRecord::new(lines.range(start, end)),
                });
            }
            Event::Empty(event) if event.name().as_ref() == b"attribute" => {
                if let Some(resource) = stack.last_mut() {
                    let values = attributes(&event)?;
                    if let Some(name) = values.get("id") {
                        let (attribute, key) = if let Some(value) = values.get("value") {
                            (value.clone(), "value")
                        } else if let Some(handle) = values.get("handle") {
                            (handle.clone(), "handle")
                        } else {
                            continue;
                        };
                        resource.record.fields.insert(name.clone(), attribute);
                        resource.record.ranges.insert(
                            name.clone(),
                            attribute_range(text, &lines, start, end, key)
                                .unwrap_or_else(|| lines.range(start, end)),
                        );
                    }
                }
            }
            Event::End(event) if event.name().as_ref() == b"node" => {
                if let Some(mut resource) = stack.pop() {
                    resource.record.range.end = lines.position(end);
                    if let Some(definition) = definition_from_resource(resource) {
                        definitions.push(definition);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ParsedFile {
        source,
        definitions,
        references: Vec::new(),
        observed_functions: Vec::new(),
        issues: Vec::new(),
    })
}

/// Parses loose localization content for the configured language.
fn parse_localization(source: SourceFile, text: &str, language: &str) -> Result<ParsedFile, Error> {
    let lines = LineMap::new(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut current: Option<(String, String, TextRange, TextRange, String)> = None;
    let mut definitions = Vec::new();

    loop {
        let start = usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX);
        let event = reader.read_event()?;
        let end = usize::try_from(reader.buffer_position()).unwrap_or(text.len());
        match event {
            Event::Start(event) if event.name().as_ref() == b"content" => {
                let values = attributes(&event)?;
                if let Some(handle) = values.get("contentuid") {
                    current = Some((
                        handle.clone(),
                        values.get("version").cloned().unwrap_or_else(|| "1".into()),
                        lines.range(start, end),
                        attribute_range(text, &lines, start, end, "contentuid")
                            .unwrap_or_else(|| lines.range(start, end)),
                        String::new(),
                    ));
                }
            }
            Event::Text(event) => {
                if let Some((_, _, _, _, body)) = current.as_mut() {
                    body.push_str(&event.xml_content(XmlVersion::Implicit1_0)?);
                }
            }
            Event::End(event) if event.name().as_ref() == b"content" => {
                if let Some((handle, version, mut range, selection_range, body)) = current.take() {
                    range.end = lines.position(end);
                    definitions.push(Definition {
                        kind: "Localization".into(),
                        name: handle,
                        range,
                        selection_range,
                        fields: BTreeMap::from([
                            ("Language".into(), language.into()),
                            ("Text".into(), body),
                            ("Version".into(), version),
                        ]),
                        field_ranges: BTreeMap::new(),
                        aliases: Vec::new(),
                        uuid: None,
                        parent: None,
                        schema_id: None,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ParsedFile {
        source,
        definitions,
        references: Vec::new(),
        observed_functions: Vec::new(),
        issues: Vec::new(),
    })
}

#[derive(Debug)]
struct XmlRecord {
    fields: BTreeMap<String, String>,
    ranges: BTreeMap<String, TextRange>,
    range: TextRange,
}

impl XmlRecord {
    /// Creates an empty XML object record at its opening element.
    fn new(range: TextRange) -> Self {
        Self {
            fields: BTreeMap::new(),
            ranges: BTreeMap::new(),
            range,
        }
    }
}

#[derive(Debug)]
struct ResourceRecord {
    node_kind: String,
    record: XmlRecord,
}

/// Converts one Toolkit field collection to a semantic declaration.
fn definition_from_xml_record(
    record: XmlRecord,
    schema_id: Option<String>,
    schema: Option<&SchemaDefinition>,
) -> Definition {
    let aliases: Vec<_> = NAME_FIELDS
        .iter()
        .filter_map(|name| record.fields.get(*name))
        .filter(|value| !value.is_empty())
        .cloned()
        .collect();
    let uuid = record
        .fields
        .get("UUID")
        .and_then(|value| Uuid::parse_str(value).ok());
    let name = aliases
        .first()
        .cloned()
        .or_else(|| uuid.map(|value| value.to_string()))
        .unwrap_or_else(|| "<anonymous>".into());
    let selection_range = NAME_FIELDS
        .iter()
        .find_map(|name| record.ranges.get(*name).copied())
        .or_else(|| record.ranges.get("UUID").copied())
        .unwrap_or(record.range);
    let kind = schema
        .and_then(|value| {
            value
                .export_type
                .as_ref()
                .or(value.object_type.as_ref())
                .or(value.category.as_ref())
        })
        .cloned()
        .or_else(|| schema.map(|value| value.name.clone()))
        .unwrap_or_else(|| "Resource".into());
    let parent = record.fields.get("Using").cloned();
    Definition {
        kind,
        name,
        range: record.range,
        selection_range,
        fields: record.fields,
        field_ranges: record.ranges,
        aliases,
        uuid,
        parent,
        schema_id,
    }
}

/// Converts one LSX node to a declaration when it has a stable UUID identity.
fn definition_from_resource(resource: ResourceRecord) -> Option<Definition> {
    let uuid = resource
        .record
        .fields
        .get("MapKey")
        .or_else(|| resource.record.fields.get("UUID"))
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let aliases: Vec<_> = NAME_FIELDS
        .iter()
        .filter_map(|name| resource.record.fields.get(*name))
        .filter(|value| !value.is_empty())
        .cloned()
        .collect();
    if !resource.record.fields.contains_key("MapKey") && aliases.is_empty() {
        return None;
    }
    let name = aliases.first().cloned().unwrap_or_else(|| uuid.to_string());
    let selection_range = NAME_FIELDS
        .iter()
        .find_map(|field| resource.record.ranges.get(*field).copied())
        .or_else(|| resource.record.ranges.get("MapKey").copied())
        .or_else(|| resource.record.ranges.get("UUID").copied())
        .unwrap_or(resource.record.range);
    let kind = resource
        .record
        .fields
        .get("Type")
        .cloned()
        .unwrap_or(resource.node_kind);
    Some(Definition {
        kind,
        name,
        range: resource.record.range,
        selection_range,
        fields: resource.record.fields,
        field_ranges: resource.record.ranges,
        aliases,
        uuid: Some(uuid),
        parent: None,
        schema_id: None,
    })
}

/// Parses references and function calls from one field value.
fn parse_value(
    parser: &mut Parser,
    value: &str,
    origin: TextRange,
    field_name: &str,
) -> Result<(Vec<Reference>, Vec<ObservedFunction>), Error> {
    if value.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let tree = parser
        .parse(value, None)
        .ok_or_else(|| Error::Parse("the Stats-value parser returned no tree".into()))?;
    let mut references = Vec::new();
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "identifier" if !is_function_name(node) => {
                let name = node.utf8_text(value.as_bytes())?.to_owned();
                references.push(Reference {
                    target: SymbolTarget::Named {
                        kind: call_context(node, value).or_else(|| field_kind(field_name)),
                        name,
                    },
                    range: translate_range(node_range(node), origin),
                    context: format!("field:{field_name}"),
                });
            }
            "uuid" => {
                if let Ok(uuid) = Uuid::parse_str(node.utf8_text(value.as_bytes())?) {
                    references.push(Reference {
                        target: SymbolTarget::Uuid(uuid),
                        range: translate_range(node_range(node), origin),
                        context: "uuid".into(),
                    });
                }
            }
            "localization_handle" => {
                references.push(Reference {
                    target: SymbolTarget::Named {
                        kind: Some("Localization".into()),
                        name: node.utf8_text(value.as_bytes())?.to_owned(),
                    },
                    range: translate_range(node_range(node), origin),
                    context: "localization".into(),
                });
            }
            "string_content"
                if node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "string_literal") =>
            {
                references.push(Reference {
                    target: SymbolTarget::Named {
                        kind: call_context(node, value).or_else(|| field_kind(field_name)),
                        name: node.utf8_text(value.as_bytes())?.to_owned(),
                    },
                    range: translate_range(node_range(node), origin),
                    context: format!("field:{field_name}"),
                });
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    Ok((references, discover_functions(value)))
}

/// Applies a schema object type only when every viable candidate agrees on the kind.
fn apply_schema_reference_kinds(
    path: &Path,
    definition: &Definition,
    references: &mut [Reference],
    schema: &SchemaCatalog,
) {
    let candidates = schema.infer(path, Some(&definition.kind));
    for reference in references {
        let SymbolTarget::Named { kind: None, .. } = &reference.target else {
            continue;
        };
        let Some(field_name) = reference.context.strip_prefix("field:") else {
            continue;
        };
        let kinds: BTreeSet<_> = candidates
            .iter()
            .filter_map(|candidate| candidate.fields.get(field_name))
            .filter_map(|field| field.object_type.clone())
            .collect();
        if kinds.len() == 1
            && let Some(kind) = kinds.into_iter().next()
            && let SymbolTarget::Named { kind: target, .. } = &mut reference.target
        {
            *target = Some(kind);
        }
    }
}

/// Determines the typed symbol kind for a reference inside a function argument.
fn call_context(node: Node<'_>, source: &str) -> Option<String> {
    let mut current = Some(node);
    while let Some(candidate) = current {
        if candidate.kind() == "call_expression" {
            let function = field(candidate, "function")?;
            let arguments = field(candidate, "arguments")?;
            if !is_descendant(node, arguments) {
                return None;
            }
            let mut cursor = arguments.walk();
            for (index, argument) in arguments.named_children(&mut cursor).enumerate() {
                if is_descendant(node, argument) {
                    let name = function.utf8_text(source.as_bytes()).ok()?;
                    return parameter_kind(name, index);
                }
            }
            return None;
        }
        current = candidate.parent();
    }
    None
}

/// Returns the symbol kind assigned to a curated function parameter.
fn parameter_kind(name: &str, index: usize) -> Option<String> {
    if index != 0 {
        return None;
    }
    let kind = match name {
        "AddPassive" | "HasPassive" | "RemovePassive" | "UnlockPassive" => "PassiveData",
        "ApplyStatus" | "ForceStatus" | "HasAnyStatus" | "HasStatus" | "IsImmuneToStatus"
        | "RemoveStatus" => "StatusData",
        "RemoveSpell" | "UnlockSpell" | "UseSpell" => "SpellData",
        "UnlockInterrupt" => "InterruptData",
        "ActionResource" => "ActionResource",
        _ => return None,
    };
    Some(kind.into())
}

/// Returns the typed symbol kind assigned to a known Stats field.
fn field_kind(name: &str) -> Option<String> {
    let kind = match name {
        "ContainerSpells" | "Spells" => "SpellData",
        "InterruptPrototype" => "InterruptData",
        "Passives" | "PassivesAdded" | "PassivesOnEquip" => "PassiveData",
        "PersonalStatusImmunities" | "StatusImmunities" | "StatusInInventory" | "StatusOnEquip" => {
            "StatusData"
        }
        _ => return None,
    };
    Some(kind.into())
}

/// Discovers function names and argument-count ranges without inventing signatures.
fn discover_functions(source: &str) -> Vec<ObservedFunction> {
    let bytes = source.as_bytes();
    let mut found = HashMap::<String, ObservedFunction>::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_identifier_continue(bytes[cursor]) {
            cursor += 1;
        }
        let end = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'(' {
            continue;
        }
        let Some((arity, next)) = function_arity(bytes, cursor) else {
            continue;
        };
        let name = source[start..end].to_owned();
        let entry = found.entry(name.clone()).or_insert(ObservedFunction {
            name,
            count: 0,
            min_arity: arity,
            max_arity: arity,
        });
        entry.count += 1;
        entry.min_arity = entry.min_arity.min(arity);
        entry.max_arity = entry.max_arity.max(arity);
        cursor = next;
    }
    sorted_functions(found)
}

/// Counts the top-level arguments in one balanced call.
fn function_arity(bytes: &[u8], open: usize) -> Option<(u16, usize)> {
    let mut depth = 1_u16;
    let mut commas = 0_u16;
    let mut has_argument = false;
    let mut quote = None;
    let mut cursor = open + 1;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(active_quote) = quote {
            if byte == active_quote && bytes.get(cursor.wrapping_sub(1)) != Some(&b'\\') {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' => {
                    depth += 1;
                    has_argument = true;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((if has_argument { commas + 1 } else { 0 }, cursor + 1));
                    }
                }
                b',' if depth == 1 => commas += 1,
                byte if depth == 1 && !byte.is_ascii_whitespace() => has_argument = true,
                _ => {}
            }
        }
        cursor += 1;
    }
    None
}

/// Merges observed function aggregates by name.
fn merge_functions(
    target: &mut HashMap<String, ObservedFunction>,
    functions: Vec<ObservedFunction>,
) {
    use std::collections::hash_map::Entry;

    for function in functions {
        match target.entry(function.name.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(function);
            }
            Entry::Occupied(mut entry) => {
                let aggregate = entry.get_mut();
                aggregate.count += function.count;
                aggregate.min_arity = aggregate.min_arity.min(function.min_arity);
                aggregate.max_arity = aggregate.max_arity.max(function.max_arity);
            }
        }
    }
}

/// Returns observed functions in deterministic name order.
fn sorted_functions(functions: HashMap<String, ObservedFunction>) -> Vec<ObservedFunction> {
    let mut functions: Vec<_> = functions.into_values().collect();
    functions.sort_by(|left, right| left.name.cmp(&right.name));
    functions
}

/// Collects Tree-sitter error and missing nodes as recoverable source issues.
fn collect_syntax_issues(root: Node<'_>, issues: &mut Vec<SourceIssue>) {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.is_error() || node.is_missing() {
            issues.push(SourceIssue {
                code: "syntax-error".into(),
                message: if node.is_missing() {
                    "Required syntax is missing.".into()
                } else {
                    "The Stats syntax is not valid.".into()
                },
                range: node_range(node),
            });
        } else {
            let mut cursor = node.walk();
            pending.extend(node.children(&mut cursor));
        }
    }
}

/// Returns the first direct named child of a given type.
fn direct_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Returns the first Tree-sitter child assigned to a grammar field.
fn field<'tree>(node: Node<'tree>, name: &str) -> Option<Node<'tree>> {
    node.child_by_field_name(name)
}

/// Extracts quoted content and excludes the quote characters from its range.
fn quoted(node: Node<'_>, source: &str) -> (String, TextRange) {
    if let Some(content) = direct_child(node, "string_content") {
        return (
            content
                .utf8_text(source.as_bytes())
                .unwrap_or_default()
                .to_owned(),
            node_range(content),
        );
    }
    let mut range = node_range(node);
    range.start.character = range.start.character.saturating_add(1);
    range.end.character = range.end.character.saturating_sub(1);
    (String::new(), range)
}

/// Converts a Tree-sitter byte-column range to the internal range type.
fn node_range(node: Node<'_>) -> TextRange {
    let start = node.start_position();
    let end = node.end_position();
    TextRange {
        start: Position {
            line: u32::try_from(start.row).unwrap_or(u32::MAX),
            character: u32::try_from(start.column).unwrap_or(u32::MAX),
        },
        end: Position {
            line: u32::try_from(end.row).unwrap_or(u32::MAX),
            character: u32::try_from(end.column).unwrap_or(u32::MAX),
        },
    }
}

/// Translates a range inside a quoted value to its source-document range.
fn translate_range(mut range: TextRange, origin: TextRange) -> TextRange {
    range.start.line += origin.start.line;
    range.end.line += origin.start.line;
    if range.start.line == origin.start.line {
        range.start.character += origin.start.character;
    }
    if range.end.line == origin.start.line {
        range.end.character += origin.start.character;
    }
    range
}

/// Tests whether a node is equal to or below an ancestor.
fn is_descendant(mut node: Node<'_>, ancestor: Node<'_>) -> bool {
    loop {
        if node == ancestor {
            return true;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
    }
}

/// Tests whether an identifier is the name of a call expression.
fn is_function_name(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "call_expression" && field(parent, "function") == Some(node)
    })
}

/// Tests the first byte of an ASCII identifier.
fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Tests the remaining bytes of an ASCII identifier.
fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Returns the canonical kind used for definition lookup.
pub fn canonical_kind(kind: &str) -> &str {
    match kind {
        "Interrupt" => "InterruptData",
        "Passive" => "PassiveData",
        "Projectile" | "Shout" | "Target" => "SpellData",
        "Status" => "StatusData",
        other => other,
    }
}

/// Tests whether a string is a canonical UUID.
pub fn is_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok() && value.len() == 36
}

/// Selects the schema for an exact Toolkit schema ID.
pub fn schema_for_toolkit<'a>(
    catalog: &'a SchemaCatalog,
    schema_id: &str,
) -> Option<&'a SchemaDefinition> {
    catalog.by_id.get(schema_id)
}

/// Infers legacy schemas from source path and entry kind.
pub fn schemas_for_plain<'a>(
    catalog: &'a SchemaCatalog,
    path: &Path,
    kind: Option<&str>,
) -> Vec<&'a SchemaDefinition> {
    catalog.infer(path, kind)
}
