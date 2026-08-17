use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use tree_sitter::{Node, Parser};
use uuid::Uuid;

use crate::Error;
use crate::catalog::{field_kind, function_spec, is_lsx_value_field};
use crate::domain::{
    Definition, LineMap, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND,
    OSIRIS_QUERY_KIND, ObservedFunction, OsirisArgument, OsirisCallRole, OsirisDatabaseOccurrence,
    OsirisFile, OsirisTypeEvidence, ParsedFile, Position, Reference, SourceFile, SourceIssue,
    SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAssignment, ThothCall,
    ThothDeclaration, ThothDeclarationOwner, ThothExpression, ThothFile, ThothMemberAccess,
    ThothParameter, ThothReturn,
};
use crate::localization::valid_handle;
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
        SourceKind::Thoth => parse_thoth(source, text),
        SourceKind::Osiris => parse_osiris(source, text),
        SourceKind::Localization => parse_localization(source, text, language),
    }
}

/// Parses one Thoth document without assigning it a filesystem path.
///
/// This entry point is used for virtual package entries. It retains only
/// source ranges and syntax-backed observations, so callers can attach their
/// own package provenance without manufacturing a local document path.
pub fn parse_thoth_file(text: &str) -> Result<ThothFile, Error> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bg3::BG3_THOTH_LANGUAGE.into())?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| Error::Parse("the Thoth parser returned no tree".into()))?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(Error::Parse(
            "the packaged Thoth source contains invalid syntax".into(),
        ));
    }
    thoth_facts(root, text)
}

/// Extracts top-level helper declarations and call references from Thoth source.
fn parse_thoth(source: SourceFile, text: &str) -> Result<ParsedFile, Error> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bg3::BG3_THOTH_LANGUAGE.into())?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| Error::Parse("the Thoth parser returned no tree".into()))?;
    let root = tree.root_node();
    let mut issues = Vec::new();
    collect_tree_syntax_issues(
        root,
        &mut issues,
        "thoth-syntax-error",
        "The Thoth syntax is not valid.",
    );
    let mut definitions = Vec::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        if node.kind() != "function_declaration" {
            continue;
        }
        let Some(name_node) = field(node, "name") else {
            continue;
        };
        let name = name_node.utf8_text(text.as_bytes())?.to_owned();
        let parameters = field(node, "parameters")
            .map(|parameters| thoth_parameters(parameters, text))
            .unwrap_or_default();
        let mut fields = BTreeMap::new();
        fields.insert("Parameters".into(), parameters.join(", "));
        definitions.push(Definition {
            kind: THOTH_FUNCTION_KIND.into(),
            name,
            range: node_range(node),
            selection_range: node_range(name_node),
            fields,
            field_ranges: BTreeMap::new(),
            aliases: Vec::new(),
            uuid: None,
            parent: None,
            schema_id: None,
            arity: None,
        });
    }

    let references = thoth_call_references(root, text)?;
    let thoth = thoth_facts(root, text)?;
    let observed_functions = thoth_observed_functions(&thoth);
    Ok(ParsedFile {
        source,
        definitions,
        references,
        observed_functions,
        issues,
        osiris: None,
        thoth: Some(thoth),
    })
}

/// Returns declared parameter names without inventing types from function bodies.
fn thoth_parameters(node: Node<'_>, text: &str) -> Vec<String> {
    thoth_parameter_facts(node, text)
        .into_iter()
        .map(|parameter| parameter.name)
        .collect()
}

