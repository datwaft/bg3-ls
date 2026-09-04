use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use tree_sitter::{Node, Parser};
use uuid::Uuid;

use crate::Error;
use crate::annotation::{
    ThothAliasAnnotation, ThothClassAnnotation, ThothFieldAnnotation, ThothFunctionAnnotation,
    ThothFunctionContract, ThothParameterAnnotation, ThothReturnAnnotation,
    ThothVariableAnnotation, TypeExpression, parse_type_expression,
};
use crate::catalog::{field_kind, function_spec, is_lsx_value_field, osiris_signature};
use crate::domain::{
    Definition, LineMap, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND,
    OSIRIS_QUERY_KIND, ObservedFunction, OsirisArgument, OsirisCallRole, OsirisDatabaseBinding,
    OsirisDatabaseOccurrence, OsirisEvidenceOrigin, OsirisFile, OsirisTypeCast, OsirisTypeEvidence,
    OsirisVariableFact, OsirisVariableOccurrence, ParsedFile, Position, Reference, SourceFile,
    SourceIssue, SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAssignment,
    ThothBinaryOperator, ThothCall, ThothControlFlowFact, ThothDeclaration, ThothDeclarationOwner,
    ThothExpression, ThothExpressionFact, ThothExpressionKind, ThothFile, ThothIfBranch,
    ThothIfBranchKind, ThothLexicalScope, ThothLiteralKind, ThothMemberAccess,
    ThothMemberAccessKind, ThothMemberSegment, ThothParameter, ThothReturn, ThothScopeId,
    ThothStatementId, ThothUnaryOperator,
};
use crate::localization::valid_handle;
use crate::osiris_catalog::{
    OSIRIS_CONTRACTS, OsirisContractKind, OsirisParameterDirection, osiris_argument_domain,
    osiris_contract, osiris_type_class,
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
    let (facts, annotation_issues) = thoth_facts(root, text)?;
    if !annotation_issues.is_empty() {
        return Err(Error::Parse(
            "the packaged Thoth source contains invalid annotations".into(),
        ));
    }
    Ok(facts)
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
    let (thoth, annotation_issues) = thoth_facts(root, text)?;
    issues.extend(annotation_issues);
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
fn thoth_facts(root: Node<'_>, text: &str) -> Result<(ThothFile, Vec<SourceIssue>), Error> {
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
                    statement: None,
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
                    let global = is_global_declaration(node);
                    facts.assignments.push(thoth_assignment(
                        assignment,
                        text,
                        node_range(node),
                        !global,
                        global,
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
    let mut annotation_issues = Vec::new();
    collect_thoth_annotations(root, text, &mut facts.annotations, &mut annotation_issues)?;
    assign_thoth_owners(&mut facts);
    collect_thoth_expression_facts(root, text, &mut facts)?;
    Ok((facts, annotation_issues))
}

/// Extracts ordered, syntax-only expression facts in a separate pass.
///
/// The legacy fact vectors above intentionally keep their existing traversal
/// and ordering. This pass follows executable children of each lexical scope
/// so statement identity remains stable when expressions are nested or when
/// two statements share a source row.
fn collect_thoth_expression_facts(
    root: Node<'_>,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    facts.scopes.push(ThothLexicalScope {
        id: ThothScopeId::File,
        parent: None,
    });
    collect_thoth_scope_children(root, ThothScopeId::File, text, facts)
}

fn collect_thoth_scope_children(
    container: Node<'_>,
    scope: ThothScopeId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let mut cursor = container.walk();
    let children = container.named_children(&mut cursor).collect::<Vec<_>>();
    let mut order = 0;
    for child in children {
        if !is_thoth_executable_statement(child) {
            continue;
        }
        let statement = ThothStatementId { scope, order };
        order = order.saturating_add(1);
        collect_thoth_statement(child, statement, text, facts)?;
    }
    Ok(())
}

fn is_thoth_executable_statement(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "assignment_statement"
            | "break_statement"
            | "do_statement"
            | "for_statement"
            | "function_call"
            | "function_declaration"
            | "goto_statement"
            | "if_statement"
            | "label_statement"
            | "repeat_statement"
            | "return_statement"
            | "try_statement"
            | "variable_declaration"
            | "while_statement"
    )
}

fn collect_thoth_statement(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    match node.kind() {
        "function_declaration" => collect_thoth_function(node, statement.scope, text, facts)?,
        "assignment_statement" => {
            collect_thoth_assignment_expressions(node, statement, text, facts)?
        }
        "variable_declaration" => {
            if let Some(assignment) = direct_child(node, "assignment_statement") {
                collect_thoth_assignment_expressions(assignment, statement, text, facts)?;
            } else if let Some(targets) = direct_child(node, "variable_list") {
                collect_thoth_expression_children(targets, statement, text, facts)?;
            }
        }
        "function_call" => collect_thoth_expression(node, statement, text, facts)?,
        "return_statement" => {
            if let Some(return_fact) = facts
                .returns
                .iter_mut()
                .find(|return_fact| return_fact.range == node_range(node))
            {
                return_fact.statement = Some(statement);
            }
            if let Some(expressions) = direct_child(node, "expression_list") {
                collect_thoth_expression_children(expressions, statement, text, facts)?;
            }
        }
        "if_statement" => {
            if let Some(condition) = field(node, "condition") {
                facts.condition_ranges.push(node_range(condition));
                collect_thoth_expression(condition, statement, text, facts)?;
            }
            let branches = collect_thoth_if_branches(node, statement, text, facts)?;
            facts.control_flow.push(ThothControlFlowFact {
                statement,
                branches,
            });
        }
        "while_statement" => {
            if let Some(condition) = field(node, "condition") {
                facts.condition_ranges.push(node_range(condition));
                collect_thoth_expression(condition, statement, text, facts)?;
            }
            collect_thoth_field_block(node, "body", statement.scope, text, facts)?;
        }
        "repeat_statement" => {
            collect_thoth_field_block(node, "body", statement.scope, text, facts)?;
            collect_thoth_field_expression(node, "condition", statement, text, facts)?;
        }
        "do_statement" => collect_thoth_field_block(node, "body", statement.scope, text, facts)?,
        "for_statement" => {
            if let Some(clause) = field(node, "clause") {
                match clause.kind() {
                    "for_generic_clause" => {
                        if let Some(targets) = direct_child(clause, "variable_list") {
                            collect_thoth_expression_children(targets, statement, text, facts)?;
                        }
                        if let Some(values) = direct_child(clause, "expression_list") {
                            collect_thoth_expression_children(values, statement, text, facts)?;
                        }
                    }
                    "for_numeric_clause" => {
                        collect_thoth_field_expression(clause, "name", statement, text, facts)?;
                        for name in ["start", "end", "step"] {
                            collect_thoth_field_expression(clause, name, statement, text, facts)?;
                        }
                    }
                    _ => {}
                }
            }
            collect_thoth_field_block(node, "body", statement.scope, text, facts)?;
        }
        "try_statement" => {
            collect_thoth_field_block(node, "body", statement.scope, text, facts)?;
            collect_thoth_field_block(node, "handler", statement.scope, text, facts)?;
        }
        _ => {}
    }
    Ok(())
}