/// Extracts syntax-backed Thoth facts without assigning types to expressions.
fn thoth_facts(root: Node<'_>, text: &str) -> Result<ThothFile, Error> {
    let mut facts = ThothFile::default();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "function_declaration" => {
                if let Some(name_node) = field(node, "name") {
                    facts.declarations.push(ThothDeclaration {
                        name: node_text(name_node, text)?,
                        range: node_range(node),
                        name_range: node_range(name_node),
                        parameters: field(node, "parameters")
                            .map(|parameters| thoth_parameter_facts(parameters, text))
                            .unwrap_or_default(),
                    });
                }
            }
            "return_statement" => {
                let expressions = direct_child(node, "expression_list")
                    .map(|list| thoth_expression_children(list, text))
                    .transpose()?
                    .unwrap_or_default();
                facts.returns.push(ThothReturn {
                    range: node_range(node),
                    expressions,
                    owner: None,
                });
            }
            "function_call" => {
                if let (Some(name_node), Some(arguments_node)) =
                    (field(node, "name"), field(node, "arguments"))
                {
                    let arguments = thoth_expression_children(arguments_node, text)?;
                    let arity = u16::try_from(arguments.len()).map_err(|_| {
                        Error::Parse("a Thoth call has more than 65535 arguments".into())
                    })?;
                    facts.calls.push(ThothCall {
                        name: node_text(name_node, text)?,
                        name_range: node_range(name_node),
                        range: node_range(node),
                        arguments,
                        arity,
                        owner: None,
                    });
                }
            }
            "variable_declaration" => {
                if let Some(assignment) = direct_child(node, "assignment_statement") {
                    facts.assignments.push(thoth_assignment(
                        assignment,
                        text,
                        node_range(node),
                        true,
                        is_global_declaration(node),
                    )?);
                } else if let Some(targets) = direct_child(node, "variable_list") {
                    facts.assignments.push(ThothAssignment {
                        range: node_range(node),
                        local: !is_global_declaration(node),
                        global: is_global_declaration(node),
                        targets: thoth_expression_children(targets, text)?,
                        values: Vec::new(),
                        owner: None,
                    });
                }
            }
            "assignment_statement" if !has_variable_declaration_parent(node) => {
                facts.assignments.push(thoth_assignment(
                    node,
                    text,
                    node_range(node),
                    false,
                    false,
                )?);
            }
            "dot_index_expression" | "method_index_expression" | "bracket_index_expression"
                if !has_member_parent(node) =>
            {
                if let Some(member_access) = thoth_member_access(node, text)? {
                    facts.member_accesses.push(member_access);
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    facts
        .declarations
        .sort_by_key(|fact| (fact.range.start.line, fact.range.start.character));
    facts
        .returns
        .sort_by_key(|fact| (fact.range.start.line, fact.range.start.character));
    facts
        .calls
        .sort_by_key(|fact| (fact.range.start.line, fact.range.start.character));
    facts
        .assignments
        .sort_by_key(|fact| (fact.range.start.line, fact.range.start.character));
    facts
        .member_accesses
        .sort_by_key(|fact| (fact.range.start.line, fact.range.start.character));
    assign_thoth_owners(&mut facts);
    Ok(facts)
}

fn assign_thoth_owners(facts: &mut ThothFile) {
    let declarations = facts.declarations.clone();
    for fact in &mut facts.returns {
        fact.owner = thoth_owner(&declarations, fact.range);
    }
    for fact in &mut facts.calls {
        fact.owner = thoth_owner(&declarations, fact.range);
    }
    for fact in &mut facts.assignments {
        fact.owner = thoth_owner(&declarations, fact.range);
    }
    for fact in &mut facts.member_accesses {
        fact.owner = thoth_owner(&declarations, fact.range);
    }
}

fn thoth_owner(
    declarations: &[ThothDeclaration],
    range: TextRange,
) -> Option<ThothDeclarationOwner> {
    declarations
        .iter()
        .rev()
        .find(|declaration| range_contains(declaration.range, range))
        .map(|declaration| ThothDeclarationOwner {
            name: declaration.name.clone(),
            range: declaration.range,
        })
}

fn range_contains(outer: TextRange, inner: TextRange) -> bool {
    range_position_le(outer.start, inner.start) && range_position_le(inner.end, outer.end)
}

fn range_position_le(left: Position, right: Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character <= right.character)
}

fn thoth_observed_functions(facts: &ThothFile) -> Vec<ObservedFunction> {
    let mut observed = HashMap::<String, ObservedFunction>::new();
    for call in &facts.calls {
        let entry = observed
            .entry(call.name.clone())
            .or_insert_with(|| ObservedFunction {
                name: call.name.clone(),
                count: 0,
                min_arity: call.arity,
                max_arity: call.arity,
            });
        entry.count += 1;
        entry.min_arity = entry.min_arity.min(call.arity);
        entry.max_arity = entry.max_arity.max(call.arity);
    }
    sorted_functions(observed)
}

fn thoth_parameter_facts(node: Node<'_>, text: &str) -> Vec<ThothParameter> {
    let mut parameters = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => parameters.push(ThothParameter {
                name: child
                    .utf8_text(text.as_bytes())
                    .unwrap_or_default()
                    .to_owned(),
                range: node_range(child),
                variadic: false,
            }),
            "vararg_expression" => parameters.push(ThothParameter {
                name: child
                    .utf8_text(text.as_bytes())
                    .unwrap_or_default()
                    .to_owned(),
                range: node_range(child),
                variadic: true,
            }),
            _ => {}
        }
    }
    parameters
}

fn thoth_assignment(
    node: Node<'_>,
    text: &str,
    range: TextRange,
    local: bool,
    global: bool,
) -> Result<ThothAssignment, Error> {
    let targets = direct_child(node, "variable_list")
        .map(|list| thoth_expression_children(list, text))
        .transpose()?
        .unwrap_or_default();
    let values = direct_child(node, "expression_list")
        .map(|list| thoth_expression_children(list, text))
        .transpose()?
        .unwrap_or_default();
    Ok(ThothAssignment {
        range,
        local,
        global,
        targets,
        values,
        owner: None,
    })
}

fn thoth_expression_children(node: Node<'_>, text: &str) -> Result<Vec<ThothExpression>, Error> {
    let mut expressions = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        expressions.push(ThothExpression {
            range: node_range(child),
            text: node_text(child, text)?,
        });
    }
    Ok(expressions)
}

fn thoth_member_access(node: Node<'_>, text: &str) -> Result<Option<ThothMemberAccess>, Error> {
    let Some((root, members)) = thoth_member_parts(node, text)? else {
        return Ok(None);
    };
    Ok(Some(ThothMemberAccess {
        range: node_range(node),
        text: node_text(node, text)?,
        root,
        members,
        owner: None,
    }))
}

fn thoth_member_parts(node: Node<'_>, text: &str) -> Result<Option<(String, Vec<String>)>, Error> {
    let (table, member) = match node.kind() {
        "dot_index_expression" => (field(node, "table"), field(node, "field")),
        "method_index_expression" => (field(node, "table"), field(node, "method")),
        "bracket_index_expression" => (field(node, "table"), field(node, "field")),
        _ => return Ok(None),
    };
    let (Some(table), Some(member)) = (table, member) else {
        return Ok(None);
    };
    let member = node_text(member, text)?;
    let (root, mut members) = if let Some((root, members)) = thoth_member_parts(table, text)? {
        (root, members)
    } else {
        (node_text(table, text)?, Vec::new())
    };
    members.push(member);
    Ok(Some((root, members)))
}

fn has_member_parent(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "dot_index_expression" | "method_index_expression" | "bracket_index_expression"
        )
    })
}

fn has_variable_declaration_parent(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == "variable_declaration")
}

fn is_global_declaration(node: Node<'_>) -> bool {
    node.parent()
        .and_then(|parent| field(parent, "global_declaration"))
        .is_some()
}

fn node_text(node: Node<'_>, text: &str) -> Result<String, Error> {
    Ok(node.utf8_text(text.as_bytes())?.to_owned())
}

/// Collects every call name, including nested calls, as a semantic helper target.
fn thoth_call_references(root: Node<'_>, text: &str) -> Result<Vec<Reference>, Error> {
    let mut references = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.kind() == "function_call"
            && let Some(name) = field(node, "name")
        {
            references.push(Reference {
                target: SymbolTarget::Named {
                    kind: Some(THOTH_FUNCTION_KIND.into()),
                    name: name.utf8_text(text.as_bytes())?.to_owned(),
                },
                range: node_range(name),
                context: "function-call".into(),
            });
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    Ok(references)
}

/// Extracts declarations, calls, database evidence, and syntax issues from one Osiris goal.
fn parse_osiris(source: SourceFile, text: &str) -> Result<ParsedFile, Error> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bg3::BG3_OSIRIS_LANGUAGE.into())?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| Error::Parse("the Osiris parser returned no tree".into()))?;
    let root = tree.root_node();
    let goal = source
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| Error::Parse("an Osiris goal path has no UTF-8 file stem".into()))?
        .to_owned();
    let mut issues = Vec::new();
    collect_tree_syntax_issues(
        root,
        &mut issues,
        "osiris-syntax-error",
        "The Osiris goal syntax is not valid.",
    );

    let mut goal_fields = BTreeMap::new();
    if let Some(version) = direct_child(root, "version_declaration")
        && let Some(value) = field(version, "value")
    {
        goal_fields.insert(
            "Version".into(),
            value.utf8_text(text.as_bytes())?.to_owned(),
        );
    }
    let goal_selection = direct_child(root, "version_declaration")
        .map(node_range)
        .unwrap_or_else(|| node_range(root));
    let mut definitions = vec![Definition {
        kind: OSIRIS_GOAL_KIND.into(),
        name: goal.clone(),
        range: node_range(root),
        selection_range: goal_selection,
        fields: goal_fields,
        field_ranges: BTreeMap::new(),
        aliases: Vec::new(),
        uuid: None,
        parent: None,
        schema_id: None,
        arity: None,
    }];
    let mut references = Vec::new();
    let mut occurrences = Vec::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "init_section" | "exit_section" => {
                let mut section_cursor = node.walk();
                for statement in node.named_children(&mut section_cursor) {
                    if statement.kind() == "fact_statement"
                        && let Some(call) = field(statement, "call")
                    {
                        collect_osiris_call(
                            call,
                            text,
                            OsirisCallRole::Write,
                            &HashMap::new(),
                            &mut references,
                            &mut occurrences,
                        )?;
                    }
                }
            }
            "kb_section" => {
                let mut section_cursor = node.walk();
                for rule in node.named_children(&mut section_cursor) {
                    if rule.kind() == "rule" {
                        parse_osiris_rule(
                            rule,
                            text,
                            &goal,
                            &mut definitions,
                            &mut references,
                            &mut occurrences,
                        )?;
                    }
                }
            }
            "parent_target_edge" => {
                if let Some(parent) = field(node, "goal") {
                    let (name, range) = quoted(parent, text);
                    references.push(Reference {
                        target: SymbolTarget::OsirisGoal { name },
                        range,
                        context: "osiris-parent-goal".into(),
                    });
                }
            }
            _ => {}
        }
    }

    add_osiris_database_anchors(&goal, &occurrences, &mut definitions);
    Ok(ParsedFile {
        source,
        definitions,
        references,
        observed_functions: Vec::new(),
        issues,
        osiris: Some(OsirisFile { goal, occurrences }),
        thoth: None,
    })
}