fn collect_thoth_function(
    node: Node<'_>,
    parent: ThothScopeId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let scope = ThothScopeId::Function {
        range: node_range(node),
    };
    facts.scopes.push(ThothLexicalScope {
        id: scope,
        parent: Some(parent),
    });
    if let Some(body) = field(node, "body") {
        collect_thoth_block(body, scope, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_block(
    node: Node<'_>,
    parent: ThothScopeId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let scope = ThothScopeId::Block {
        range: node_range(node),
    };
    facts.scopes.push(ThothLexicalScope {
        id: scope,
        parent: Some(parent),
    });
    collect_thoth_scope_children(node, scope, text, facts)
}

fn collect_thoth_if_branches(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<Vec<ThothIfBranch>, Error> {
    let mut branches = Vec::new();
    if let Some(consequence) = field(node, "consequence") {
        collect_thoth_block(consequence, statement.scope, text, facts)?;
        branches.push(ThothIfBranch {
            kind: ThothIfBranchKind::Consequence,
            condition: field(node, "condition").map(node_range),
            scope: Some(ThothScopeId::Block {
                range: node_range(consequence),
            }),
        });
    } else {
        branches.push(ThothIfBranch {
            kind: ThothIfBranchKind::Consequence,
            condition: field(node, "condition").map(node_range),
            scope: None,
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "elseif_statement" => {
                if let Some(condition) = field(child, "condition") {
                    facts.condition_ranges.push(node_range(condition));
                    collect_thoth_expression(condition, statement, text, facts)?;
                }
                if let Some(body) = field(child, "consequence") {
                    collect_thoth_block(body, statement.scope, text, facts)?;
                    branches.push(ThothIfBranch {
                        kind: ThothIfBranchKind::ElseIf,
                        condition: field(child, "condition").map(node_range),
                        scope: Some(ThothScopeId::Block {
                            range: node_range(body),
                        }),
                    });
                } else {
                    branches.push(ThothIfBranch {
                        kind: ThothIfBranchKind::ElseIf,
                        condition: field(child, "condition").map(node_range),
                        scope: None,
                    });
                }
            }
            "else_statement" => {
                if let Some(body) = field(child, "body") {
                    collect_thoth_block(body, statement.scope, text, facts)?;
                    branches.push(ThothIfBranch {
                        kind: ThothIfBranchKind::Else,
                        condition: None,
                        scope: Some(ThothScopeId::Block {
                            range: node_range(body),
                        }),
                    });
                } else {
                    branches.push(ThothIfBranch {
                        kind: ThothIfBranchKind::Else,
                        condition: None,
                        scope: None,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(branches)
}

fn collect_thoth_field_block(
    node: Node<'_>,
    name: &str,
    parent: ThothScopeId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    if let Some(body) = field(node, name) {
        collect_thoth_block(body, parent, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_field_expression(
    node: Node<'_>,
    name: &str,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    if let Some(expression) = field(node, name) {
        collect_thoth_expression(expression, statement, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_assignment_expressions(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    if let Some(targets) = direct_child(node, "variable_list") {
        collect_thoth_expression_children(targets, statement, text, facts)?;
    }
    if let Some(values) = direct_child(node, "expression_list") {
        collect_thoth_expression_children(values, statement, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_expression_children(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_thoth_expression(child, statement, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_expression(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let kind = match node.kind() {
        "nil" => ThothExpressionKind::Literal(ThothLiteralKind::Nil),
        "false" | "true" => ThothExpressionKind::Literal(ThothLiteralKind::Boolean),
        "number" => ThothExpressionKind::Literal(ThothLiteralKind::Number),
        "string" => ThothExpressionKind::Literal(ThothLiteralKind::String),
        "identifier" => ThothExpressionKind::Identifier,
        "function_call" => ThothExpressionKind::FunctionCall,
        "parenthesized_expression" => {
            first_named_child(node).map_or(ThothExpressionKind::Unknown, |expression| {
                ThothExpressionKind::Parenthesized {
                    expression: node_range(expression),
                }
            })
        }
        "unary_expression" => match (thoth_unary_operator(node).ok(), field(node, "operand")) {
            (Some(operator), Some(operand)) => ThothExpressionKind::Unary {
                operator,
                operand: node_range(operand),
            },
            _ => ThothExpressionKind::Unknown,
        },
        "binary_expression" => match (
            thoth_binary_operator(node).ok(),
            field(node, "left"),
            field(node, "right"),
        ) {
            (Some(operator), Some(left), Some(right)) => ThothExpressionKind::Binary {
                operator,
                left: node_range(left),
                right: node_range(right),
            },
            _ => ThothExpressionKind::Unknown,
        },
        "dot_index_expression" | "method_index_expression" | "bracket_index_expression" => {
            thoth_member_segments(node, text)?.map_or(
                ThothExpressionKind::Unknown,
                ThothExpressionKind::MemberAccess,
            )
        }
        _ => ThothExpressionKind::Unknown,
    };
    facts.expression_facts.push(ThothExpressionFact {
        range: node_range(node),
        text: node_text(node, text)?,
        kind,
        statement,
    });

    match node.kind() {
        "function_call" => {
            if let Some(name) = field(node, "name") {
                collect_thoth_expression(name, statement, text, facts)?;
            }
            if let Some(arguments) = field(node, "arguments") {
                if arguments.kind() == "arguments" {
                    collect_thoth_expression_children(arguments, statement, text, facts)?;
                } else {
                    collect_thoth_expression(arguments, statement, text, facts)?;
                }
            }
        }
        "function_definition" => {
            let scope = ThothScopeId::Function {
                range: node_range(node),
            };
            facts.scopes.push(ThothLexicalScope {
                id: scope,
                parent: Some(statement.scope),
            });
            if let Some(body) = field(node, "body") {
                collect_thoth_block(body, scope, text, facts)?;
            }
        }
        "dot_index_expression" | "method_index_expression" | "bracket_index_expression" => {
            collect_thoth_member_children(node, statement, text, facts)?;
        }
        "field" => collect_thoth_table_field(node, statement, text, facts)?,
        _ => collect_thoth_unknown_children(node, statement, text, facts)?,
    }
    Ok(())
}

fn thoth_unary_operator(node: Node<'_>) -> Result<ThothUnaryOperator, Error> {
    let operator = field(node, "operator")
        .ok_or_else(|| Error::Parse("a unary Thoth expression has no operator".into()))?
        .kind();
    match operator {
        "not" => Ok(ThothUnaryOperator::Not),
        "#" => Ok(ThothUnaryOperator::Length),
        "-" => Ok(ThothUnaryOperator::Negate),
        "~" => Ok(ThothUnaryOperator::BitNot),
        _ => Err(Error::Parse(format!(
            "unsupported Thoth unary operator `{operator}`"
        ))),
    }
}

fn thoth_binary_operator(node: Node<'_>) -> Result<ThothBinaryOperator, Error> {
    let operator = field(node, "operator")
        .ok_or_else(|| Error::Parse("a binary Thoth expression has no operator".into()))?
        .kind();
    match operator {
        "or" => Ok(ThothBinaryOperator::Or),
        "and" => Ok(ThothBinaryOperator::And),
        "<" => Ok(ThothBinaryOperator::Less),
        "<=" => Ok(ThothBinaryOperator::LessOrEqual),
        "==" => Ok(ThothBinaryOperator::Equal),
        "~=" => Ok(ThothBinaryOperator::NotEqual),
        ">=" => Ok(ThothBinaryOperator::GreaterOrEqual),
        ">" => Ok(ThothBinaryOperator::Greater),
        "|" => Ok(ThothBinaryOperator::BitOr),
        "~" => Ok(ThothBinaryOperator::BitXor),
        "&" => Ok(ThothBinaryOperator::BitAnd),
        "<<" => Ok(ThothBinaryOperator::ShiftLeft),
        ">>" => Ok(ThothBinaryOperator::ShiftRight),
        ".." => Ok(ThothBinaryOperator::Concatenate),
        "+" => Ok(ThothBinaryOperator::Add),
        "-" => Ok(ThothBinaryOperator::Subtract),
        "*" => Ok(ThothBinaryOperator::Multiply),
        "/" => Ok(ThothBinaryOperator::Divide),
        "//" => Ok(ThothBinaryOperator::FloorDivide),
        "%" => Ok(ThothBinaryOperator::Modulo),
        "^" => Ok(ThothBinaryOperator::Power),
        _ => Err(Error::Parse(format!(
            "unsupported Thoth binary operator `{operator}`"
        ))),
    }
}

fn collect_thoth_unknown_children(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "field" => collect_thoth_table_field(child, statement, text, facts)?,
            "expression_list" | "variable_list" | "arguments" => {
                collect_thoth_expression_children(child, statement, text, facts)?
            }
            "block" | "parameters" => {}
            _ => collect_thoth_expression(child, statement, text, facts)?,
        }
    }
    Ok(())
}

fn collect_thoth_table_field(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    if let Some(name) = field(node, "name") {
        let source = node_text(node, text)?;
        let is_bracket_key = source.trim_start().starts_with('[');
        if is_bracket_key {
            collect_thoth_expression(name, statement, text, facts)?;
        }
    }
    if let Some(value) = field(node, "value") {
        collect_thoth_expression(value, statement, text, facts)?;
    }
    Ok(())
}

fn collect_thoth_member_children(
    node: Node<'_>,
    statement: ThothStatementId,
    text: &str,
    facts: &mut ThothFile,
) -> Result<(), Error> {
    if let Some(table) = field(node, "table") {
        if matches!(
            table.kind(),
            "dot_index_expression" | "method_index_expression" | "bracket_index_expression"
        ) {
            collect_thoth_member_children(table, statement, text, facts)?;
        } else if matches!(table.kind(), "function_call" | "parenthesized_expression") {
            collect_thoth_expression(table, statement, text, facts)?;
        }
    }
    if node.kind() == "bracket_index_expression"
        && let Some(field) = field(node, "field")
    {
        collect_thoth_expression(field, statement, text, facts)?;
    }
    Ok(())
}

fn thoth_member_segments(
    node: Node<'_>,
    text: &str,
) -> Result<Option<Vec<ThothMemberSegment>>, Error> {
    match node.kind() {
        "identifier" | "function_call" | "parenthesized_expression" => {
            Ok(Some(vec![ThothMemberSegment {
                text: node_text(node, text)?,
                range: node_range(node),
                access: ThothMemberAccessKind::Root,
            }]))
        }
        "dot_index_expression" | "method_index_expression" | "bracket_index_expression" => {
            let Some(table) = field(node, "table") else {
                return Ok(None);
            };
            let Some(mut segments) = thoth_member_segments(table, text)? else {
                return Ok(None);
            };
            let (access, member) = match node.kind() {
                "dot_index_expression" => {
                    let Some(member) = field(node, "field") else {
                        return Ok(None);
                    };
                    (ThothMemberAccessKind::Dot, member)
                }
                "method_index_expression" => {
                    let Some(member) = field(node, "method") else {
                        return Ok(None);
                    };
                    (ThothMemberAccessKind::Method, member)
                }
                "bracket_index_expression" => {
                    let Some(member) = field(node, "field") else {
                        return Ok(None);
                    };
                    (ThothMemberAccessKind::Bracket, member)
                }
                _ => return Ok(None),
            };
            segments.push(ThothMemberSegment {
                text: node_text(member, text)?,
                range: node_range(member),
                access,
            });
            Ok(Some(segments))
        }
        _ => Ok(None),
    }
}

#[derive(Clone, Debug)]
struct ThothAnnotationComment {
    range: TextRange,
    line: u32,
    tag: ThothAnnotationTag,
}

#[derive(Clone, Debug)]
enum ThothAnnotationTag {
    Class {
        name: String,
        name_range: TextRange,
    },
    Field {
        name: String,
        name_range: TextRange,
        ty: TypeExpression,
        type_range: TextRange,
    },
    Alias {
        name: String,
        name_range: TextRange,
        ty: TypeExpression,
        type_range: TextRange,
    },
    Param {
        name: String,
        name_range: TextRange,
        ty: TypeExpression,
        type_range: TextRange,
        variadic: bool,
    },
    Return {
        ty: TypeExpression,
        type_range: TextRange,
    },
    Type {
        ty: TypeExpression,
        type_range: TextRange,
    },
    Documentation(String),
    Unsupported,
    Invalid,
}

#[derive(Clone, Copy, Debug)]
enum ThothAnnotationTarget<'tree> {
    Function(Node<'tree>),
    Variable(Node<'tree>),
}

/// Extracts LuaCATS-like line annotations and attaches them only across an
/// uninterrupted row boundary. Tree-sitter exposes comments as named extras;
/// retaining all comments in the ordered stream makes ordinary comments and
/// blank rows explicit attachment barriers.
fn collect_thoth_annotations(
    root: Node<'_>,
    text: &str,
    annotations: &mut crate::annotation::ThothAnnotations,
    issues: &mut Vec<SourceIssue>,
) -> Result<(), Error> {
    let mut comments = Vec::new();
    let mut targets = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.kind() == "comment"
            && let Some(comment) = parse_thoth_annotation_comment(node, text, issues)?
        {
            comments.push(comment);
        }
        match node.kind() {
            "function_declaration" => targets.push(ThothAnnotationTarget::Function(node)),
            "variable_declaration" => targets.push(ThothAnnotationTarget::Variable(node)),
            "assignment_statement" if !has_variable_declaration_parent(node) => {
                targets.push(ThothAnnotationTarget::Variable(node));
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }

    comments.sort_by_key(|comment| (comment.range.start.line, comment.range.start.character));
    targets.sort_by_key(|target| match target {
        ThothAnnotationTarget::Function(node) | ThothAnnotationTarget::Variable(node) => {
            (node.start_position().row, node.start_position().column)
        }
    });

    let mut groups: Vec<Vec<ThothAnnotationComment>> = Vec::new();
    for comment in comments {
        if matches!(comment.tag, ThothAnnotationTag::Unsupported) {
            groups.push(vec![comment]);
        } else if groups.last().is_some_and(|group| {
            !matches!(
                group.last().expect("non-empty group").tag,
                ThothAnnotationTag::Unsupported
            ) && comment.line == group.last().expect("non-empty group").line + 1
        }) {
            groups.last_mut().expect("group exists").push(comment);
        } else {
            groups.push(vec![comment]);
        }
    }

    for group in groups {
        collect_class_annotations(&group, annotations);
        collect_alias_annotations(&group, annotations);

        let last_line = group.last().expect("annotation group is non-empty").line;
        let target = targets.iter().find(|target| match target {
            ThothAnnotationTarget::Function(node) | ThothAnnotationTarget::Variable(node) => {
                u32::try_from(node.start_position().row).unwrap_or(u32::MAX) == last_line + 1
            }
        });
        if let Some(ThothAnnotationTarget::Function(function)) = target
            && let Some(annotation) = function_annotation(&group, *function, text)
        {
            annotations.functions.push(annotation);
        }
        if let Some(ThothAnnotationTarget::Variable(variable)) = target
            && let Some(variable_annotations) = variable_annotations(&group, *variable, text)?
        {
            annotations.variables.extend(variable_annotations);
        }
    }
    Ok(())
}

/// Recognizes a triple-dash doc-comment line.
///
/// The grammar consumes the first two dashes as the comment marker, so a
/// `--- text` line exposes the content `- text`. A single trailing dash that
/// is followed by whitespace or nothing marks documentation; deeper dash runs
/// stay unsupported so that `----` separators keep breaking attachment.
fn thoth_documentation_text(content: &str) -> Option<String> {
    let prose = content.strip_prefix('-')?;
    if !prose.is_empty() && !prose.starts_with(char::is_whitespace) {
        return None;
    }
    Some(prose.trim().to_owned())
}

fn parse_thoth_annotation_comment(
    node: Node<'_>,
    text: &str,
    issues: &mut Vec<SourceIssue>,
) -> Result<Option<ThothAnnotationComment>, Error> {
    let Some(content) = field(node, "content") else {
        return Ok(Some(ThothAnnotationComment {
            range: node_range(node),
            line: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
            tag: ThothAnnotationTag::Unsupported,
        }));
    };
    let content_text = content.utf8_text(text.as_bytes())?;
    if let Some(prose) = thoth_documentation_text(content_text) {
        return Ok(Some(ThothAnnotationComment {
            range: node_range(node),
            line: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
            tag: ThothAnnotationTag::Documentation(prose),
        }));
    }
    let Some(tag_text) = content_text.strip_prefix("-@") else {
        return Ok(Some(ThothAnnotationComment {
            range: node_range(node),
            line: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
            tag: ThothAnnotationTag::Unsupported,
        }));
    };
    let range = node_range(node);
    let line = u32::try_from(node.start_position().row).unwrap_or(u32::MAX);
    let content_start = content.start_byte() + 2;
    let tag = parse_thoth_annotation_tag(tag_text, content_start, text)?;
    if matches!(tag, ThothAnnotationTag::Invalid) {
        issues.push(SourceIssue {
            code: "thoth-annotation-error".into(),
            message: "The Thoth annotation is malformed.".into(),
            range,
        });
    }
    Ok(Some(ThothAnnotationComment { range, line, tag }))
}

fn parse_thoth_annotation_tag(
    input: &str,
    source_start: usize,
    source: &str,
) -> Result<ThothAnnotationTag, Error> {
    let (tag, rest, rest_offset) = take_annotation_word(input);
    let Some(tag) = tag else {
        return Ok(ThothAnnotationTag::Invalid);
    };
    let rest_start = source_start + rest_offset + tag.len();
    match tag {
        "class" => {
            let (Some(name), _, offset) = take_annotation_word(rest) else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            if !valid_annotation_name(name, true) {
                return Ok(ThothAnnotationTag::Invalid);
            }
            Ok(ThothAnnotationTag::Class {
                name: name.to_owned(),
                name_range: byte_range(
                    source,
                    rest_start + offset,
                    rest_start + offset + name.len(),
                ),
            })
        }
        "field" => {
            let (Some(name), type_input, offset) = take_annotation_word(rest) else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            if !valid_annotation_name(name, false) {
                return Ok(ThothAnnotationTag::Invalid);
            }
            let Some((ty, type_range)) =
                parse_annotation_type(type_input, source, rest_start + offset + name.len())?
            else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            Ok(ThothAnnotationTag::Field {
                name: name.to_owned(),
                name_range: byte_range(
                    source,
                    rest_start + offset,
                    rest_start + offset + name.len(),
                ),
                ty,
                type_range,
            })
        }
        "alias" => {
            let (Some(name), type_input, offset) = take_annotation_word(rest) else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            if !valid_annotation_name(name, true) {
                return Ok(ThothAnnotationTag::Invalid);
            }
            let Some((ty, type_range)) =
                parse_annotation_type(type_input, source, rest_start + offset + name.len())?
            else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            Ok(ThothAnnotationTag::Alias {
                name: name.to_owned(),
                name_range: byte_range(
                    source,
                    rest_start + offset,
                    rest_start + offset + name.len(),
                ),
                ty,
                type_range,
            })
        }
        "param" => {
            let (Some(raw_name), type_input, offset) = take_annotation_word(rest) else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            let raw_name_len = raw_name.len();
            let (raw_name, variadic) = raw_name
                .strip_prefix("...")
                .map_or((raw_name, false), |name| (name, true));
            let (name, optional) = raw_name
                .strip_suffix('?')
                .map_or((raw_name, false), |name| (name, true));
            if !valid_annotation_name(name, false) {
                return Ok(ThothAnnotationTag::Invalid);
            }
            let name_offset = rest_start + offset + usize::from(variadic) * 3;
            let Some((ty, type_range)) =
                parse_annotation_type(type_input, source, rest_start + offset + raw_name_len)?
            else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            let ty = if optional {
                TypeExpression::union([ty, TypeExpression::Nil])
            } else {
                ty
            };
            Ok(ThothAnnotationTag::Param {
                name: name.to_owned(),
                name_range: byte_range(source, name_offset, name_offset + name.len()),
                ty,
                type_range,
                variadic,
            })
        }
        "return" | "returns" => {
            let Some((ty, type_range)) = parse_annotation_type(rest, source, rest_start)? else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            Ok(ThothAnnotationTag::Return { ty, type_range })
        }
        "type" => {
            let Some((ty, type_range)) = parse_annotation_type(rest, source, rest_start)? else {
                return Ok(ThothAnnotationTag::Invalid);
            };
            Ok(ThothAnnotationTag::Type { ty, type_range })
        }
        _ => Ok(ThothAnnotationTag::Unsupported),
    }
}

fn take_annotation_word(input: &str) -> (Option<&str>, &str, usize) {
    let trimmed = input.trim_start_matches(char::is_whitespace);
    let offset = input.len() - trimmed.len();
    let end = trimmed
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(trimmed.len());
    if end == 0 {
        (None, trimmed, offset)
    } else {
        (Some(&trimmed[..end]), &trimmed[end..], offset)
    }
}

fn parse_annotation_type(
    input: &str,
    source: &str,
    source_start: usize,
) -> Result<Option<(TypeExpression, TextRange)>, Error> {
    let trimmed = input.trim_start_matches(char::is_whitespace);
    if trimmed.is_empty() {
        return Ok(None);
    }
    let leading = input.len() - trimmed.len();
    let parsed = match parse_type_expression(trimmed) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    let start = source_start + leading;
    let consumed = parsed.consumed();
    if trimmed[consumed..]
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return Ok(None);
    }
    Ok(Some((
        parsed.ty,
        byte_range(source, start, start + consumed),
    )))
}

fn valid_annotation_name(name: &str, dotted: bool) -> bool {
    if dotted {
        name.split('.').all(valid_annotation_identifier)
    } else {
        valid_annotation_identifier(name)
    }
}

fn valid_annotation_identifier(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && is_identifier_start(bytes[0])
        && bytes[1..].iter().copied().all(is_identifier_continue)
}

fn collect_class_annotations(
    group: &[ThothAnnotationComment],
    annotations: &mut crate::annotation::ThothAnnotations,
) {
    for (index, comment) in group.iter().enumerate() {
        let ThothAnnotationTag::Class { name, name_range } = &comment.tag else {
            continue;
        };
        let mut fields = Vec::new();
        for field_comment in group.iter().skip(index + 1) {
            let ThothAnnotationTag::Field {
                name,
                name_range,
                ty,
                type_range,
            } = &field_comment.tag
            else {
                break;
            };
            fields.push(ThothFieldAnnotation {
                name: name.clone(),
                ty: ty.clone(),
                range: field_comment.range,
                name_range: *name_range,
                type_range: *type_range,
            });
        }
        let end = fields
            .last()
            .map_or(comment.range.end, |field| field.range.end);
        annotations.classes.push(ThothClassAnnotation {
            name: name.clone(),
            range: TextRange {
                start: comment.range.start,
                end,
            },
            name_range: *name_range,
            fields,
        });
    }
}

fn collect_alias_annotations(
    group: &[ThothAnnotationComment],
    annotations: &mut crate::annotation::ThothAnnotations,
) {
    for comment in group {
        if let ThothAnnotationTag::Alias {
            name,
            name_range,
            ty,
            type_range,
        } = &comment.tag
        {
            annotations.aliases.push(ThothAliasAnnotation {
                name: name.clone(),
                ty: ty.clone(),
                range: comment.range,
                name_range: *name_range,
                type_range: *type_range,
            });
        }
    }
}

fn function_annotation(
    group: &[ThothAnnotationComment],
    function: Node<'_>,
    text: &str,
) -> Option<ThothFunctionAnnotation> {
    let description = group
        .iter()
        .filter_map(|comment| match &comment.tag {
            ThothAnnotationTag::Documentation(prose) if !prose.is_empty() => Some(prose.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let parameters = group
        .iter()
        .filter_map(|comment| match &comment.tag {
            ThothAnnotationTag::Param {
                name,
                name_range,
                ty,
                type_range,
                variadic,
            } => Some(ThothParameterAnnotation {
                name: name.clone(),
                ty: ty.clone(),
                range: comment.range,
                name_range: *name_range,
                type_range: *type_range,
                variadic: *variadic,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let returns = group
        .iter()
        .filter_map(|comment| match &comment.tag {
            ThothAnnotationTag::Return { ty, type_range } => Some(ThothReturnAnnotation {
                ty: ty.clone(),
                range: comment.range,
                type_range: *type_range,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parameters.is_empty() && returns.is_empty() && description.is_empty() {
        return None;
    }
    let name = field(function, "name");
    let name_range = name.map(node_range);
    Some(ThothFunctionAnnotation {
        name: name.and_then(|node| node.utf8_text(text.as_bytes()).ok().map(str::to_owned)),
        range: TextRange {
            start: group.first()?.range.start,
            end: group.last()?.range.end,
        },
        name_range,
        contracts: vec![ThothFunctionContract {
            parameters,
            returns,
            description,
            range: TextRange {
                start: group.first()?.range.start,
                end: group.last()?.range.end,
            },
        }],
    })
}

fn variable_annotations(
    group: &[ThothAnnotationComment],
    variable: Node<'_>,
    text: &str,
) -> Result<Option<Vec<ThothVariableAnnotation>>, Error> {
    let types = group
        .iter()
        .filter_map(|comment| match &comment.tag {
            ThothAnnotationTag::Type { ty, type_range } => {
                Some((comment.range, ty.clone(), *type_range))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if types.is_empty() {
        return Ok(None);
    }
    if types.len() != 1 {
        return Ok(None);
    }
    let targets = if variable.kind() == "variable_declaration" {
        direct_child(variable, "assignment_statement")
            .or(Some(variable))
            .and_then(|node| direct_child(node, "variable_list"))
    } else {
        direct_child(variable, "variable_list")
    };
    let Some(targets) = targets else {
        return Ok(None);
    };
    let mut cursor = targets.walk();
    let target_nodes = targets.named_children(&mut cursor).collect::<Vec<_>>();
    if target_nodes.len() != 1 {
        return Ok(None);
    }
    let target = target_nodes[0];
    let target_text = target.utf8_text(text.as_bytes())?.to_owned();
    let (range, ty, type_range) = types.first().expect("non-empty type annotations");
    Ok(Some(vec![ThothVariableAnnotation {
        target: target_text,
        ty: ty.clone(),
        range: *range,
        target_range: node_range(target),
        type_range: *type_range,
    }]))
}

fn byte_range(source: &str, start: usize, end: usize) -> TextRange {
    TextRange {
        start: byte_position(source, start),
        end: byte_position(source, end),
    }
}

fn byte_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let character = prefix.rsplit('\n').next().map_or(prefix.len(), str::len);
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(character).unwrap_or(u32::MAX),
    }
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
    node.child(0).is_some_and(|child| child.kind() == "global")
        || node
            .parent()
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
    // A `.txt` file below a Goals directory must contain a complete goal.
    // The grammar also accepts standalone callable signatures for editor
    // tooling, and Tree-sitter can recover a partial goal while a document is
    // being edited. Neither shape is an indexable goal source. Keep the
    // syntax issues from the recovery tree, but do not let partial facts leak
    // into module or database indexes.
    let Some(goal_root) = complete_osiris_goal_root(root) else {
        return Ok(ParsedFile {
            source,
            definitions: Vec::new(),
            references: Vec::new(),
            observed_functions: Vec::new(),
            issues,
            osiris: None,
            thoth: None,
        });
    };
    collect_osiris_callable_role_issues(goal_root, text, &mut issues)?;

    let mut goal_fields = BTreeMap::new();
    if let Some(version) = direct_child(goal_root, "version_declaration")
        && let Some(value) = field(version, "value")
    {
        goal_fields.insert(
            "Version".into(),
            value.utf8_text(text.as_bytes())?.to_owned(),
        );
    }
    let goal_selection = direct_child(goal_root, "version_declaration")
        .map(node_range)
        .unwrap_or_else(|| node_range(goal_root));
    let mut definitions = vec![Definition {
        kind: OSIRIS_GOAL_KIND.into(),
        name: goal.clone(),
        range: node_range(goal_root),
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
    let mut variables = Vec::new();
    let casts = osiris_type_casts(goal_root, text)?;
    let mut cursor = goal_root.walk();
    for node in goal_root.named_children(&mut cursor) {
        match node.kind() {
            "init_section" | "exit_section" => {
                let mut section_cursor = node.walk();
                for statement in node.named_children(&mut section_cursor) {
                    if statement.kind() == "fact_statement"
                        && let Some(call) = field(statement, "call")
                    {
                        let role = osiris_statement_role(statement, call, text)?;
                        collect_osiris_call(
                            call,
                            text,
                            role,
                            OsirisCallPlacement::InitExitAction,
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
                            &mut variables,
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
        osiris: Some(OsirisFile {
            goal,
            occurrences,
            variables,
            casts,
        }),
        thoth: None,
    })
}

/// Validates the placement of known engine callables in one goal.
///
/// The generated catalog and curated event signatures are the only sources
/// strong enough to establish an engine callable's role. Unknown names remain
/// silent because they may be user-defined procedures/queries or calls
/// supplied by another Story source. The rules mirror the compiler's role
/// checks: events and databases can start an IF rule, queries can be
/// conditions, and calls can be actions.
fn collect_osiris_callable_role_issues(
    root: Node<'_>,
    text: &str,
    issues: &mut Vec<SourceIssue>,
) -> Result<(), Error> {
    let mut cursor = root.walk();
    for section in root.named_children(&mut cursor) {
        match section.kind() {
            "init_section" | "exit_section" => {
                let mut section_cursor = section.walk();
                for statement in section.named_children(&mut section_cursor) {
                    if statement.kind() != "fact_statement" {
                        continue;
                    }
                    let Some(call) = field(statement, "call") else {
                        continue;
                    };
                    validate_osiris_callable_placement(
                        call,
                        text,
                        OsirisCallPlacement::InitExitAction,
                        field(statement, "negation").is_some(),
                        issues,
                    )?;
                }
            }
            "kb_section" => {
                let mut section_cursor = section.walk();
                for rule in section.named_children(&mut section_cursor) {
                    if rule.kind() != "rule" {
                        continue;
                    }
                    let kind = field(rule, "kind")
                        .and_then(|kind| kind.utf8_text(text.as_bytes()).ok())
                        .unwrap_or("IF");
                    if let Some(head) = field(rule, "head") {
                        let placement = match kind {
                            "PROC" => OsirisCallPlacement::ProcedureHead,
                            "QRY" => OsirisCallPlacement::QueryHead,
                            _ => OsirisCallPlacement::IfHead,
                        };
                        validate_osiris_callable_placement(head, text, placement, false, issues)?;
                    }

                    let mut rule_cursor = rule.walk();
                    for child in rule.named_children(&mut rule_cursor) {
                        match child.kind() {
                            "condition" => {
                                let Some(call) = direct_child(child, "call_expression") else {
                                    continue;
                                };
                                validate_osiris_callable_placement(
                                    call,
                                    text,
                                    OsirisCallPlacement::Condition,
                                    field(child, "negation").is_some(),
                                    issues,
                                )?;
                            }
                            "action_statement" => {
                                let Some(call) = field(child, "call") else {
                                    continue;
                                };
                                validate_osiris_callable_placement(
                                    call,
                                    text,
                                    OsirisCallPlacement::RuleAction,
                                    field(child, "negation").is_some(),
                                    issues,
                                )?;
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum OsirisCallPlacement {
    InitExitAction,
    IfHead,
    ProcedureHead,
    QueryHead,
    Condition,
    RuleAction,
}

/// Returns the generated role for a known callable, including legacy event
/// aliases that remain part of the event catalog fallback.
fn known_osiris_callable_kind(name: &str, arity: u16) -> Option<OsirisContractKind> {
    if let Some(contract) = osiris_contract(OSIRIS_CONTRACTS, name, arity) {
        return Some(contract.kind);
    }
    // Do not let the legacy event aliases hide an ambiguous generated
    // declaration. A generated name/arity match that cannot be selected is
    // not strong enough evidence for a placement diagnostic.
    if OSIRIS_CONTRACTS
        .iter()
        .any(|contract| contract.name == name && contract.parameters.len() as u16 == arity)
    {
        return None;
    }
    osiris_signature(name, arity).map(|_| OsirisContractKind::Event)
}

fn validate_osiris_callable_placement(
    call: Node<'_>,
    text: &str,
    placement: OsirisCallPlacement,
    negated: bool,
    issues: &mut Vec<SourceIssue>,
) -> Result<(), Error> {
    let Some(name_node) = field(call, "name") else {
        return Ok(());
    };
    let Some(arguments) = field(call, "arguments") else {
        return Ok(());
    };
    let name = name_node.utf8_text(text.as_bytes())?;
    let arity = osiris_arity(arguments)?;
    // A PROC/QRY head declares a user callable. Unknown names are left alone;
    // a prefix such as `DB_`, `QRY_`, or `PROC_` is only a convention and does
    // not prove an engine role.
    let Some(kind) = known_osiris_callable_kind(name, arity) else {
        return Ok(());
    };

    if negated
        && matches!(
            placement,
            OsirisCallPlacement::RuleAction | OsirisCallPlacement::InitExitAction
        )
    {
        issues.push(SourceIssue {
            code: "osiris-invalid-negation".into(),
            message: format!(
                "NOT can only be applied to database facts; `{name}/{arity}` is an engine {}.",
                osiris_contract_kind_name(kind)
            ),
            range: node_range(name_node),
        });
    }

    let valid = osiris_contract_allowed_at(kind, placement);
    if valid {
        return Ok(());
    }

    let message = match placement {
        OsirisCallPlacement::IfHead => format!(
            "Osiris engine {} `{name}/{arity}` cannot trigger an IF rule; only events and databases can be the first trigger.",
            osiris_contract_kind_name(kind)
        ),
        OsirisCallPlacement::ProcedureHead => format!(
            "Osiris engine {} `{name}/{arity}` cannot be declared as a PROC.",
            osiris_contract_kind_name(kind)
        ),
        OsirisCallPlacement::QueryHead => format!(
            "Osiris engine {} `{name}/{arity}` cannot be declared as a QRY.",
            osiris_contract_kind_name(kind)
        ),
        OsirisCallPlacement::Condition => format!(
            "Osiris engine {} `{name}/{arity}` cannot be used as a condition; only queries, sysqueries, user queries, and databases are valid here.",
            osiris_contract_kind_name(kind)
        ),
        OsirisCallPlacement::RuleAction | OsirisCallPlacement::InitExitAction => format!(
            "Osiris engine {} `{name}/{arity}` cannot be used as an action; only calls, syscalls, procedures, and databases are valid here.",
            osiris_contract_kind_name(kind)
        ),
    };
    issues.push(SourceIssue {
        code: "osiris-invalid-callable-role".into(),
        message,
        range: node_range(name_node),
    });
    Ok(())
}

fn osiris_contract_allowed_at(kind: OsirisContractKind, placement: OsirisCallPlacement) -> bool {
    match placement {
        OsirisCallPlacement::IfHead => kind == OsirisContractKind::Event,
        OsirisCallPlacement::ProcedureHead | OsirisCallPlacement::QueryHead => false,
        OsirisCallPlacement::Condition => {
            matches!(
                kind,
                OsirisContractKind::Query | OsirisContractKind::Sysquery
            )
        }
        OsirisCallPlacement::RuleAction | OsirisCallPlacement::InitExitAction => {
            matches!(kind, OsirisContractKind::Call | OsirisContractKind::Syscall)
        }
    }
}

fn osiris_contract_kind_name(kind: OsirisContractKind) -> &'static str {
    match kind {
        OsirisContractKind::Call => "call",
        OsirisContractKind::Event => "event",
        OsirisContractKind::Query => "query",
        OsirisContractKind::Syscall => "syscall",
        OsirisContractKind::Sysquery => "sysquery",
    }
}

/// Returns the complete goal node represented by one Osiris parse tree.
///
/// `source_file` also accepts the standalone callable-signature form used by
/// editor tooling. The goal grammar may wrap its required sections in a
/// named `goal_file` node, while older generated parsers expose those sections
/// directly under `source_file`; accept both shapes through their structural
/// contract instead of relying on one generated node name. Syntax errors do
/// not disqualify a structurally complete goal: loose parsing must preserve
/// the recovered declarations and facts that editor features can still use.
fn complete_osiris_goal_root(root: Node<'_>) -> Option<Node<'_>> {
    if has_complete_osiris_goal_sections(root) {
        return Some(root);
    }
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .find(|child| has_complete_osiris_goal_sections(*child))
}

/// Checks the required top-level structure of a complete Osiris goal.
fn has_complete_osiris_goal_sections(node: Node<'_>) -> bool {
    [
        "version_declaration",
        "subgoal_combiner_declaration",
        "init_section",
        "kb_section",
        "exit_section",
    ]
    .into_iter()
    .all(|kind| direct_child(node, kind).is_some())
}

/// Collects verified type casts from one complete Osiris goal.
///
/// A cast is source metadata for hover. It is not a normal reference because
/// intrinsic and generated engine types have no source declaration target.
fn osiris_type_casts(root: Node<'_>, text: &str) -> Result<Vec<OsirisTypeCast>, Error> {
    let mut casts = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.kind() == "type_cast"
            && let Some(type_node) = field(node, "type")
        {
            let type_name = type_node.utf8_text(text.as_bytes())?.to_owned();
            if osiris_type_class(&type_name).is_some() {
                casts.push(OsirisTypeCast {
                    type_name,
                    range: node_range(type_node),
                });
            }
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    casts.sort_by_key(|cast| (cast.range.start.line, cast.range.start.character));
    Ok(casts)
}

/// Extracts one complete rule and its rule-local explicit variable types.
fn parse_osiris_rule(
    rule: Node<'_>,
    text: &str,
    goal: &str,
    definitions: &mut Vec<Definition>,
    references: &mut Vec<Reference>,
    occurrences: &mut Vec<OsirisDatabaseOccurrence>,
    variables: &mut Vec<OsirisVariableFact>,
) -> Result<(), Error> {
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
    let head_binds_variables = kind != "IF";
    let contract_inference =
        osiris_rule_contract_types(rule, text, &name, arguments, kind, OSIRIS_CONTRACTS)?;
    let known_engine_event = osiris_event_contract(OSIRIS_CONTRACTS, &name, arity).is_some();
    let variable_analysis = collect_osiris_variable_analysis(
        rule,
        text,
        head_binds_variables || known_engine_event,
        &contract_inference.query_output_ranges,
        &contract_inference.database_bindings,
        &contract_inference.contract_types,
    )?;
    match kind {
        "PROC" | "QRY" => {
            // The declaration label may use an unambiguous cast in the body,
            // but this rule-wide lookup is intentionally isolated from the
            // occurrence-level evidence used by DB arguments.
            let parameter_types = osiris_variable_types_in_node(rule, text)?;
            let parameters = osiris_parameter_labels(arguments, text, &parameter_types)?;
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
            OsirisCallPlacement::IfHead,
            &variable_analysis.occurrence_types,
            references,
            occurrences,
        )?,
    }

    variables.extend(variable_analysis.facts);

    let mut cursor = rule.walk();
    for child in rule.named_children(&mut cursor) {
        match child.kind() {
            "condition" => {
                if let Some(call) = direct_child(child, "call_expression") {
                    collect_osiris_call(
                        call,
                        text,
                        OsirisCallRole::Read,
                        OsirisCallPlacement::Condition,
                        &variable_analysis.occurrence_types,
                        references,
                        occurrences,
                    )?;
                }
            }
            "action_statement" => {
                if let Some(call) = field(child, "call") {
                    let role = osiris_statement_role(child, call, text)?;
                    collect_osiris_call(
                        call,
                        text,
                        role,
                        OsirisCallPlacement::RuleAction,
                        &variable_analysis.occurrence_types,
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

/// Classifies a fact/action statement while keeping negated user-database
/// facts distinct from ordinary side-effecting calls.
fn osiris_statement_role(
    statement: Node<'_>,
    call: Node<'_>,
    text: &str,
) -> Result<OsirisCallRole, Error> {
    let negated = field(statement, "negation").is_some();
    let database = field(call, "name")
        .map(|name| name.utf8_text(text.as_bytes()))
        .transpose()?
        .is_some_and(|name| name.starts_with("DB_"));
    Ok(if negated && database {
        OsirisCallRole::Remove
    } else {
        OsirisCallRole::Write
    })
}

/// The source-ordered variable facts and per-occurrence type observations for
/// one Osiris rule.
struct OsirisVariableAnalysis {
    facts: Vec<OsirisVariableFact>,
    occurrence_types: HashMap<TextRange, OsirisTypeEvidence>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OsirisUnitBindingKind {
    None,
    All,
    Database,
    Query,
}

#[derive(Clone)]
struct OsirisVariableOccurrenceState {
    binding: Option<(TextRange, Option<OsirisDatabaseBinding>)>,
    evidence: Option<OsirisTypeEvidence>,
}

struct OsirisVariableUnitContext<'a> {
    producer_ranges: &'a HashSet<TextRange>,
    query_output_ranges: &'a HashSet<TextRange>,
    database_bindings: &'a HashMap<TextRange, OsirisDatabaseBinding>,
    contract_types: &'a HashMap<TextRange, OsirisTypeEvidence>,
    text: &'a str,
}

/// Groups source-ordered local variables by name within one rule while
/// retaining the producer visible at each occurrence.
///
/// Osiris does not declare variables separately. A head variable is an
/// incoming trigger or subroutine parameter, and a variable first seen in a
/// positive DB condition is assigned by that database match. Other first
/// uses are retained as occurrences but do not receive a proven binding.
fn collect_osiris_variable_analysis(
    rule: Node<'_>,
    text: &str,
    head_binds_variables: bool,
    query_output_ranges: &[TextRange],
    database_bindings: &[(TextRange, OsirisDatabaseBinding)],
    contract_types: &HashMap<TextRange, OsirisTypeEvidence>,
) -> Result<OsirisVariableAnalysis, Error> {
    let query_output_ranges = query_output_ranges.iter().copied().collect::<HashSet<_>>();
    let database_bindings = database_bindings.iter().cloned().collect::<HashMap<_, _>>();
    let head = field(rule, "head");
    let mut current_bindings = HashMap::<String, (TextRange, Option<OsirisDatabaseBinding>)>::new();
    let mut current_types = HashMap::<String, Option<OsirisTypeEvidence>>::new();
    let mut occurrence_states = HashMap::<TextRange, OsirisVariableOccurrenceState>::new();

    // A head establishes its arguments before any condition runs. Receivers
    // are deliberately absent from the argument range set: they are inputs.
    if let Some(head) = head {
        let head_name = field(head, "name")
            .and_then(|name| name.utf8_text(text.as_bytes()).ok())
            .unwrap_or_default();
        let head_kind = if head_binds_variables {
            OsirisUnitBindingKind::All
        } else if head_name.starts_with("DB_") {
            OsirisUnitBindingKind::Database
        } else {
            OsirisUnitBindingKind::None
        };
        let producer_ranges = field(head, "arguments")
            .map(local_variable_nodes)
            .unwrap_or_default()
            .into_iter()
            .map(node_range)
            .collect::<HashSet<_>>();
        process_osiris_variable_unit(
            local_variable_nodes(head),
            head_kind,
            &OsirisVariableUnitContext {
                producer_ranges: &producer_ranges,
                query_output_ranges: &query_output_ranges,
                database_bindings: &database_bindings,
                contract_types,
                text,
            },
            &mut current_bindings,
            &mut current_types,
            &mut occurrence_states,
        )?;
    }

    // Conditions consume one result set and produce the next one. In
    // particular, a query output is not visible to another argument in that
    // same call; all new bindings merge only after the unit is processed.
    let mut cursor = rule.walk();
    for child in rule.named_children(&mut cursor) {
        if head.is_some_and(|head| node_range(head) == node_range(child)) {
            continue;
        }
        let unit_kind = if child.kind() == "condition" {
            if field(child, "negation").is_some() {
                OsirisUnitBindingKind::None
            } else if let Some(call) = direct_child(child, "call_expression") {
                let name = field(call, "name")
                    .map(|name| name.utf8_text(text.as_bytes()))
                    .transpose()?;
                if name.is_some_and(|name| name.starts_with("DB_")) {
                    OsirisUnitBindingKind::Database
                } else if local_variable_nodes(child)
                    .iter()
                    .any(|variable| query_output_ranges.contains(&node_range(*variable)))
                {
                    OsirisUnitBindingKind::Query
                } else {
                    OsirisUnitBindingKind::None
                }
            } else {
                OsirisUnitBindingKind::None
            }
        } else {
            OsirisUnitBindingKind::None
        };
        let producer_ranges = if unit_kind == OsirisUnitBindingKind::Database {
            direct_child(child, "call_expression")
                .and_then(|call| field(call, "arguments"))
                .map(local_variable_nodes)
                .unwrap_or_default()
                .into_iter()
                .map(node_range)
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        process_osiris_variable_unit(
            local_variable_nodes(child),
            unit_kind,
            &OsirisVariableUnitContext {
                producer_ranges: &producer_ranges,
                query_output_ranges: &query_output_ranges,
                database_bindings: &database_bindings,
                contract_types,
                text,
            },
            &mut current_bindings,
            &mut current_types,
            &mut occurrence_states,
        )?;
    }

    let mut variables = local_variable_nodes(rule);
    variables.sort_by_key(|node| node.start_byte());
    let mut occurrence_types = HashMap::new();
    let mut facts = Vec::new();
    for variable in variables {
        let name = variable.utf8_text(text.as_bytes())?.to_owned();
        let range = node_range(variable);
        let state = occurrence_states
            .entry(range)
            .or_insert_with(|| OsirisVariableOccurrenceState {
                binding: None,
                evidence: osiris_variable_cast_evidence(variable, text).ok().flatten(),
            })
            .clone();
        if let Some(evidence) = state.evidence.clone() {
            occurrence_types.insert(range, evidence);
        }
        let occurrence_fact = OsirisVariableOccurrence {
            range,
            binding_range: state.binding.as_ref().map(|(range, _)| *range),
            database_binding: state.binding.and_then(|(_, binding)| binding),
            evidence: state.evidence,
        };
        if let Some(fact) = facts
            .iter_mut()
            .find(|fact: &&mut OsirisVariableFact| fact.name == name)
        {
            fact.occurrences.push(range);
            if fact.binding_range.is_none() {
                fact.binding_range = occurrence_fact.binding_range;
                fact.database_binding = occurrence_fact.database_binding.clone();
            }
            if fact.evidence.is_none() {
                fact.evidence = occurrence_fact.evidence.clone();
            }
            fact.occurrence_facts.push(occurrence_fact);
        } else {
            facts.push(OsirisVariableFact {
                rule_range: node_range(rule),
                name: name.clone(),
                occurrences: vec![range],
                binding_range: occurrence_fact.binding_range,
                database_binding: occurrence_fact.database_binding.clone(),
                evidence: occurrence_fact.evidence.clone(),
                occurrence_facts: vec![occurrence_fact],
            });
        }
    }
    Ok(OsirisVariableAnalysis {
        facts,
        occurrence_types,
    })
}

fn process_osiris_variable_unit(
    mut variables: Vec<Node<'_>>,
    binding_kind: OsirisUnitBindingKind,
    context: &OsirisVariableUnitContext<'_>,
    current_bindings: &mut HashMap<String, (TextRange, Option<OsirisDatabaseBinding>)>,
    current_types: &mut HashMap<String, Option<OsirisTypeEvidence>>,
    occurrence_states: &mut HashMap<TextRange, OsirisVariableOccurrenceState>,
) -> Result<(), Error> {
    variables.sort_by_key(|node| node.start_byte());
    let previous_bindings = current_bindings.clone();
    let previous_types = current_types.clone();
    let repeats_share_producer = matches!(
        binding_kind,
        OsirisUnitBindingKind::All | OsirisUnitBindingKind::Database
    );
    let mut new_bindings = HashMap::<String, (TextRange, Option<OsirisDatabaseBinding>)>::new();
    let mut type_updates = Vec::new();

    for variable in variables {
        let name = variable.utf8_text(context.text.as_bytes())?.to_owned();
        let range = node_range(variable);
        let was_bound = previous_bindings.contains_key(&name);
        let is_producer = match binding_kind {
            OsirisUnitBindingKind::None => false,
            OsirisUnitBindingKind::All | OsirisUnitBindingKind::Database => {
                context.producer_ranges.contains(&range)
            }
            OsirisUnitBindingKind::Query => context.query_output_ranges.contains(&range),
        } && !was_bound;
        let database_binding = context.database_bindings.get(&range).cloned();
        let binding = previous_bindings.get(&name).cloned().or_else(|| {
            if is_producer {
                new_bindings
                    .get(&name)
                    .cloned()
                    .or(Some((range, database_binding.clone())))
            } else if repeats_share_producer {
                new_bindings.get(&name).cloned()
            } else {
                None
            }
        });
        if is_producer {
            new_bindings
                .entry(name.clone())
                .or_insert_with(|| (range, database_binding));
        }

        let explicit = osiris_variable_cast_evidence(variable, context.text)?;
        // A query output that was already bound before this condition is a
        // filter. Its catalog type cannot overwrite the existing value type.
        let contract_type = if context.query_output_ranges.contains(&range) && !is_producer {
            None
        } else {
            context.contract_types.get(&range).cloned()
        };
        let evidence = explicit.clone().or(contract_type.clone()).or_else(|| {
            previous_types
                .get(&name)
                .and_then(|evidence| evidence.clone())
        });
        if let Some(type_evidence) = explicit.or(contract_type) {
            type_updates.push((name, type_evidence));
        }
        occurrence_states.insert(range, OsirisVariableOccurrenceState { binding, evidence });
    }

    for (name, binding) in new_bindings {
        current_bindings.entry(name).or_insert(binding);
    }
    for (name, evidence) in type_updates {
        update_current_type(current_types, &name, Some(evidence));
    }
    Ok(())
}

fn local_variable_nodes(node: Node<'_>) -> Vec<Node<'_>> {
    let mut variables = Vec::new();
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        if node.kind() == "local_variable" {
            variables.push(node);
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    variables.sort_by_key(|node| node.start_byte());
    variables
}

fn osiris_variable_cast_evidence(
    variable: Node<'_>,
    text: &str,
) -> Result<Option<OsirisTypeEvidence>, Error> {
    let Some(parent) = variable.parent() else {
        return Ok(None);
    };
    if parent.kind() != "typed_variable" {
        return Ok(None);
    }
    let Some(cast) = field(parent, "cast") else {
        return Ok(None);
    };
    let Some(type_node) = field(cast, "type") else {
        return Ok(None);
    };
    Ok(Some(OsirisTypeEvidence {
        type_name: type_node.utf8_text(text.as_bytes())?.to_owned(),
        source_range: node_range(type_node),
        origin: OsirisEvidenceOrigin::Explicit,
    }))
}

fn update_current_type(
    current_types: &mut HashMap<String, Option<OsirisTypeEvidence>>,
    name: &str,
    evidence: Option<OsirisTypeEvidence>,
) {
    current_types
        .entry(name.to_owned())
        .and_modify(|current| {
            if current
                .as_ref()
                .zip(evidence.as_ref())
                .is_some_and(|(current, evidence)| current.type_name != evidence.type_name)
            {
                *current = None;
            }
        })
        .or_insert(evidence);
}

/// Adds one call reference and retains structured evidence for user databases.
fn collect_osiris_call(
    call: Node<'_>,
    text: &str,
    role: OsirisCallRole,
    placement: OsirisCallPlacement,
    occurrence_types: &HashMap<TextRange, OsirisTypeEvidence>,
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
            OsirisCallRole::Remove => "osiris-remove",
        }
        .into(),
    });

    let contract = osiris_contract(OSIRIS_CONTRACTS, &name, arity)
        .filter(|contract| osiris_contract_allowed_at(contract.kind, placement));
    let mut argument_cursor = arguments_node.walk();
    for (index, argument) in arguments_node
        .named_children(&mut argument_cursor)
        .enumerate()
    {
        let Some(contract) = contract else {
            break;
        };
        let Some(domain) = osiris_argument_domain(contract.kind, &name, arity, index) else {
            continue;
        };
        let Some(value) = field(argument, "value") else {
            continue;
        };
        if value.kind() != "string_literal" {
            continue;
        }
        let Some(content) = field(value, "content") else {
            continue;
        };
        references.push(Reference {
            target: SymbolTarget::Named {
                kind: Some(domain.into()),
                name: content.utf8_text(text.as_bytes())?.to_owned(),
            },
            range: node_range(content),
            context: "osiris-string-literal".into(),
        });
    }

    if !name.starts_with("DB_") {
        return Ok(());
    }

    let mut arguments = Vec::new();
    let mut cursor = arguments_node.walk();
    for argument in arguments_node.named_children(&mut cursor) {
        arguments.push(OsirisArgument {
            range: node_range(argument),
            evidence: osiris_argument_occurrence_evidence(argument, text, occurrence_types)?,
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
            OsirisCallRole::Remove => {}
        }
        if occurrence.role == OsirisCallRole::Write {
            for (column, argument) in database.types.iter_mut().zip(&occurrence.arguments) {
                if let Some(evidence) = &argument.evidence {
                    column.insert(evidence.type_name.clone());
                }
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
fn osiris_variable_types_in_node(
    node: Node<'_>,
    text: &str,
) -> Result<HashMap<String, OsirisTypeEvidence>, Error> {
    let mut candidates = HashMap::<String, Option<OsirisTypeEvidence>>::new();
    let mut pending = vec![node];
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
                origin: OsirisEvidenceOrigin::Explicit,
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
            origin: OsirisEvidenceOrigin::Explicit,
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
        origin: OsirisEvidenceOrigin::Explicit,
    }))
}

/// Returns the type observation available at one source occurrence. Unlike
/// the rule-wide parameter-label map, this cannot inherit a later cast.
fn osiris_argument_occurrence_evidence(
    argument: Node<'_>,
    text: &str,
    occurrence_types: &HashMap<TextRange, OsirisTypeEvidence>,
) -> Result<Option<OsirisTypeEvidence>, Error> {
    if let Some(cast) = field(argument, "cast")
        && let Some(type_node) = field(cast, "type")
    {
        return Ok(Some(OsirisTypeEvidence {
            type_name: type_node.utf8_text(text.as_bytes())?.to_owned(),
            source_range: node_range(type_node),
            origin: OsirisEvidenceOrigin::Explicit,
        }));
    }
    let Some(value) = field(argument, "value") else {
        return Ok(None);
    };
    if value.kind() == "local_variable" {
        return Ok(occurrence_types.get(&node_range(value)).cloned());
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
        origin: OsirisEvidenceOrigin::Explicit,
    }))
}

/// A generated engine contract, or the legacy event-only aliases retained
/// until the generated catalog is populated.
#[derive(Clone, Copy)]
enum OsirisContractView<'a> {
    Generated(&'a crate::osiris_catalog::OsirisContractSpec),
    Legacy(crate::catalog::OsirisSignature),
}

/// Contract-derived facts collected while walking one Osiris rule.
struct OsirisContractInference {
    query_output_ranges: Vec<TextRange>,
    database_bindings: Vec<(TextRange, OsirisDatabaseBinding)>,
    contract_types: HashMap<TextRange, OsirisTypeEvidence>,
}

fn osiris_contract_view<'a>(
    contracts: &'a [crate::osiris_catalog::OsirisContractSpec],
    name: &str,
    arity: u16,
) -> Option<OsirisContractView<'a>> {
    osiris_contract(contracts, name, arity)
        .map(OsirisContractView::Generated)
        .or_else(|| osiris_signature(name, arity).map(OsirisContractView::Legacy))
}

fn osiris_event_contract<'a>(
    contracts: &'a [crate::osiris_catalog::OsirisContractSpec],
    name: &str,
    arity: u16,
) -> Option<OsirisContractView<'a>> {
    match osiris_contract(contracts, name, arity) {
        Some(contract) if contract.kind == OsirisContractKind::Event => {
            Some(OsirisContractView::Generated(contract))
        }
        Some(_) => None,
        None => osiris_signature(name, arity).map(OsirisContractView::Legacy),
    }
}

/// Collects generated-contract type evidence from actual rule producers.
/// Known event heads provide all their arguments. Positive generated query and
/// sysquery conditions provide only their `[out]` parameters. Input-only
/// arguments and action calls are constraints or side effects, so they never
/// establish a local variable here.
fn osiris_rule_contract_types(
    rule: Node<'_>,
    text: &str,
    head_name: &str,
    head_arguments: Node<'_>,
    kind: &str,
    contracts: &[crate::osiris_catalog::OsirisContractSpec],
) -> Result<OsirisContractInference, Error> {
    let mut output_ranges = Vec::new();
    let mut database_bindings = Vec::new();
    let mut contract_types = HashMap::new();

    if kind == "IF" && head_name.starts_with("DB_") {
        let arity = osiris_arity(head_arguments)?;
        let mut argument_cursor = head_arguments.walk();
        for (column, argument) in head_arguments
            .named_children(&mut argument_cursor)
            .enumerate()
        {
            let mut pending = vec![argument];
            while let Some(node) = pending.pop() {
                if node.kind() == "local_variable" {
                    database_bindings.push((
                        node_range(node),
                        OsirisDatabaseBinding {
                            name: head_name.to_owned(),
                            arity,
                            column: u16::try_from(column).unwrap_or(u16::MAX),
                        },
                    ));
                }
                let mut node_cursor = node.walk();
                pending.extend(node.named_children(&mut node_cursor));
            }
        }
    }

    if kind == "IF"
        && let Some(contract) =
            osiris_event_contract(contracts, head_name, osiris_arity(head_arguments)?)
    {
        add_osiris_contract_arguments(
            contract,
            head_arguments,
            text,
            false,
            &mut output_ranges,
            &mut contract_types,
        )?;
    }

    let mut cursor = rule.walk();
    for child in rule.named_children(&mut cursor) {
        let (call, positive) = match child.kind() {
            "condition" => (
                direct_child(child, "call_expression"),
                field(child, "negation").is_none(),
            ),
            _ => (None, false),
        };
        let Some(call) = call else {
            continue;
        };
        let Some(name) = field(call, "name") else {
            continue;
        };
        let Some(arguments) = field(call, "arguments") else {
            continue;
        };
        let name = name.utf8_text(text.as_bytes())?;
        if child.kind() == "condition" && positive && name.starts_with("DB_") {
            let arity = osiris_arity(arguments)?;
            let mut argument_cursor = arguments.walk();
            for (column, argument) in arguments.named_children(&mut argument_cursor).enumerate() {
                let mut pending = vec![argument];
                while let Some(node) = pending.pop() {
                    if node.kind() == "local_variable" {
                        database_bindings.push((
                            node_range(node),
                            OsirisDatabaseBinding {
                                name: name.to_owned(),
                                arity,
                                column: u16::try_from(column).unwrap_or(u16::MAX),
                            },
                        ));
                    }
                    let mut node_cursor = node.walk();
                    pending.extend(node.named_children(&mut node_cursor));
                }
            }
        }
        let Some(contract) = osiris_contract_view(contracts, name, osiris_arity(arguments)?) else {
            continue;
        };
        let binds_query_outputs = positive
            && matches!(
                contract,
                OsirisContractView::Generated(contract)
                    if matches!(
                        contract.kind,
                        OsirisContractKind::Query | OsirisContractKind::Sysquery
                    )
            );
        if !binds_query_outputs {
            continue;
        }
        add_osiris_contract_arguments(
            contract,
            arguments,
            text,
            true,
            &mut output_ranges,
            &mut contract_types,
        )?;
    }

    Ok(OsirisContractInference {
        query_output_ranges: output_ranges,
        database_bindings,
        contract_types,
    })
}

fn add_osiris_contract_arguments(
    contract: OsirisContractView<'_>,
    arguments: Node<'_>,
    _text: &str,
    binds_query_outputs: bool,
    output_ranges: &mut Vec<TextRange>,
    contract_types: &mut HashMap<TextRange, OsirisTypeEvidence>,
) -> Result<(), Error> {
    let mut cursor = arguments.walk();
    for (index, argument) in arguments.named_children(&mut cursor).enumerate() {
        let (type_name, direction) = match contract {
            OsirisContractView::Generated(contract) => {
                let Some(parameter) = contract.parameters.get(index) else {
                    break;
                };
                (parameter.type_name, Some(parameter.direction))
            }
            OsirisContractView::Legacy(signature) => {
                let Some(type_name) = signature.get(index) else {
                    break;
                };
                (*type_name, None)
            }
        };
        let Some(value) = field(argument, "value") else {
            continue;
        };
        if binds_query_outputs
            && !matches!(
                direction,
                Some(OsirisParameterDirection::InOut | OsirisParameterDirection::Out)
            )
        {
            continue;
        }
        if value.kind() != "local_variable" {
            continue;
        }
        let range = node_range(value);
        let is_output = binds_query_outputs
            && matches!(
                direction,
                Some(OsirisParameterDirection::InOut | OsirisParameterDirection::Out)
            );
        // A cast changes type evidence, not whether a contract [out] value
        // binds the local. Record the range before skipping derived evidence.
        if is_output {
            output_ranges.push(range);
        }
        if field(argument, "cast").is_some() {
            continue;
        }
        let evidence = OsirisTypeEvidence {
            type_name: type_name.to_owned(),
            source_range: range,
            origin: OsirisEvidenceOrigin::Engine,
        };
        if !binds_query_outputs || is_output {
            contract_types.insert(range, evidence);
        }
    }
    Ok(())
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
            "identifier"
                if !is_function_name(node)
                    // Prefix and bracket-group identifiers select an execution
                    // context; only the wrapped statements can name declarations.
                    && node.parent().is_none_or(|parent| {
                        parent.kind() != "prefixed_expression"
                            && parent.kind() != "bracket_group"
                    }) =>
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
                    let parameter = form.parameters.get(index)?;
                    // Expression parameters hold Stats expressions whose bare
                    // identifiers stay ordinary references.
                    if parameter.expression {
                        return None;
                    }
                    return Some(parameter.kind.map(str::to_owned));
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

/// Returns the first direct named child regardless of its expression subtype.
fn first_named_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
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

/// Returns whether one legacy Stats value parses completely and carries
/// expression structure beyond a bare constant.
///
/// Values that are only identifiers, numbers, handles, or UUIDs stay false so
/// editor previews can reserve fenced blocks for genuine functor, condition,
/// dice, and resource expressions.
pub fn is_structural_stats_value(value: &str) -> bool {
    const STRUCTURAL: &[&str] = &[
        "call_expression",
        "prefixed_expression",
        "bracket_group",
        "if_expression",
        "binary_expression",
        "unary_expression",
        "member_expression",
        "resource_expression",
        "dice_literal",
        "list_literal",
        "parenthesized_expression",
    ];
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bg3::BG3_STATS_VALUE_LANGUAGE.into())
        .is_err()
    {
        return false;
    }
    let Some(tree) = parser.parse(value, None) else {
        return false;
    };
    if tree.root_node().has_error() {
        return false;
    }
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if STRUCTURAL.contains(&node.kind()) {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_contracts_supply_event_and_query_variable_semantics() {
        let text = concat!(
            "Version 1\n",
            "SubGoalCombiner SGC_AND\n",
            "INITSECTION\n",
            "KBSECTION\n",
            "IF\n",
            "CastedSpell((CHARACTER)_Caster, _Spell, \"Type\", \"Element\", 1)\n",
            "AND\n",
            "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _Bonus)\n",
            "AND\n",
            "NOT\n",
            "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _Negated)\n",
            "AND\n",
            "AddActionPoints(_CallArg, 1)\n",
            "THEN\n",
            "GetActionResourceValuePersonal(_ActionCaster, \"BonusActionPoint\", 0, _ActionOutput);\n",
            "DB_Result(_Caster, _Bonus, _Negated, _CallArg);\n",
            "EXITSECTION\n",
            "ENDEXITSECTION\n",
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bg3::BG3_OSIRIS_LANGUAGE.into())
            .expect("Osiris grammar");
        let tree = parser.parse(text, None).expect("tree");
        let goal = tree
            .root_node()
            .named_children(&mut tree.root_node().walk())
            .find(|node| node.kind() == "goal_file")
            .expect("goal file");
        let rule = goal
            .named_children(&mut goal.walk())
            .find(|node| node.kind() == "kb_section")
            .and_then(|section| {
                section
                    .named_children(&mut section.walk())
                    .find(|node| node.kind() == "rule")
            })
            .expect("rule");
        let head = field(rule, "head").expect("head");
        let name = field(head, "name")
            .expect("head name")
            .utf8_text(text.as_bytes())
            .expect("head name text");
        let arguments = field(head, "arguments").expect("head arguments");
        let inference =
            osiris_rule_contract_types(rule, text, name, arguments, "IF", OSIRIS_CONTRACTS)
                .expect("contract evidence");
        let facts = collect_osiris_variable_analysis(
            rule,
            text,
            true,
            &inference.query_output_ranges,
            &inference.database_bindings,
            &inference.contract_types,
        )
        .expect("variable facts")
        .facts;
        let fact = |name: &str| facts.iter().find(|fact| fact.name == name).expect(name);

        assert_eq!(
            fact("_Caster").binding_range,
            Some(fact("_Caster").occurrences[0])
        );
        assert_eq!(
            fact("_Caster").evidence.as_ref().unwrap().type_name,
            "CHARACTER"
        );
        assert_eq!(
            fact("_Caster").evidence.as_ref().unwrap().origin,
            OsirisEvidenceOrigin::Explicit
        );
        assert_eq!(
            fact("_Bonus").binding_range,
            Some(fact("_Bonus").occurrences[0])
        );
        assert_eq!(fact("_Bonus").evidence.as_ref().unwrap().type_name, "REAL");
        assert_eq!(fact("_Negated").binding_range, None);
        assert!(fact("_Negated").evidence.is_none());
        assert_eq!(fact("_CallArg").binding_range, None);
        assert!(fact("_CallArg").evidence.is_none());
        assert_eq!(fact("_ActionOutput").binding_range, None);
        assert!(fact("_ActionOutput").evidence.is_none());
    }

    #[test]
    fn validates_known_osiris_callable_roles_and_negation() {
        let text = concat!(
            "Version 1\n",
            "SubGoalCombiner SGC_AND\n",
            "INITSECTION\n",
            "HasPassive(11111111-1111-1111-1111-111111111111, \"P\", 1);\n",
            "KBSECTION\n",
            "IF\n",
            "HasPassive(_Actor, \"P\", 1)\n",
            "AND\n",
            "Died(_Actor)\n",
            "AND\n",
            "AddActionPoints(_Actor, 1)\n",
            "AND\n",
            "NOT HasPassive(_Actor, \"P\", 1)\n",
            "THEN\n",
            "HasPassive(_Actor, \"P\", 1);\n",
            "NOT AddActionPoints(_Actor, 1);\n",
            "IF\n",
            "Died(_Actor)\n",
            "THEN\n",
            "AddActionPoints(_Actor, 1);\n",
            "PROC\n",
            "Died(_Actor)\n",
            "THEN\n",
            "AddActionPoints(_Actor, 1);\n",
            "QRY\n",
            "AddActionPoints(_Actor, 1)\n",
            "THEN\n",
            "DB_NOOP(1);\n",
            "IF\n",
            "QRY_User(_Actor)\n",
            "AND\n",
            "UnknownCondition(_Actor)\n",
            "THEN\n",
            "UnknownAction(_Actor);\n",
            "PROC\n",
            "PROC_User(_Actor)\n",
            "AND\n",
            "UnknownCondition(_Actor)\n",
            "THEN\n",
            "UnknownAction(_Actor);\n",
            "QRY\n",
            "QRY_User(_Actor)\n",
            "AND\n",
            "UnknownCondition(_Actor)\n",
            "THEN\n",
            "DB_NOOP(1);\n",
            "IF\n",
            "DB_User(_Actor)\n",
            "THEN\n",
            "NOT DB_User(_Actor);\n",
            "EXITSECTION\n",
            "NOT HasPassive(11111111-1111-1111-1111-111111111111, \"P\", 1);\n",
            "ENDEXITSECTION\n",
        );
        let parsed = parse_source(
            SourceFile {
                path: Path::new("Mods/MyMod/Story/RawFiles/Goals/Roles.txt").into(),
                kind: SourceKind::Osiris,
            },
            text,
            &SchemaCatalog::default(),
            "English",
        )
        .expect("valid Osiris source");

        let role_issues = parsed
            .issues
            .iter()
            .filter(|issue| issue.code == "osiris-invalid-callable-role")
            .collect::<Vec<_>>();
        assert_eq!(role_issues.len(), 8);
        assert!(role_issues.iter().any(|issue| {
            issue
                .message
                .contains("only events and databases can be the first trigger")
        }));
        assert!(role_issues.iter().any(|issue| {
            issue
                .message
                .contains("cannot be used as a condition; only queries, sysqueries, user queries, and databases")
        }));
        assert!(role_issues.iter().any(|issue| {
            issue.message.contains(
                "cannot be used as an action; only calls, syscalls, procedures, and databases",
            )
        }));
        assert_eq!(
            parsed
                .issues
                .iter()
                .filter(|issue| issue.code == "osiris-invalid-negation")
                .count(),
            2
        );
        assert!(parsed.issues.iter().all(|issue| {
            issue.code == "osiris-invalid-callable-role" || issue.code == "osiris-invalid-negation"
        }));
    }

    #[test]
    fn positive_inout_query_conditions_produce_variable_evidence() {
        static PARAMETERS: &[crate::osiris_catalog::OsirisParameterSpec] =
            &[crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::InOut,
                type_name: "INTEGER",
                name: "_Value",
            }];
        static CONTRACTS: &[crate::osiris_catalog::OsirisContractSpec] =
            &[crate::osiris_catalog::OsirisContractSpec {
                kind: OsirisContractKind::Query,
                name: "InOutQuery",
                parameters: PARAMETERS,
            }];
        let text = concat!(
            "Version 1\n",
            "SubGoalCombiner SGC_AND\n",
            "INITSECTION\n",
            "KBSECTION\n",
            "IF\n",
            "UnknownEvent()\n",
            "AND\n",
            "InOutQuery(_Value)\n",
            "THEN\n",
            "DB_Result(_Value);\n",
            "EXITSECTION\n",
            "ENDEXITSECTION\n",
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bg3::BG3_OSIRIS_LANGUAGE.into())
            .expect("Osiris grammar");
        let tree = parser.parse(text, None).expect("tree");
        let goal = tree
            .root_node()
            .named_children(&mut tree.root_node().walk())
            .find(|node| node.kind() == "goal_file")
            .expect("goal file");
        let rule = goal
            .named_children(&mut goal.walk())
            .find(|node| node.kind() == "kb_section")
            .and_then(|section| {
                section
                    .named_children(&mut section.walk())
                    .find(|node| node.kind() == "rule")
            })
            .expect("rule");
        let head = field(rule, "head").expect("head");
        let name = field(head, "name")
            .expect("head name")
            .utf8_text(text.as_bytes())
            .expect("head name text");
        let arguments = field(head, "arguments").expect("head arguments");
        let inference = osiris_rule_contract_types(rule, text, name, arguments, "IF", CONTRACTS)
            .expect("contract evidence");
        let facts = collect_osiris_variable_analysis(
            rule,
            text,
            false,
            &inference.query_output_ranges,
            &inference.database_bindings,
            &inference.contract_types,
        )
        .expect("variable facts")
        .facts;
        let value = facts
            .iter()
            .find(|fact| fact.name == "_Value")
            .expect("value fact");
        assert_eq!(value.binding_range, Some(value.occurrences[0]));
        assert_eq!(value.evidence.as_ref().unwrap().type_name, "INTEGER");
        assert_eq!(
            value.evidence.as_ref().unwrap().origin,
            OsirisEvidenceOrigin::Engine
        );
        assert_eq!(inference.query_output_ranges, vec![value.occurrences[0]]);
    }

    #[test]
    fn query_outputs_bind_only_after_their_condition() {
        static PARAMETERS: &[crate::osiris_catalog::OsirisParameterSpec] = &[
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_First",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_Second",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::Out,
                type_name: "INTEGER",
                name: "_Result",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_Fourth",
            },
        ];
        static INOUT_PARAMETERS: &[crate::osiris_catalog::OsirisParameterSpec] = &[
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_First",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_Second",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::InOut,
                type_name: "INTEGER",
                name: "_Result",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_Fourth",
            },
        ];
        static MULTI_OUTPUT_PARAMETERS: &[crate::osiris_catalog::OsirisParameterSpec] = &[
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::Out,
                type_name: "REAL",
                name: "_X",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::Out,
                type_name: "REAL",
                name: "_Y",
            },
            crate::osiris_catalog::OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "GUIDSTRING",
                name: "_Target",
            },
        ];
        static CONTRACTS: &[crate::osiris_catalog::OsirisContractSpec] = &[
            crate::osiris_catalog::OsirisContractSpec {
                kind: OsirisContractKind::Query,
                name: "GetPositionStyle",
                parameters: MULTI_OUTPUT_PARAMETERS,
            },
            crate::osiris_catalog::OsirisContractSpec {
                kind: OsirisContractKind::Query,
                name: "InOutTemplateIsInPartyInventory",
                parameters: INOUT_PARAMETERS,
            },
            crate::osiris_catalog::OsirisContractSpec {
                kind: OsirisContractKind::Query,
                name: "TemplateIsInPartyInventory",
                parameters: PARAMETERS,
            },
        ];
        let text = concat!(
            "Version 1\n",
            "SubGoalCombiner SGC_AND\n",
            "INITSECTION\n",
            "KBSECTION\n",
            "IF\n",
            "UnknownEvent()\n",
            "AND\n",
            "TemplateIsInPartyInventory(_X, _X, _X, _X)\n",
            "AND\n",
            "InOutTemplateIsInPartyInventory(_Y, _Y, _Y, _Y)\n",
            "AND\n",
            "GetPositionStyle(_Z, _Z, _Z)\n",
            "THEN\n",
            "DB_Result(_X, _Y, _Z);\n",
            "EXITSECTION\n",
            "ENDEXITSECTION\n",
        );
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bg3::BG3_OSIRIS_LANGUAGE.into())
            .expect("Osiris grammar");
        let tree = parser.parse(text, None).expect("tree");
        let goal = tree
            .root_node()
            .named_children(&mut tree.root_node().walk())
            .find(|node| node.kind() == "goal_file")
            .expect("goal file");
        let rule = goal
            .named_children(&mut goal.walk())
            .find(|node| node.kind() == "kb_section")
            .and_then(|section| {
                section
                    .named_children(&mut section.walk())
                    .find(|node| node.kind() == "rule")
            })
            .expect("rule");
        let head = field(rule, "head").expect("head");
        let name = field(head, "name")
            .expect("head name")
            .utf8_text(text.as_bytes())
            .expect("head name text");
        let arguments = field(head, "arguments").expect("head arguments");
        let inference = osiris_rule_contract_types(rule, text, name, arguments, "IF", CONTRACTS)
            .expect("contract evidence");
        let facts = collect_osiris_variable_analysis(
            rule,
            text,
            false,
            &inference.query_output_ranges,
            &inference.database_bindings,
            &inference.contract_types,
        )
        .expect("variable facts")
        .facts;
        for name in ["_X", "_Y"] {
            let fact = facts.iter().find(|fact| fact.name == name).expect(name);
            assert_eq!(fact.occurrence_facts.len(), 5);
            assert_eq!(fact.occurrence_facts[0].binding_range, None);
            assert_eq!(fact.occurrence_facts[1].binding_range, None);
            assert_eq!(
                fact.occurrence_facts[2].binding_range,
                Some(fact.occurrences[2])
            );
            assert_eq!(fact.occurrence_facts[3].binding_range, None);
            assert_eq!(
                fact.occurrence_facts[4].binding_range,
                Some(fact.occurrences[2])
            );
            assert_eq!(
                fact.occurrence_facts[2]
                    .evidence
                    .as_ref()
                    .expect("query output type")
                    .type_name,
                "INTEGER"
            );
        }

        let multi_output = facts
            .iter()
            .find(|fact| fact.name == "_Z")
            .expect("multi-output variable");
        assert_eq!(multi_output.occurrence_facts.len(), 4);
        assert_eq!(
            multi_output.occurrence_facts[0].binding_range,
            Some(multi_output.occurrences[0])
        );
        assert_eq!(
            multi_output.occurrence_facts[1].binding_range,
            Some(multi_output.occurrences[0])
        );
        assert_eq!(multi_output.occurrence_facts[2].binding_range, None);
        assert_eq!(
            multi_output.occurrence_facts[3].binding_range,
            Some(multi_output.occurrences[0])
        );
    }
}