/// Extracts one complete rule and its rule-local explicit variable types.
fn parse_osiris_rule(
    rule: Node<'_>,
    text: &str,
    goal: &str,
    definitions: &mut Vec<Definition>,
    references: &mut Vec<Reference>,
    occurrences: &mut Vec<OsirisDatabaseOccurrence>,
) -> Result<(), Error> {
    let variable_types = osiris_rule_variable_types(rule, text)?;
    let Some(head) = field(rule, "head") else {
        return Ok(());
    };
    let Some(name_node) = field(head, "name") else {
        return Ok(());
    };
    let Some(arguments) = field(head, "arguments") else {
        return Ok(());
    };
    let name = name_node.utf8_text(text.as_bytes())?.to_owned();
    let arity = osiris_arity(arguments)?;
    let kind = field(rule, "kind")
        .and_then(|kind| kind.utf8_text(text.as_bytes()).ok())
        .unwrap_or("IF");
    match kind {
        "PROC" | "QRY" => {
            let parameters = osiris_parameter_labels(arguments, text, &variable_types)?;
            definitions.push(Definition {
                kind: if kind == "PROC" {
                    OSIRIS_PROCEDURE_KIND.into()
                } else {
                    OSIRIS_QUERY_KIND.into()
                },
                name,
                range: node_range(rule),
                selection_range: node_range(name_node),
                fields: BTreeMap::from([
                    ("Goal".into(), goal.into()),
                    ("Parameters".into(), parameters.join(", ")),
                ]),
                field_ranges: BTreeMap::new(),
                aliases: Vec::new(),
                uuid: None,
                parent: None,
                schema_id: None,
                arity: Some(arity),
            });
        }
        _ => collect_osiris_call(
            head,
            text,
            OsirisCallRole::Read,
            &variable_types,
            references,
            occurrences,
        )?,
    }

    let mut cursor = rule.walk();
    for child in rule.named_children(&mut cursor) {
        match child.kind() {
            "condition" => {
                if let Some(call) = direct_child(child, "call_expression") {
                    collect_osiris_call(
                        call,
                        text,
                        OsirisCallRole::Read,
                        &variable_types,
                        references,
                        occurrences,
                    )?;
                }
            }
            "action_statement" => {
                if let Some(call) = field(child, "call") {
                    collect_osiris_call(
                        call,
                        text,
                        OsirisCallRole::Write,
                        &variable_types,
                        references,
                        occurrences,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Adds one call reference and retains structured evidence for user databases.
fn collect_osiris_call(
    call: Node<'_>,
    text: &str,
    role: OsirisCallRole,
    variable_types: &HashMap<String, OsirisTypeEvidence>,
    references: &mut Vec<Reference>,
    occurrences: &mut Vec<OsirisDatabaseOccurrence>,
) -> Result<(), Error> {
    let Some(name_node) = field(call, "name") else {
        return Ok(());
    };
    let Some(arguments_node) = field(call, "arguments") else {
        return Ok(());
    };
    let name = name_node.utf8_text(text.as_bytes())?.to_owned();
    let arity = osiris_arity(arguments_node)?;
    let target = if name.starts_with("DB_") {
        SymbolTarget::OsirisDatabase {
            name: name.clone(),
            arity,
        }
    } else {
        SymbolTarget::OsirisCallable {
            name: name.clone(),
            arity,
        }
    };
    references.push(Reference {
        target,
        range: node_range(name_node),
        context: match role {
            OsirisCallRole::Read => "osiris-read",
            OsirisCallRole::Write => "osiris-write",
        }
        .into(),
    });
    if !name.starts_with("DB_") {
        return Ok(());
    }

    let mut arguments = Vec::new();
    let mut cursor = arguments_node.walk();
    for argument in arguments_node.named_children(&mut cursor) {
        arguments.push(OsirisArgument {
            range: node_range(argument),
            evidence: osiris_argument_evidence(argument, text, variable_types)?,
        });
    }
    occurrences.push(OsirisDatabaseOccurrence {
        name,
        arity,
        range: node_range(call),
        selection_range: node_range(name_node),
        role,
        arguments,
    });
    Ok(())
}

/// Converts a grammar argument count into the stable symbol-key width.
fn osiris_arity(arguments: Node<'_>) -> Result<u16, Error> {
    u16::try_from(arguments.named_child_count())
        .map_err(|_| Error::Parse("an Osiris call has more than 65,535 arguments".into()))
}

/// Records one navigation anchor for each database written by this goal.
fn add_osiris_database_anchors(
    goal: &str,
    occurrences: &[OsirisDatabaseOccurrence],
    definitions: &mut Vec<Definition>,
) {
    let mut databases = BTreeMap::<(String, u16), OsirisDatabaseAggregate>::new();
    for (index, occurrence) in occurrences.iter().enumerate() {
        let database = databases
            .entry((occurrence.name.clone(), occurrence.arity))
            .or_insert_with(|| OsirisDatabaseAggregate {
                first_write: None,
                reads: 0,
                writes: 0,
                types: vec![BTreeSet::new(); usize::from(occurrence.arity)],
            });
        match occurrence.role {
            OsirisCallRole::Read => database.reads += 1,
            OsirisCallRole::Write => {
                database.writes += 1;
                database.first_write.get_or_insert(index);
            }
        }
        for (column, argument) in database.types.iter_mut().zip(&occurrence.arguments) {
            if let Some(evidence) = &argument.evidence {
                column.insert(evidence.type_name.clone());
            }
        }
    }

    for ((name, arity), database) in databases {
        let Some(first_write) = database.first_write else {
            continue;
        };
        let occurrence = &occurrences[first_write];
        let parameters = database
            .types
            .into_iter()
            .map(osiris_type_summary)
            .collect::<Vec<_>>();
        definitions.push(Definition {
            kind: OSIRIS_DATABASE_KIND.into(),
            name,
            range: occurrence.range,
            selection_range: occurrence.selection_range,
            fields: BTreeMap::from([
                ("Goal".into(), goal.into()),
                ("Parameters".into(), parameters.join(", ")),
                ("Reads".into(), database.reads.to_string()),
                ("Writes".into(), database.writes.to_string()),
            ]),
            field_ranges: BTreeMap::new(),
            aliases: Vec::new(),
            uuid: None,
            parent: None,
            schema_id: None,
            arity: Some(arity),
        });
    }
}

/// Per-goal database facts accumulated in one pass over call occurrences.
struct OsirisDatabaseAggregate {
    first_write: Option<usize>,
    reads: usize,
    writes: usize,
    types: Vec<BTreeSet<String>>,
}

/// Summarizes exact source types without choosing among conflicts.
fn osiris_type_summary(types: BTreeSet<String>) -> String {
    if types.len() == 1 {
        types.into_iter().next().unwrap_or_else(|| "unknown".into())
    } else if types.len() > 1 {
        "conflicting".into()
    } else {
        "unknown".into()
    }
}

/// Returns display labels for one declared PROC or QRY parameter list.
fn osiris_parameter_labels(
    arguments: Node<'_>,
    text: &str,
    variable_types: &HashMap<String, OsirisTypeEvidence>,
) -> Result<Vec<String>, Error> {
    let mut parameters = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.named_children(&mut cursor) {
        let value = field(argument, "value")
            .and_then(|value| value.utf8_text(text.as_bytes()).ok())
            .unwrap_or("value");
        let evidence = osiris_argument_evidence(argument, text, variable_types)?;
        parameters.push(evidence.map_or_else(
            || value.to_owned(),
            |evidence| format!("{} {value}", evidence.type_name),
        ));
    }
    Ok(parameters)
}

/// Collects unambiguous explicit casts for variables inside one rule.
fn osiris_rule_variable_types(
    rule: Node<'_>,
    text: &str,
) -> Result<HashMap<String, OsirisTypeEvidence>, Error> {
    let mut candidates = HashMap::<String, Option<OsirisTypeEvidence>>::new();
    let mut pending = vec![rule];
    while let Some(node) = pending.pop() {
        if node.kind() == "typed_variable"
            && let (Some(cast), Some(value)) = (field(node, "cast"), field(node, "value"))
            && value.kind() == "local_variable"
            && let Some(type_node) = field(cast, "type")
        {
            let name = value.utf8_text(text.as_bytes())?.to_owned();
            let evidence = OsirisTypeEvidence {
                type_name: type_node.utf8_text(text.as_bytes())?.to_owned(),
                source_range: node_range(type_node),
            };
            candidates
                .entry(name)
                .and_modify(|current| {
                    if current
                        .as_ref()
                        .is_some_and(|current| current.type_name != evidence.type_name)
                    {
                        *current = None;
                    }
                })
                .or_insert(Some(evidence));
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    Ok(candidates
        .into_iter()
        .filter_map(|(name, evidence)| evidence.map(|evidence| (name, evidence)))
        .collect())
}

/// Returns an exact type observation for one literal or explicitly typed variable.
fn osiris_argument_evidence(
    argument: Node<'_>,
    text: &str,
    variable_types: &HashMap<String, OsirisTypeEvidence>,
) -> Result<Option<OsirisTypeEvidence>, Error> {
    if let Some(cast) = field(argument, "cast")
        && let Some(type_node) = field(cast, "type")
    {
        return Ok(Some(OsirisTypeEvidence {
            type_name: type_node.utf8_text(text.as_bytes())?.to_owned(),
            source_range: node_range(type_node),
        }));
    }
    let Some(value) = field(argument, "value") else {
        return Ok(None);
    };
    if value.kind() == "local_variable" {
        return Ok(variable_types
            .get(value.utf8_text(text.as_bytes())?)
            .cloned());
    }
    let type_name = match value.kind() {
        "integer" => "INTEGER",
        "real" => "REAL",
        "string_literal" => "STRING",
        "guid_literal" => "GUIDSTRING",
        _ => return Ok(None),
    };
    Ok(Some(OsirisTypeEvidence {
        type_name: type_name.into(),
        source_range: node_range(value),
    }))
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
        osiris: None,
        thoth: None,
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
            arity: None,
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
        arity: None,
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
        osiris: None,
        thoth: None,
    })
}

/// Parses UUID-bearing resource declarations from relevant loose LSX files.
fn parse_lsx(source: SourceFile, text: &str) -> Result<ParsedFile, Error> {
    let lines = LineMap::new(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<ResourceRecord> = Vec::new();
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
                        let range = attribute_range(text, &lines, start, end, key)
                            .unwrap_or_else(|| lines.range(start, end));
                        if key == "handle"
                            && values
                                .get("type")
                                .is_some_and(|field_type| field_type == "TranslatedString")
                            && valid_handle(&attribute)
                        {
                            references.push(Reference {
                                target: SymbolTarget::Named {
                                    kind: Some("Localization".into()),
                                    name: attribute.clone(),
                                },
                                range,
                                context: "localization".into(),
                            });
                        }
                        resource.record.fields.insert(name.clone(), attribute);
                        resource.record.ranges.insert(name.clone(), range);
                    }
                }
            }
            Event::End(event) if event.name().as_ref() == b"node" => {
                if let Some(mut resource) = stack.pop() {
                    resource.record.range.end = lines.position(end);
                    if let Some(definition) = definition_from_resource(resource) {
                        for (field_name, value) in &definition.fields {
                            if !is_lsx_value_field(field_name) {
                                continue;
                            }
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
        osiris: None,
        thoth: None,
    })
}

/// Parses loose localization content for the configured language.
fn parse_localization(source: SourceFile, text: &str, language: &str) -> Result<ParsedFile, Error> {
    let lines = LineMap::new(text);
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut current: Option<(String, String, TextRange, TextRange, usize, String)> = None;
    let mut definitions = Vec::new();
    let mut references = Vec::new();

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
                        end,
                        String::new(),
                    ));
                }
            }
            Event::Text(event) => {
                if let Some((_, _, _, _, _, body)) = current.as_mut() {
                    body.push_str(&event.xml_content(XmlVersion::Implicit1_0)?);
                }
            }
            Event::CData(event) => {
                if let Some((_, _, _, _, _, body)) = current.as_mut() {
                    body.push_str(&event.xml_content(XmlVersion::Implicit1_0)?);
                }
            }
            Event::GeneralRef(event) => {
                if let Some((_, _, _, _, _, body)) = current.as_mut() {
                    let reference = event.xml10_content()?;
                    if let Some(number) = reference.strip_prefix('#') {
                        let (radix, digits) = number
                            .strip_prefix(['x', 'X'])
                            .map_or((10, number), |digits| (16, digits));
                        let value = u32::from_str_radix(digits, radix).map_err(|_| {
                            Error::Parse(format!(
                                "localization contains invalid character reference `&{reference};`"
                            ))
                        })?;
                        let character = char::from_u32(value).ok_or_else(|| {
                            Error::Parse(format!(
                                "localization contains invalid character reference `&{reference};`"
                            ))
                        })?;
                        body.push(character);
                    } else if let Some(value) = quick_xml::escape::resolve_xml_entity(&reference) {
                        body.push_str(value);
                    } else {
                        return Err(Error::Parse(format!(
                            "localization contains unknown entity `&{reference};`"
                        )));
                    }
                }
            }
            Event::End(event) if event.name().as_ref() == b"content" => {
                if let Some((handle, version, mut range, selection_range, body_start, body)) =
                    current.take()
                {
                    references.extend(localization_tooltip_references(
                        text, &lines, body_start, start,
                    ));
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
                        arity: None,
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
        references,
        observed_functions: Vec::new(),
        issues: Vec::new(),
        osiris: None,
        thoth: None,
    })
}

/// Finds exact `Tooltip` value ranges in encoded, mixed, and literal LSTag markup.
fn localization_tooltip_references(
    source: &str,
    lines: &LineMap,
    start: usize,
    end: usize,
) -> Vec<Reference> {
    let mut references = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let literal = source[cursor..end]
            .find("<LSTag")
            .map(|offset| cursor + offset);
        let encoded = source[cursor..end]
            .find("&lt;LSTag")
            .map(|offset| cursor + offset);
        let Some(tag_start) = [literal, encoded].into_iter().flatten().min() else {
            break;
        };
        let name_end = tag_start
            + if source[tag_start..].starts_with("&lt;") {
                "&lt;LSTag".len()
            } else {
                "<LSTag".len()
            };
        if !source.as_bytes().get(name_end).is_some_and(|byte| {
            byte.is_ascii_whitespace()
                || *byte == b'>'
                || source.as_bytes()[name_end..end].starts_with(b"&gt;")
        }) {
            cursor = name_end;
            continue;
        }
        let Some(tag_end) = localization_tag_end(source, name_end, end) else {
            break;
        };
        let attributes = localization_tag_attributes(source, name_end, tag_end);
        if let Some((tooltip, value_start, value_end)) = attributes.get("Tooltip") {
            let kind = attributes
                .get("Type")
                .and_then(|(value, _, _)| localization_tag_kind(value));
            let target = if attributes.contains_key("Type") {
                kind.map(|kind| SymbolTarget::Named {
                    kind: Some(kind.into()),
                    name: tooltip.clone(),
                })
            } else {
                Some(SymbolTarget::Tooltip {
                    name: tooltip.clone(),
                })
            };
            if let Some(target) = target {
                references.push(Reference {
                    target,
                    range: lines.range(*value_start, *value_end),
                    context: "localization-tooltip".into(),
                });
            }
        }
        cursor = tag_end.saturating_add(1);
    }
    references
}

fn localization_tag_end(source: &str, start: usize, limit: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut quote = None;
    let mut cursor = start;
    while cursor < limit {
        let byte = bytes[cursor];
        if let Some(active) = quote {
            if byte == active {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'>' || bytes[cursor..limit].starts_with(b"&gt;") {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn localization_tag_attributes(
    source: &str,
    start: usize,
    end: usize,
) -> BTreeMap<String, (String, usize, usize)> {
    let bytes = source.as_bytes();
    let mut attributes = BTreeMap::new();
    let mut cursor = start;
    while cursor < end {
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < end
            && (bytes[cursor].is_ascii_alphanumeric()
                || matches!(bytes[cursor], b'_' | b':' | b'-'))
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &source[name_start..cursor];
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let Some(quote @ (b'\'' | b'"')) = bytes.get(cursor).copied() else {
            continue;
        };
        cursor += 1;
        let value_start = cursor;
        while cursor < end && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor >= end {
            break;
        }
        attributes.insert(
            name.to_owned(),
            (source[value_start..cursor].to_owned(), value_start, cursor),
        );
        cursor += 1;
    }
    attributes
}

fn localization_tag_kind(value: &str) -> Option<&'static str> {
    match value {
        "ActionResource" => Some("ActionResource"),
        "Interrupt" => Some("InterruptData"),
        "Passive" => Some("PassiveData"),
        "Spell" => Some("SpellData"),
        "Status" => Some("StatusData"),
        _ => None,
    }
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
        arity: None,
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
        arity: None,
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
            "call_expression" => {
                if let Some(function) = field(node, "function") {
                    let name = function.utf8_text(value.as_bytes())?;
                    // Curated functions already have hover, completion, and signature
                    // contracts. Only unknown calls can resolve to a mod helper.
                    if function_spec(name).is_none() {
                        references.push(Reference {
                            target: SymbolTarget::Named {
                                kind: Some(THOTH_FUNCTION_KIND.into()),
                                name: name.to_owned(),
                            },
                            range: translate_range(node_range(function), origin),
                            context: "function-call".into(),
                        });
                    }
                }
            }
            "identifier" if !is_function_name(node) => {
                let name = node.utf8_text(value.as_bytes())?.to_owned();
                let call_kind = call_context(node, value);
                if call_kind == Some(None) {
                    continue;
                }
                references.push(Reference {
                    target: named_target(
                        name,
                        call_kind
                            .flatten()
                            .or_else(|| field_kind(field_name).map(str::to_owned)),
                    ),
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
                let name = node.utf8_text(value.as_bytes())?.to_owned();
                let call_kind = call_context(node, value);
                if call_kind == Some(None) {
                    continue;
                }
                references.push(Reference {
                    target: named_target(
                        name,
                        call_kind
                            .flatten()
                            .or_else(|| field_kind(field_name).map(str::to_owned)),
                    ),
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

/// Separates status-group markers from concrete status declarations.
fn named_target(name: String, kind: Option<String>) -> SymbolTarget {
    let kind = if name.starts_with("SG_") && kind.as_deref() == Some("StatusData") {
        Some("StatusGroup".into())
    } else {
        kind
    };
    SymbolTarget::Named { kind, name }
}

/// Applies a schema object type only when every viable candidate agrees on the kind.
fn apply_schema_reference_kinds(
    path: &Path,
    definition: &Definition,
    references: &mut [Reference],
    schema: &SchemaCatalog,
) {
    let candidates = schema.infer_legacy(path, Some(&definition.kind), &definition.fields);
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
fn call_context(node: Node<'_>, source: &str) -> Option<Option<String>> {
    let mut current = Some(node);
    while let Some(candidate) = current {
        if candidate.kind() == "call_expression" {
            let function = field(candidate, "function")?;
            let arguments = field(candidate, "arguments")?;
            if !is_descendant(node, arguments) {
                return None;
            }
            let mut cursor = arguments.walk();
            let argument_count = arguments.named_child_count();
            let first_argument = arguments
                .named_child(0)
                .and_then(|argument| argument.utf8_text(source.as_bytes()).ok());
            for (index, argument) in arguments.named_children(&mut cursor).enumerate() {
                if is_descendant(node, argument) {
                    let name = function.utf8_text(source.as_bytes()).ok()?;
                    let form = function_spec(name)?.form_for_call(argument_count, first_argument);
                    return form
                        .parameters
                        .get(index)
                        .map(|parameter| parameter.kind.map(str::to_owned));
                }
            }
            return None;
        }
        current = candidate.parent();
    }
    None
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
    collect_tree_syntax_issues(
        root,
        issues,
        "syntax-error",
        "The Stats syntax is not valid.",
    );
}

/// Collects one stable issue for each outermost Tree-sitter error or missing node.
fn collect_tree_syntax_issues(
    root: Node<'_>,
    issues: &mut Vec<SourceIssue>,
    code: &str,
    invalid_message: &str,
) {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.is_error() || node.is_missing() {
            issues.push(SourceIssue {
                code: code.into(),
                message: if node.is_missing() {
                    "Required syntax is missing.".into()
                } else {
                    invalid_message.into()
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
