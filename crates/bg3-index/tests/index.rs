use std::fs;
use std::path::{Path, PathBuf};

use bg3_index::{
    CacheStore, ModuleIndex, ModuleRole, ModuleSpec, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND,
    OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND, OsirisCallRole, OsirisEvidenceOrigin, SchemaCatalog,
    SourceFile, SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, ThothBinaryOperator,
    ThothExpressionKind, ThothIfBranchKind, ThothLiteralKind, ThothMemberAccessKind,
    ThothParameter, ThothScopeId, ThothUnaryOperator, discover_module, is_structural_stats_value,
    parse_source, parse_thoth_file, parse_tooltip_catalog, read_base_localization_package,
    read_base_tooltip_catalog, read_localization_package, source_kind_for_document,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

fn load_schema(root: &Path) -> SchemaCatalog {
    let mut schema = SchemaCatalog::default();
    for relative in [
        "game/Editor/Config/Stats/StatObjectDefinitions.sod",
        "game/Editor/Config/UuidObjects/TableDefinitions.sod",
    ] {
        schema
            .merge_definitions(&fs::read_to_string(root.join(relative)).unwrap())
            .unwrap();
    }
    for relative in [
        "game/Editor/Config/Stats/Enumerations.xml",
        "game/Editor/Config/UuidObjects/Enumerations.toe",
    ] {
        schema
            .merge_enumerations(&fs::read_to_string(root.join(relative)).unwrap())
            .unwrap();
    }
    schema
}

fn synthetic_loca(entries: &[(&str, u16, &str)]) -> Vec<u8> {
    let table_size = 12 + entries.len() * 70;
    let mut bytes = Vec::with_capacity(
        table_size
            + entries
                .iter()
                .map(|(_, _, text)| text.len() + 1)
                .sum::<usize>(),
    );
    bytes.extend_from_slice(b"LOCA");
    bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(table_size).unwrap().to_le_bytes());
    for (handle, version, text) in entries {
        let mut key = [0_u8; 64];
        key[..handle.len()].copy_from_slice(handle.as_bytes());
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(text.len() + 1).unwrap().to_le_bytes());
    }
    for (_, _, text) in entries {
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn synthetic_package(language: &str, loca: &[u8], compression: u8) -> Vec<u8> {
    synthetic_package_entry(
        &format!(
            "Localization/{language}/{}.loca",
            language.to_ascii_lowercase()
        ),
        loca,
        compression,
    )
}

fn synthetic_package_entry(name: &str, contents: &[u8], compression: u8) -> Vec<u8> {
    let stored = match compression {
        0 => contents.to_vec(),
        2 => lz4_flex::block::compress(contents),
        _ => contents.to_vec(),
    };
    let mut entry = vec![0_u8; 272];
    entry[..name.len()].copy_from_slice(name.as_bytes());
    entry[256..260].copy_from_slice(&40_u32.to_le_bytes());
    entry[263] = compression;
    entry[264..268].copy_from_slice(&u32::try_from(stored.len()).unwrap().to_le_bytes());
    let uncompressed = if compression == 0 { 0 } else { contents.len() };
    entry[268..272].copy_from_slice(&u32::try_from(uncompressed).unwrap().to_le_bytes());

    let compressed_list = lz4_flex::block::compress(&entry);
    let mut file_list = Vec::with_capacity(8 + compressed_list.len());
    file_list.extend_from_slice(&1_u32.to_le_bytes());
    file_list.extend_from_slice(&u32::try_from(compressed_list.len()).unwrap().to_le_bytes());
    file_list.extend_from_slice(&compressed_list);

    let file_list_offset = 40 + stored.len();
    let mut package = Vec::with_capacity(file_list_offset + file_list.len());
    package.extend_from_slice(b"LSPK");
    package.extend_from_slice(&18_u32.to_le_bytes());
    package.extend_from_slice(&u64::try_from(file_list_offset).unwrap().to_le_bytes());
    package.extend_from_slice(&u32::try_from(file_list.len()).unwrap().to_le_bytes());
    package.push(0);
    package.push(0);
    package.extend_from_slice(&[0_u8; 16]);
    package.extend_from_slice(&1_u16.to_le_bytes());
    package.extend_from_slice(&stored);
    package.extend_from_slice(&file_list);
    package
}

#[test]
fn loads_schema_metadata_and_enumerations() {
    let root = fixtures();
    let schema = load_schema(&root);
    let passive = &schema.by_id["11111111-1111-1111-1111-111111111111"];
    let resource = &schema.by_id["44444444-4444-4444-4444-444444444444"];

    assert_eq!(passive.export_type.as_deref(), Some("PassiveData"));
    assert_eq!(
        passive.fields["Boosts"].description.as_deref(),
        Some("Passive effects")
    );
    assert!(resource.fields["Name"].auto_generated);
    assert_eq!(schema.enumerations["YesNo"], ["Default", "No", "Yes"]);
}

#[test]
fn infers_legacy_schemas_from_type_discriminators() {
    let root = fixtures();
    let schema = load_schema(&root);
    let status_fields = std::collections::BTreeMap::from([("StatusType".into(), "BOOST".into())]);
    let status = schema.infer_legacy(Path::new("Status.txt"), Some("StatusData"), &status_fields);
    assert_eq!(
        status
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        ["Status_BOOST"]
    );

    let spell_fields = std::collections::BTreeMap::from([("SpellType".into(), "Target".into())]);
    let spell = schema.infer_legacy(Path::new("Spell.txt"), Some("SpellData"), &spell_fields);
    assert_eq!(
        spell
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        ["Spell_Target"]
    );
}

#[test]
fn parses_plain_stats_references_and_functions() {
    let root = fixtures();
    let path = root.join("project/Public/MyMod/Stats/Generated/Data/Passive.txt");
    let parsed = parse_source(
        SourceFile {
            path: path.clone(),
            kind: SourceKind::PlainStats,
        },
        &fs::read_to_string(path).unwrap(),
        &load_schema(&root),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.definitions.len(), 2);
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: Some("SpellData".into()),
                name: "Target_Test".into(),
            }
    }));
    assert!(
        parsed
            .observed_functions
            .iter()
            .any(|function| function.name == "UnlockSpell")
    );
}

#[test]
fn parses_prefixed_functor_statements_without_prefix_references() {
    let source = SourceFile {
        path: PathBuf::from("project/Public/MyMod/Stats/Generated/Data/Spell.txt"),
        kind: SourceKind::PlainStats,
    };
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:DealDamage(MainMeleeWeapon,X);AI_IGNORE:CAST:Kill()\"\n";
    let parsed = parse_source(source, text, &SchemaCatalog::default(), "English").unwrap();

    // Prefix words select an execution context and never name a declaration.
    assert!(!parsed.references.iter().any(|reference| matches!(
        &reference.target,
        SymbolTarget::Named { name, .. }
            if name == "GROUND" || name == "AI_IGNORE" || name == "CAST"
    )));
    // Unknown callees still become helper references through the prefixes.
    assert!(parsed.references.iter().any(|reference| reference.target
        == SymbolTarget::Named {
            kind: Some(THOTH_FUNCTION_KIND.into()),
            name: "Kill".into(),
        }));
    assert!(parsed.references.iter().any(|reference| reference.target
        == SymbolTarget::Named {
            kind: None,
            name: "MainMeleeWeapon".into(),
        }));
}

#[test]
fn extracts_references_from_bracketed_functor_groups() {
    let source = SourceFile {
        path: PathBuf::from("project/Public/MyMod/Stats/Generated/Data/Spell.txt"),
        kind: SourceKind::PlainStats,
    };
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:DealDamage(MainMeleeWeapon,X);CastOffhand[IF(not Dead()):ExecuteWeaponFunctors(OffHand)]\"\n";
    let parsed = parse_source(source, text, &SchemaCatalog::default(), "English").unwrap();

    // The group name selects a context and never names a declaration.
    assert!(!parsed.references.iter().any(|reference| matches!(
        &reference.target,
        SymbolTarget::Named { name, .. } if name == "CastOffhand"
    )));
    // Statements inside the brackets extract normally.
    assert!(parsed.references.iter().any(|reference| reference.target
        == SymbolTarget::Named {
            kind: None,
            name: "MainMeleeWeapon".into(),
        }));
    assert!(parsed.issues.is_empty());
}

#[test]
fn classifies_structural_stats_values_for_previews() {
    assert!(is_structural_stats_value(
        "GROUND:DealDamage(1d6,Fire);RemoveStatus(SELF,X)"
    ));
    assert!(is_structural_stats_value(
        "Attack(AttackType.MeleeWeaponAttack)"
    ));
    assert!(is_structural_stats_value("ActionPoint:1"));
    assert!(is_structural_stats_value(
        "IF(not Dead()):DealDamage(1d4,Fire)"
    ));

    // Bare constants, handles, and markers get no expression preview.
    assert!(!is_structural_stats_value("Action_GargantuanCleave"));
    assert!(!is_structural_stats_value("OncePerTurn"));
    assert!(!is_structural_stats_value(
        "h8b70d7dbg9a77g42eagb9afge63af35661e0;1"
    ));
    assert!(!is_structural_stats_value("%%%EMPTY"));
    assert!(!is_structural_stats_value(""));
}

#[test]
fn parses_thoth_declarations_parameters_and_calls() {
    let source = SourceFile {
        path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Test.khn"),
        kind: SourceKind::Thoth,
    };
    let parsed = parse_source(
        source,
        "function ProjectOnly(entity, fallback)\n  try\n    return DependencyOnly(entity)\n  catch error then\n    return fallback\n  end\nend\n",
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.definitions.len(), 1);
    assert_eq!(parsed.definitions[0].kind, THOTH_FUNCTION_KIND);
    assert_eq!(parsed.definitions[0].name, "ProjectOnly");
    assert_eq!(
        parsed.definitions[0].fields["Parameters"],
        "entity, fallback"
    );
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: Some(THOTH_FUNCTION_KIND.into()),
                name: "DependencyOnly".into(),
            }
    }));
    assert!(parsed.issues.is_empty());
}

#[test]
fn extracts_cacheable_thoth_facts_without_inventing_types() {
    let text = "function Compute(entity, ...)\n  local result = Namespace.Enum.Value\n  result = Helper(entity, Namespace.Enum.Value)\n  return result, Namespace.Enum.Value\nend\n";
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Compute.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    let facts = parsed.thoth.expect("Thoth facts");
    assert_eq!(parse_thoth_file(text).unwrap(), facts);
    assert_eq!(facts.declarations.len(), 1);
    assert_eq!(facts.declarations[0].name, "Compute");
    assert_eq!(
        facts.declarations[0].parameters,
        vec![
            ThothParameter {
                name: "entity".into(),
                range: bg3_index::TextRange {
                    start: bg3_index::Position {
                        line: 0,
                        character: 17,
                    },
                    end: bg3_index::Position {
                        line: 0,
                        character: 23,
                    },
                },
                variadic: false,
            },
            ThothParameter {
                name: "...".into(),
                range: bg3_index::TextRange {
                    start: bg3_index::Position {
                        line: 0,
                        character: 25,
                    },
                    end: bg3_index::Position {
                        line: 0,
                        character: 28,
                    },
                },
                variadic: true,
            },
        ]
    );
    assert_eq!(facts.returns.len(), 1);
    assert_eq!(facts.returns[0].expressions.len(), 2);
    assert_eq!(facts.returns[0].expressions[0].text, "result");
    assert_eq!(facts.returns[0].expressions[1].text, "Namespace.Enum.Value");
    assert_eq!(facts.calls.len(), 1);
    assert_eq!(facts.calls[0].name, "Helper");
    assert_eq!(facts.calls[0].arity, 2);
    assert_eq!(facts.calls[0].arguments[1].text, "Namespace.Enum.Value");
    assert_eq!(
        facts.calls[0]
            .owner
            .as_ref()
            .map(|owner| owner.name.as_str()),
        Some("Compute")
    );
    assert_eq!(parsed.observed_functions.len(), 1);
    assert_eq!(parsed.observed_functions[0].name, "Helper");
    assert_eq!(parsed.observed_functions[0].count, 1);
    assert_eq!(parsed.observed_functions[0].min_arity, 2);
    assert_eq!(parsed.observed_functions[0].max_arity, 2);
    assert_eq!(facts.assignments.len(), 2);
    assert!(facts.assignments[0].local);
    assert!(!facts.assignments[1].local);
    assert_eq!(
        facts.assignments[0]
            .owner
            .as_ref()
            .map(|owner| owner.name.as_str()),
        Some("Compute")
    );
    assert_eq!(facts.assignments[0].targets[0].text, "result");
    assert_eq!(facts.assignments[0].values[0].text, "Namespace.Enum.Value");
    assert!(
        facts
            .member_accesses
            .iter()
            .any(|member| member.text == "Namespace.Enum.Value"
                && member.root == "Namespace"
                && member.members == ["Enum", "Value"])
    );
}

#[test]
fn distinguishes_local_and_global_thoth_declarations_with_values() {
    let facts = parse_thoth_file("local local_value = 1\nglobal global_value = 2\n").unwrap();

    assert_eq!(facts.assignments.len(), 2);
    assert!(facts.assignments[0].local);
    assert!(!facts.assignments[0].global);
    assert!(!facts.assignments[1].local);
    assert!(facts.assignments[1].global);
}

#[test]
fn classifies_thoth_expression_facts_and_preserves_return_order() {
    let text = "function Facts(value)\n  return nil, false, 42, \"text\", value, Call(value), Namespace.Member, value + 1\nend\n";
    let facts = parse_thoth_file(text).unwrap();
    let expressions = &facts.expression_facts;
    let returned = &facts.returns[0].expressions;
    assert_eq!(
        returned
            .iter()
            .map(|expression| expression.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "nil",
            "false",
            "42",
            "\"text\"",
            "value",
            "Call(value)",
            "Namespace.Member",
            "value + 1"
        ]
    );

    for (text, kind) in [
        ("nil", ThothExpressionKind::Literal(ThothLiteralKind::Nil)),
        (
            "false",
            ThothExpressionKind::Literal(ThothLiteralKind::Boolean),
        ),
        ("42", ThothExpressionKind::Literal(ThothLiteralKind::Number)),
        (
            "\"text\"",
            ThothExpressionKind::Literal(ThothLiteralKind::String),
        ),
        ("value", ThothExpressionKind::Identifier),
        ("Call(value)", ThothExpressionKind::FunctionCall),
        (
            "Namespace.Member",
            ThothExpressionKind::MemberAccess(Vec::new()),
        ),
        (
            "value + 1",
            ThothExpressionKind::Binary {
                operator: ThothBinaryOperator::Add,
                left: Default::default(),
                right: Default::default(),
            },
        ),
    ] {
        let fact = expressions
            .iter()
            .find(|fact| fact.text == text)
            .expect("expression fact");
        assert_eq!(
            fact.range,
            returned
                .iter()
                .find(|expression| expression.text == text)
                .unwrap()
                .range
        );
        match (&fact.kind, kind) {
            (ThothExpressionKind::Literal(left), ThothExpressionKind::Literal(right)) => {
                assert_eq!(left, &right)
            }
            (ThothExpressionKind::Identifier, ThothExpressionKind::Identifier)
            | (ThothExpressionKind::FunctionCall, ThothExpressionKind::FunctionCall)
            | (ThothExpressionKind::Unknown, ThothExpressionKind::Unknown)
            | (ThothExpressionKind::MemberAccess(_), ThothExpressionKind::MemberAccess(_)) => {}
            (
                ThothExpressionKind::Binary { operator: left, .. },
                ThothExpressionKind::Binary {
                    operator: right, ..
                },
            ) => assert_eq!(*left, right),
            _ => panic!("unexpected kind for {text}"),
        }
    }
}

#[test]
fn preserves_structured_operators_parentheses_branches_and_return_statements() {
    let text = "function Flow(value)\n  if value ~= nil then\n    return (value)\n  else\n    return -value\n  end\nend\n";
    let facts = parse_thoth_file(text).unwrap();

    let guard = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "value ~= nil")
        .expect("nil guard fact");
    let ThothExpressionKind::Binary {
        operator,
        left,
        right,
    } = &guard.kind
    else {
        panic!("nil guard must be a binary fact");
    };
    assert_eq!(*operator, ThothBinaryOperator::NotEqual);
    assert_eq!(
        *left,
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "value")
            .unwrap()
            .range
    );
    assert_eq!(
        *right,
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "nil")
            .unwrap()
            .range
    );

    let parenthesized = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "(value)")
        .expect("parenthesized return fact");
    let ThothExpressionKind::Parenthesized { expression } = &parenthesized.kind else {
        panic!("parentheses must remain structured");
    };
    assert_eq!(
        *expression,
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "value" && fact.range.start.line == 2)
            .unwrap()
            .range
    );

    let unary = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "-value")
        .expect("unary return fact");
    let ThothExpressionKind::Unary { operator, operand } = &unary.kind else {
        panic!("unary expression must remain structured");
    };
    assert_eq!(*operator, ThothUnaryOperator::Negate);
    assert_eq!(
        *operand,
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "value" && fact.range.start.line == 4)
            .unwrap()
            .range
    );

    let returns = facts.returns.iter().collect::<Vec<_>>();
    assert_eq!(returns.len(), 2);
    assert!(
        returns
            .iter()
            .all(|return_fact| return_fact.statement.is_some())
    );

    let flow = &facts.control_flow;
    assert_eq!(flow.len(), 1);
    assert_eq!(flow[0].branches.len(), 2);
    assert_eq!(flow[0].branches[0].kind, ThothIfBranchKind::Consequence);
    assert_eq!(flow[0].branches[1].kind, ThothIfBranchKind::Else);
    assert_eq!(flow[0].branches[0].condition, Some(guard.range));
    assert!(flow[0].branches.iter().all(|branch| branch.scope.is_some()));
}

#[test]
fn records_each_member_segment_and_direct_assignment_target() {
    let text = "function Members(value)\n  local target = Namespace.Member\n  target.field = value\n  target:Method(value)\n  target[\"key\"] = value\n  return a.b[c], a[b]\nend\n";
    let facts = parse_thoth_file(text).unwrap();
    let member_fact = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "target:Method")
        .expect("method member fact");
    let ThothExpressionKind::MemberAccess(segments) = &member_fact.kind else {
        panic!("method access must be a member fact");
    };
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        vec!["target", "Method"]
    );
    assert_eq!(segments[0].access, ThothMemberAccessKind::Root);
    assert_eq!(segments[1].access, ThothMemberAccessKind::Method);
    assert_eq!(segments[0].text, "target");
    assert_eq!(segments[1].text, "Method");
    assert_eq!(segments[0].range.start.line, member_fact.range.start.line);
    assert_eq!(
        segments[0].range.start.character,
        member_fact.range.start.character
    );
    assert_eq!(
        segments[0].range.end.character - segments[0].range.start.character,
        6
    );
    assert_eq!(
        segments[1].range.start.character,
        member_fact.range.start.character + 7
    );
    assert_eq!(
        segments[1].range.end.character,
        member_fact.range.end.character
    );

    let dotted = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "Namespace.Member")
        .unwrap();
    let ThothExpressionKind::MemberAccess(segments) = &dotted.kind else {
        panic!("dotted access")
    };
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Namespace", "Member"]
    );
    assert_eq!(segments[1].access, ThothMemberAccessKind::Dot);
    assert_eq!(segments[0].range.start.character, 17);
    assert_eq!(segments[0].range.end.character, 26);
    assert_eq!(segments[1].range.start.character, 27);
    assert_eq!(segments[1].range.end.character, 33);
    let bracket = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "target[\"key\"]")
        .unwrap();
    let ThothExpressionKind::MemberAccess(segments) = &bracket.kind else {
        panic!("bracket access")
    };
    assert_eq!(segments[1].access, ThothMemberAccessKind::Bracket);
    assert_eq!(segments[0].text, "target");
    assert_eq!(segments[1].text, "\"key\"");

    let chain = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "a.b[c]")
        .unwrap();
    let ThothExpressionKind::MemberAccess(segments) = &chain.kind else {
        panic!("three-segment chain")
    };
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.text.as_str(), segment.access))
            .collect::<Vec<_>>(),
        vec![
            ("a", ThothMemberAccessKind::Root),
            ("b", ThothMemberAccessKind::Dot),
            ("c", ThothMemberAccessKind::Bracket),
        ]
    );
    assert_eq!(segments[0].range.start.character, 9);
    assert_eq!(segments[0].range.end.character, 10);
    assert_eq!(segments[1].range.start.character, 11);
    assert_eq!(segments[1].range.end.character, 12);
    assert_eq!(segments[2].range.start.character, 13);
    assert_eq!(segments[2].range.end.character, 14);

    let dynamic = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "a[b]")
        .unwrap();
    let ThothExpressionKind::MemberAccess(segments) = &dynamic.kind else {
        panic!("dynamic bracket access")
    };
    assert_eq!(segments[0].text, "a");
    assert_eq!(segments[0].access, ThothMemberAccessKind::Root);
    assert_eq!(segments[1].text, "b");
    assert_eq!(segments[1].access, ThothMemberAccessKind::Bracket);

    let assignment_targets = facts
        .assignments
        .iter()
        .flat_map(|assignment| assignment.targets.iter())
        .collect::<Vec<_>>();
    assert_eq!(
        assignment_targets
            .iter()
            .map(|target| target.text.as_str())
            .collect::<Vec<_>>(),
        vec!["target", "target.field", "target[\"key\"]"]
    );
    assert!(matches!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "target")
            .unwrap()
            .kind,
        ThothExpressionKind::Identifier
    ));
    assert!(matches!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "target.field")
            .unwrap()
            .kind,
        ThothExpressionKind::MemberAccess(_)
    ));
    assert!(matches!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.text == "target[\"key\"]")
            .unwrap()
            .kind,
        ThothExpressionKind::MemberAccess(_)
    ));
}

#[test]
fn assigns_deterministic_scope_hierarchy_and_statement_order() {
    let text = "local top = 1; top = 2\nfunction Outer(unusedOuter)\n  local first = function(unusedAnonymous) return 1 end\n  local chained = GetObject().Field\n  local parenthesized = (top).Field\n  if top then\n    function Inner(unusedInner) return 2 end\n  end\nend\n";
    let first = parse_thoth_file(text).unwrap();
    let second = parse_thoth_file(text).unwrap();
    assert_eq!(first.scopes, second.scopes);
    assert_eq!(first.expression_facts, second.expression_facts);
    let file = first
        .scopes
        .iter()
        .find(|scope| scope.id == ThothScopeId::File)
        .unwrap();
    assert_eq!(file.parent, None);
    let functions = first
        .scopes
        .iter()
        .filter(|scope| matches!(scope.id, ThothScopeId::Function { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        functions.len(),
        3,
        "Outer, Inner, and anonymous function scopes"
    );
    let outer = functions
        .iter()
        .find(|scope| matches!(scope.id, ThothScopeId::Function { range } if range.start.line == 1))
        .unwrap();
    assert_eq!(outer.parent, Some(ThothScopeId::File));
    let nested = functions
        .iter()
        .find(|scope| matches!(scope.id, ThothScopeId::Function { range } if range.start.line == 6))
        .unwrap();
    let anonymous = functions
        .iter()
        .find(|scope| matches!(scope.id, ThothScopeId::Function { range } if range.start.line == 2))
        .unwrap();
    assert!(matches!(nested.parent, Some(ThothScopeId::Block { .. })));
    assert!(matches!(anonymous.parent, Some(ThothScopeId::Block { .. })));
    let block = first
        .scopes
        .iter()
        .find(|scope| scope.parent == Some(outer.id))
        .unwrap();
    assert_eq!(block.parent, Some(outer.id));

    let file_orders = first
        .expression_facts
        .iter()
        .filter_map(|fact| {
            (fact.statement.scope == ThothScopeId::File).then_some(fact.statement.order)
        })
        .collect::<Vec<_>>();
    assert_eq!(file_orders, vec![0, 0, 1, 1]);
    let outer_orders = first
        .expression_facts
        .iter()
        .filter_map(|fact| (fact.statement.scope == block.id).then_some(fact.statement.order))
        .collect::<Vec<_>>();
    // Nested call/member roots retain the same statement identity as their
    // containing expression, so one statement can contribute several facts.
    assert_eq!(outer_orders, vec![0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3]);
    for (text, root) in [
        ("GetObject().Field", "GetObject()"),
        ("(top).Field", "(top)"),
    ] {
        let fact = first
            .expression_facts
            .iter()
            .find(|fact| fact.text == text)
            .unwrap();
        let ThothExpressionKind::MemberAccess(segments) = &fact.kind else {
            panic!("{text} must remain a member access");
        };
        assert_eq!(
            segments
                .iter()
                .map(|segment| (segment.text.as_str(), segment.access))
                .collect::<Vec<_>>(),
            vec![
                (root, ThothMemberAccessKind::Root),
                ("Field", ThothMemberAccessKind::Dot),
            ]
        );
        assert_eq!(segments[0].range.start, fact.range.start);
        assert_eq!(
            segments[0].range.end.character,
            fact.range.start.character + u32::try_from(root.len()).unwrap()
        );
        assert_eq!(
            segments[1].range.start.character,
            segments[0].range.end.character + 1
        );
        assert_eq!(segments[1].range.end, fact.range.end);
    }
    assert!(
        !first
            .expression_facts
            .iter()
            .any(|fact| fact.text == "Outer")
    );
    assert!(
        !first
            .expression_facts
            .iter()
            .any(|fact| fact.text == "Inner")
    );
    let anonymous_return = first
        .expression_facts
        .iter()
        .find(|fact| fact.text == "1" && fact.range.start.line == 2)
        .unwrap();
    let inner_return = first
        .expression_facts
        .iter()
        .find(|fact| fact.text == "2" && fact.range.start.line == 6)
        .unwrap();
    for parameter in ["unusedOuter", "unusedAnonymous", "unusedInner"] {
        assert!(
            !first
                .expression_facts
                .iter()
                .any(|fact| fact.text == parameter)
        );
    }
    let anonymous_scope = functions
        .iter()
        .find(|scope| matches!(scope.id, ThothScopeId::Function { range } if range.start.line == 2))
        .unwrap();
    let inner_scope = functions
        .iter()
        .find(|scope| matches!(scope.id, ThothScopeId::Function { range } if range.start.line == 6))
        .unwrap();
    assert!(first.scopes.iter().any(|scope| {
        scope.parent == Some(anonymous_scope.id) && scope.id == anonymous_return.statement.scope
    }));
    assert!(first.scopes.iter().any(|scope| {
        scope.parent == Some(inner_scope.id) && scope.id == inner_return.statement.scope
    }));
}

#[test]
fn repeat_body_facts_precede_the_trailing_until_condition() {
    let facts = parse_thoth_file("repeat\n  Tick(value)\nuntil Ready(value)\n").unwrap();
    let body = facts
        .expression_facts
        .iter()
        .position(|fact| fact.text == "Tick(value)")
        .expect("repeat body call fact");
    let condition = facts
        .expression_facts
        .iter()
        .position(|fact| fact.text == "Ready(value)")
        .expect("repeat condition call fact");
    assert!(
        body < condition,
        "body facts must precede the until condition"
    );
}

#[test]
fn warm_local_cache_preserves_thoth_expression_facts_and_scopes() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("Facts.khn");
    let text = "function Facts(value)\n  local result = value\n  if value ~= nil then\n    return (result)\n  end\nend\n";
    fs::write(&source_path, text).unwrap();
    let source = SourceFile {
        path: source_path,
        kind: SourceKind::Thoth,
    };
    let module = ModuleSpec {
        name: "Synthetic".into(),
        root: directory.path().into(),
        role: ModuleRole::Project,
    };
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();
    let (cold, cold_stats) = cache
        .build_module(
            &module,
            std::slice::from_ref(&source),
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
    let (warm, warm_stats) = cache
        .build_module(
            &module,
            std::slice::from_ref(&source),
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
    assert_eq!(cold_stats.misses, 1);
    assert_eq!(warm_stats.hits, 1);
    assert_eq!(warm_stats.misses, 0);
    assert_eq!(cold[0].thoth, warm[0].thoth);
    assert_eq!(
        cold[0].thoth.as_ref().unwrap().expression_facts,
        warm[0].thoth.as_ref().unwrap().expression_facts
    );
    assert_eq!(
        cold[0].thoth.as_ref().unwrap().scopes,
        warm[0].thoth.as_ref().unwrap().scopes
    );
    assert_eq!(
        cold[0].thoth.as_ref().unwrap().control_flow,
        warm[0].thoth.as_ref().unwrap().control_flow
    );
    assert_eq!(
        cold[0].thoth.as_ref().unwrap().returns,
        warm[0].thoth.as_ref().unwrap().returns
    );
}

#[test]
fn attaches_thoth_annotations_only_across_adjacent_rows() {
    let text = "---@alias WeaponId integer\n---@class Weapon\n---@field id integer\n---@field label string\n\n---@param item? Weapon\n-- ordinary comment breaks the contract\n---@return boolean\nfunction IsWeapon(item)\n  return item ~= nil\nend\n\n---@type Weapon\nlocal value\n";
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Types.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    let facts = parsed.thoth.expect("Thoth facts");
    assert_eq!(facts.annotations.classes.len(), 1);
    assert_eq!(facts.annotations.classes[0].name, "Weapon");
    assert_eq!(facts.annotations.classes[0].name_range.start.character, 10);
    assert_eq!(facts.annotations.classes[0].name_range.end.character, 16);
    assert_eq!(facts.annotations.classes[0].fields.len(), 2);
    assert_eq!(facts.annotations.classes[0].fields[0].name, "id");
    assert_eq!(
        facts.annotations.classes[0].fields[0].ty.to_string(),
        "integer"
    );
    assert_eq!(
        facts.annotations.classes[0].fields[0]
            .name_range
            .start
            .character,
        10
    );
    assert_eq!(
        facts.annotations.classes[0].fields[0]
            .name_range
            .end
            .character,
        12
    );
    assert_eq!(
        facts.annotations.classes[0].fields[0]
            .type_range
            .start
            .character,
        13
    );
    assert_eq!(
        facts.annotations.classes[0].fields[0]
            .type_range
            .end
            .character,
        20
    );
    assert_eq!(
        facts.annotations.classes[0].fields[1]
            .name_range
            .start
            .character,
        10
    );
    assert_eq!(
        facts.annotations.classes[0].fields[1]
            .name_range
            .end
            .character,
        15
    );
    assert_eq!(
        facts.annotations.classes[0].fields[1]
            .type_range
            .start
            .character,
        16
    );
    assert_eq!(
        facts.annotations.classes[0].fields[1]
            .type_range
            .end
            .character,
        22
    );
    assert_eq!(facts.annotations.aliases[0].name, "WeaponId");
    assert_eq!(facts.annotations.aliases[0].name_range.start.character, 10);
    assert_eq!(facts.annotations.aliases[0].name_range.end.character, 18);
    assert_eq!(facts.annotations.aliases[0].type_range.start.character, 19);
    assert_eq!(facts.annotations.aliases[0].type_range.end.character, 26);

    assert_eq!(facts.annotations.functions.len(), 1);
    let contract = &facts.annotations.functions[0].contracts[0];
    assert!(contract.parameters.is_empty());
    assert_eq!(contract.returns[0].ty.to_string(), "boolean");

    assert_eq!(facts.annotations.variables.len(), 1);
    assert_eq!(facts.annotations.variables[0].target, "value");
    assert_eq!(facts.annotations.variables[0].ty.to_string(), "Weapon");
}

#[test]
fn optional_and_variadic_parameter_annotations_preserve_ranges() {
    let text =
        "---@param item? Weapon\n---@param ...rest string\nfunction Inspect(item, ...)\nend\n";
    let facts = parse_thoth_file(text).unwrap();
    let parameters = &facts.annotations.functions[0].contracts[0].parameters;

    assert_eq!(parameters[0].name, "item");
    assert_eq!(parameters[0].ty.to_string(), "Weapon|nil");
    assert!(!parameters[0].variadic);
    assert_eq!(parameters[0].name_range.start.character, 10);
    assert_eq!(parameters[0].type_range.start.character, 16);
    assert_eq!(parameters[1].name, "rest");
    assert!(parameters[1].variadic);
    assert_eq!(parameters[1].type_range.start.character, 18);
}

#[test]
fn unsupported_annotation_tags_break_function_attachment() {
    let text = "---@param item string\n---@unsupported ignored\n---@return boolean\nfunction IsValid(item)\nend\n";
    let facts = parse_thoth_file(text).unwrap();
    let contract = &facts.annotations.functions[0].contracts[0];
    assert!(contract.parameters.is_empty());
    assert_eq!(contract.returns.len(), 1);
}

#[test]
fn captures_prose_documentation_lines_and_returns_alias() {
    let text = concat!(
        "--- Used in condition contexts.\n",
        "--- Returns whether the helper applies.\n",
        "---@param value number\n",
        "---@returns boolean\n",
        "function Helper(value)\n",
        "  return true\n",
        "end\n",
        "\n",
        "--- Documents the fallback without typing it.\n",
        "function Fallback(value)\n",
        "end\n",
    );
    let facts = parse_thoth_file(text).unwrap();
    assert_eq!(facts.annotations.functions.len(), 2);

    let contract = &facts.annotations.functions[0].contracts[0];
    assert_eq!(
        contract.description,
        vec![
            "Used in condition contexts.".to_owned(),
            "Returns whether the helper applies.".to_owned(),
        ]
    );
    assert_eq!(contract.parameters[0].name, "value");
    assert_eq!(contract.parameters[0].ty.to_string(), "number");
    assert_eq!(contract.returns[0].ty.to_string(), "boolean");

    let untyped = &facts.annotations.functions[1].contracts[0];
    assert_eq!(
        untyped.description,
        vec!["Documents the fallback without typing it.".to_owned()]
    );
    assert!(untyped.parameters.is_empty());
    assert!(untyped.returns.is_empty());
}

#[test]
fn plain_comments_still_break_prose_documentation_attachment() {
    let broken = "--- Documents the helper.\n-- ordinary comment\nfunction Broken(value)\nend\n";
    let facts = parse_thoth_file(broken).unwrap();
    assert!(facts.annotations.functions.is_empty());

    let dashed = "--- Documents the helper.\n---- separator\nfunction Dashed(value)\nend\n";
    let facts = parse_thoth_file(dashed).unwrap();
    assert!(facts.annotations.functions.is_empty());
}

#[test]
fn rejects_non_whitespace_type_suffixes() {
    for suffix in ["Weapon$", "boolean)", "Weapon[]foo"] {
        let text = format!("---@param item {suffix}\nfunction Broken(item)\nend\n");
        let parsed = parse_source(
            SourceFile {
                path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Broken.khn"),
                kind: SourceKind::Thoth,
            },
            &text,
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
        assert!(
            parsed
                .issues
                .iter()
                .any(|issue| { issue.code == "thoth-annotation-error" }),
            "suffix {suffix:?} was accepted"
        );
        assert!(parse_thoth_file(&text).is_err());
    }
}

#[test]
fn rejects_malformed_annotation_names_but_accepts_dotted_nominal_names() {
    for annotation in [
        "---@class Bad-Name",
        "---@field bad-name string",
        "---@alias Bad..Name string",
        "---@param bad-name string",
    ] {
        let text = format!("{annotation}\nfunction Broken(item)\nend\n");
        let parsed = parse_source(
            SourceFile {
                path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Broken.khn"),
                kind: SourceKind::Thoth,
            },
            &text,
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
        assert!(
            parsed
                .issues
                .iter()
                .any(|issue| { issue.code == "thoth-annotation-error" }),
            "name in {annotation:?} was accepted"
        );
    }

    let facts =
        parse_thoth_file("---@class Namespace.Weapon\n---@alias Namespace.WeaponId integer\n")
            .unwrap();
    assert_eq!(facts.annotations.classes[0].name, "Namespace.Weapon");
    assert_eq!(facts.annotations.aliases[0].name, "Namespace.WeaponId");
}

#[test]
fn omits_ambiguous_type_annotations_without_an_issue() {
    let text = "---@type Weapon\nlocal left, right\n";
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Ambiguous.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    let facts = parsed.thoth.expect("Thoth facts");
    assert!(facts.annotations.variables.is_empty());
    assert!(parsed.issues.is_empty());
    assert!(parse_thoth_file(text).is_ok());
}

#[test]
fn omits_multiple_type_annotations_and_ignores_four_dash_comments() {
    let multiple = "---@type Weapon\n---@type Armor\nlocal value\n";
    let facts = parse_thoth_file(multiple).unwrap();
    assert!(facts.annotations.variables.is_empty());

    let ignored = "----@class NotAnAnnotation\nfunction Example()\nend\n";
    let facts = parse_thoth_file(ignored).unwrap();
    assert!(facts.annotations.classes.is_empty());
    assert!(facts.annotations.functions.is_empty());
}

#[test]
fn malformed_supported_annotations_are_issues_and_rejected_from_virtual_sources() {
    let text = "---@param item\nfunction Broken(item)\nend\n";
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Broken.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(
        parsed
            .issues
            .iter()
            .any(|issue| issue.code == "thoth-annotation-error")
    );
    assert!(parse_thoth_file(text).is_err());
}

#[test]
fn rejects_partial_facts_from_malformed_virtual_thoth_sources() {
    let error = parse_thoth_file("function Broken(entity)\n  @\nend\n").unwrap_err();

    assert!(error.to_string().contains("invalid syntax"));
}

#[test]
fn reports_thoth_syntax_errors_for_malformed_sources() {
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Broken.khn"),
            kind: SourceKind::Thoth,
        },
        "function Broken(entity)\n  @\nend\n",
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.issues.len(), 1);
    assert_eq!(parsed.issues[0].code, "thoth-syntax-error");
    assert_eq!(parsed.issues[0].message, "The Thoth syntax is not valid.");
    assert_eq!(
        parsed.issues[0].range,
        bg3_index::TextRange {
            start: bg3_index::Position {
                line: 1,
                character: 2,
            },
            end: bg3_index::Position {
                line: 1,
                character: 3,
            },
        }
    );
}

#[test]
fn incomplete_structured_expressions_keep_the_loose_overlay_parseable() {
    for text in [
        "local value = (\n",
        "local value = -\n",
        "local value = 1 +\n",
    ] {
        let parsed = parse_source(
            SourceFile {
                path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Editing.khn"),
                kind: SourceKind::Thoth,
            },
            text,
            &SchemaCatalog::default(),
            "English",
        )
        .expect("incomplete editor text must still produce an overlay");

        assert!(parsed.thoth.is_some());
        assert!(
            parsed
                .issues
                .iter()
                .any(|issue| issue.code == "thoth-syntax-error")
        );
    }
}

#[test]
fn parses_osiris_goals_declarations_calls_and_database_evidence() {
    let root = fixtures();
    let path = root.join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let parsed = parse_source(
        SourceFile {
            path: path.clone(),
            kind: SourceKind::Osiris,
        },
        &fs::read_to_string(path).unwrap(),
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert!(parsed.issues.is_empty());
    assert!(parsed.definitions.iter().any(|definition| {
        definition.kind == OSIRIS_GOAL_KIND && definition.name == "MainGoal"
    }));
    assert!(parsed.definitions.iter().any(|definition| {
        definition.kind == OSIRIS_PROCEDURE_KIND
            && definition.name == "SharedProc"
            && definition.arity == Some(1)
    }));
    assert!(parsed.definitions.iter().any(|definition| {
        definition.kind == OSIRIS_QUERY_KIND
            && definition.name == "ProjectQuery"
            && definition.arity == Some(1)
    }));
    let databases: Vec<_> = parsed
        .definitions
        .iter()
        .filter(|definition| definition.kind == OSIRIS_DATABASE_KIND)
        .collect();
    assert_eq!(databases.len(), 2);
    assert!(databases.iter().any(|definition| {
        definition.name == "DB_Tracked"
            && definition.arity == Some(2)
            && definition.fields["Parameters"] == "CHARACTER, INTEGER"
    }));
    assert!(
        databases
            .iter()
            .any(|definition| { definition.name == "DB_NOOP" && definition.arity == Some(1) })
    );
    assert!(
        !databases
            .iter()
            .any(|definition| definition.name == "DB_PackedOnly")
    );
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::OsirisCallable {
                name: "SharedProc".into(),
                arity: 1,
            }
    }));
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::OsirisCallable {
                name: "ApplyExample".into(),
                arity: 1,
            }
    }));
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::OsirisGoal {
                name: "SharedGoal".into(),
            }
    }));
    let osiris = parsed.osiris.as_ref().unwrap();
    let tracked: Vec<_> = osiris
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.name == "DB_Tracked")
        .collect();
    assert_eq!(tracked.len(), 6);
    assert!(tracked.iter().any(|occurrence| {
        occurrence.role == OsirisCallRole::Read
            && occurrence.arguments[0]
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.type_name == "CHARACTER")
    }));
}

#[test]
fn derives_engine_aliases_for_uncast_head_variables() {
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\nDied(_Char)\nTHEN\nDB_Deceased(_Char);\n",
        "IF\nUnknownEvent(_Who)\nTHEN\nDB_Missing(_Who);\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Derived.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(parsed.issues.is_empty());
    let osiris = parsed.osiris.as_ref().unwrap();

    let deceased = osiris_occurrence(&osiris.occurrences, "DB_Deceased");
    let evidence = deceased.arguments[0].evidence.as_ref().unwrap();
    assert_eq!(evidence.type_name, "CHARACTER");
    assert_eq!(evidence.origin, OsirisEvidenceOrigin::Engine);

    let missing = osiris_occurrence(&osiris.occurrences, "DB_Missing");
    assert!(missing.arguments[0].evidence.is_none());
}

#[test]
fn leaves_repeated_engine_variables_unknown_when_aliases_conflict() {
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\nEnteredLevel(_Ambiguous, _Ambiguous, _Other)\nTHEN\n",
        "DB_Ambiguous(_Ambiguous);\nDB_Consistent(_Other);\n",
        "IF\nAddedTo(_Same, _Same, _Text)\nTHEN\n",
        "DB_SameAlias(_Same);\nDB_Text(_Text);\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Aliases.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(parsed.issues.is_empty());
    let osiris = parsed.osiris.as_ref().unwrap();

    let ambiguous = osiris_occurrence(&osiris.occurrences, "DB_Ambiguous");
    assert!(ambiguous.arguments[0].evidence.is_none());

    let consistent = osiris_occurrence(&osiris.occurrences, "DB_Consistent");
    let evidence = consistent.arguments[0].evidence.as_ref().unwrap();
    assert_eq!(evidence.type_name, "STRING");
    assert_eq!(evidence.origin, OsirisEvidenceOrigin::Engine);

    let same_alias = osiris_occurrence(&osiris.occurrences, "DB_SameAlias");
    let evidence = same_alias.arguments[0].evidence.as_ref().unwrap();
    assert_eq!(evidence.type_name, "GUIDSTRING");
    assert_eq!(evidence.origin, OsirisEvidenceOrigin::Engine);

    let text = osiris_occurrence(&osiris.occurrences, "DB_Text");
    let evidence = text.arguments[0].evidence.as_ref().unwrap();
    assert_eq!(evidence.type_name, "STRING");
    assert_eq!(evidence.origin, OsirisEvidenceOrigin::Engine);
}

#[test]
fn tracks_rule_local_osiris_variable_occurrences_and_proven_bindings() {
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "Died((CHARACTER)_Caster)\n",
        "AND\n",
        "DB_Characters(_FromDb)\n",
        "AND\n",
        "UnknownQuery(_Unknown)\n",
        "AND\n",
        "_Receiver.DB_Method(_FromDb)\n",
        "THEN\n",
        "DB_Result(_Caster, _FromDb, _Unknown);\n",
        "(CHARACTER)_Caster.ApplyExample(_FromDb);\n",
        "IF\n",
        "AnotherEvent(_Caster)\n",
        "THEN\n",
        "DB_Other(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Variables.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(parsed.issues.is_empty());
    let variables = &parsed.osiris.as_ref().unwrap().variables;

    let caster: Vec<_> = variables
        .iter()
        .filter(|fact| fact.name == "_Caster")
        .collect();
    assert_eq!(
        caster.len(),
        2,
        "same names in separate rules must stay separate"
    );
    assert_ne!(caster[0].rule_range, caster[1].rule_range);
    assert_eq!(caster[0].occurrences.len(), 3);
    assert_eq!(caster[0].binding_range, Some(caster[0].occurrences[0]));
    assert!(caster[0].database_binding.is_none());
    assert_eq!(caster[0].evidence.as_ref().unwrap().type_name, "CHARACTER");
    assert_eq!(caster[1].occurrences.len(), 2);
    assert_eq!(caster[1].binding_range, None);

    let from_db = variables
        .iter()
        .find(|fact| fact.name == "_FromDb")
        .unwrap();
    assert_eq!(from_db.occurrences.len(), 4);
    assert_eq!(from_db.binding_range, Some(from_db.occurrences[0]));
    assert_eq!(
        from_db.database_binding,
        Some(bg3_index::OsirisDatabaseBinding {
            name: "DB_Characters".into(),
            arity: 1,
            column: 0,
        })
    );

    let unknown = variables
        .iter()
        .find(|fact| fact.name == "_Unknown")
        .unwrap();
    assert_eq!(unknown.occurrences.len(), 2);
    assert_eq!(unknown.binding_range, None);

    let receiver = variables
        .iter()
        .find(|fact| fact.name == "_Receiver")
        .unwrap();
    assert_eq!(receiver.binding_range, None);
}

#[test]
fn records_only_positive_database_bindings_and_write_types() {
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "Died((CHARACTER)_Writer)\n",
        "THEN\n",
        "DB_ReadOnly((CHARACTER)_Writer);\n",
        "IF\n",
        "DB_ReadOnly(_ReadCaster)\n",
        "AND\n",
        "HasPassive(_ReadCaster, \"SomePassive\", 0)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "UnknownEvent(_HeadCaster)\n",
        "AND\n",
        "NOT DB_ReadOnly(_NegatedCaster)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Bindings.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    let osiris = parsed.osiris.as_ref().unwrap();

    let read_caster = osiris
        .variables
        .iter()
        .find(|fact| fact.name == "_ReadCaster")
        .unwrap();
    assert_eq!(
        read_caster.database_binding,
        Some(bg3_index::OsirisDatabaseBinding {
            name: "DB_ReadOnly".into(),
            arity: 1,
            column: 0,
        })
    );

    let writer = osiris
        .variables
        .iter()
        .find(|fact| fact.name == "_Writer")
        .unwrap();
    assert!(writer.database_binding.is_none());

    let negated = osiris
        .variables
        .iter()
        .find(|fact| fact.name == "_NegatedCaster")
        .unwrap();
    assert!(negated.database_binding.is_none());
    assert_eq!(negated.binding_range, None);

    let database = parsed
        .definitions
        .iter()
        .find(|definition| {
            definition.kind == OSIRIS_DATABASE_KIND && definition.name == "DB_ReadOnly"
        })
        .unwrap();
    assert_eq!(
        database.fields.get("Parameters"),
        Some(&"CHARACTER".to_owned())
    );
}

#[test]
fn excludes_database_removals_from_write_counts_and_types() {
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "DB_Removed((CHARACTER)11111111-1111-1111-1111-111111111111);\n",
        "NOT DB_Removed((GUIDSTRING)22222222-2222-2222-2222-222222222222);\n",
        "NOT DB_OnlyRemoved((GUIDSTRING)44444444-4444-4444-4444-444444444444);\n",
        "KBSECTION\n",
        "IF\n",
        "Died((CHARACTER)_Writer)\n",
        "THEN\n",
        "DB_Action((CHARACTER)_Writer);\n",
        "NOT DB_Action((GUIDSTRING)_Writer);\n",
        "EXITSECTION\n",
        "NOT DB_Removed((GUIDSTRING)33333333-3333-3333-3333-333333333333);\n",
        "ENDEXITSECTION\n",
    );
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Removals.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();
    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);

    let osiris = parsed.osiris.as_ref().unwrap();
    let removed: Vec<_> = osiris
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.name == "DB_Removed")
        .collect();
    assert_eq!(removed.len(), 3);
    assert!(parsed.references.iter().any(|reference| {
        reference.context == "osiris-remove"
            && reference.target
                == SymbolTarget::OsirisDatabase {
                    name: "DB_Removed".into(),
                    arity: 1,
                }
    }));
    assert!(removed.iter().any(|occurrence| {
        occurrence.role == OsirisCallRole::Write
            && occurrence.arguments[0]
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.type_name == "CHARACTER")
    }));
    assert_eq!(
        removed
            .iter()
            .filter(|occurrence| occurrence.role == OsirisCallRole::Remove)
            .count(),
        2
    );
    let action: Vec<_> = osiris
        .occurrences
        .iter()
        .filter(|occurrence| occurrence.name == "DB_Action")
        .collect();
    assert_eq!(action.len(), 2);
    assert!(action.iter().any(|occurrence| {
        occurrence.role == OsirisCallRole::Write
            && occurrence.arguments[0]
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.type_name == "CHARACTER")
    }));
    assert!(action.iter().any(|occurrence| {
        occurrence.role == OsirisCallRole::Remove
            && occurrence.arguments[0]
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence.type_name == "GUIDSTRING")
    }));

    let only_removed_definition = parsed
        .definitions
        .iter()
        .find(|definition| definition.name == "DB_OnlyRemoved");
    assert!(only_removed_definition.is_none());
    let removed_definition = parsed
        .definitions
        .iter()
        .find(|definition| definition.name == "DB_Removed")
        .unwrap();
    assert_eq!(removed_definition.fields["Reads"], "0");
    assert_eq!(removed_definition.fields["Writes"], "1");
    assert_eq!(removed_definition.fields["Parameters"], "CHARACTER");
    let action_definition = parsed
        .definitions
        .iter()
        .find(|definition| definition.name == "DB_Action")
        .unwrap();
    assert_eq!(action_definition.fields["Reads"], "0");
    assert_eq!(action_definition.fields["Writes"], "1");
    assert_eq!(action_definition.fields["Parameters"], "CHARACTER");
}

fn osiris_occurrence<'a>(
    occurrences: &'a [bg3_index::OsirisDatabaseOccurrence],
    name: &str,
) -> &'a bg3_index::OsirisDatabaseOccurrence {
    occurrences
        .iter()
        .find(|occurrence| occurrence.name == name)
        .unwrap()
}

#[test]
fn reports_only_osiris_syntax_errors_for_malformed_goals() {
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/Broken.txt"),
            kind: SourceKind::Osiris,
        },
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\nBroken(\nEXITSECTION\nENDEXITSECTION\n",
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert!(!parsed.issues.is_empty());
    assert!(
        parsed
            .issues
            .iter()
            .all(|issue| issue.code == "osiris-syntax-error")
    );
}

#[test]
fn discovers_thoth_helpers_only_below_mod_script_trees() {
    let root = fixtures();
    let module = ModuleSpec {
        name: "MyMod".into(),
        root: root.join("project"),
        role: ModuleRole::Project,
    };
    let files = discover_module(&module, &root.join("game"), "English", false).unwrap();

    assert!(files.iter().any(|file| {
        file.kind == SourceKind::Thoth
            && file
                .path
                .ends_with("Mods/MyMod/Scripts/thoth/helpers/MyMod.khn")
    }));
    assert!(files.iter().all(|file| {
        file.kind != SourceKind::Thoth
            || file.path.to_string_lossy().contains("/Mods/")
                && file.path.to_string_lossy().contains("/Scripts/thoth/")
    }));
}

#[test]
fn discovers_only_txt_files_at_the_osiris_goal_path() {
    let root = fixtures();
    let module = ModuleSpec {
        name: "MyMod".into(),
        root: root.join("project"),
        role: ModuleRole::Project,
    };
    let files = discover_module(&module, &root.join("game"), "English", false).unwrap();

    let goals: Vec<_> = files
        .iter()
        .filter(|file| file.kind == SourceKind::Osiris)
        .collect();
    assert_eq!(goals.len(), 2);
    assert!(goals.iter().all(|file| {
        file.path
            .to_string_lossy()
            .contains("/Story/RawFiles/Goals/")
            && file
                .path
                .extension()
                .is_some_and(|extension| extension == "txt")
    }));
}

#[test]
fn classifies_schema_object_references_without_guessing_generic_identifiers() {
    let root = fixtures();
    let parsed = parse_source(
        SourceFile {
            path: root.join("project/Public/MyMod/Stats/Generated/Data/Passive.txt"),
            kind: SourceKind::PlainStats,
        },
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"ResourceId\" \"ActionPoint\"\ndata \"Boosts\" \"UnknownName\"\n",
        &load_schema(&root),
        "English",
    )
    .unwrap();

    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: Some("ActionResource".into()),
                name: "ActionPoint".into(),
            }
    }));
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: None,
                name: "UnknownName".into(),
            }
    }));
}

#[test]
fn classifies_status_groups_separately_from_status_declarations() {
    let root = fixtures();
    let parsed = parse_source(
        SourceFile {
            path: root.join("project/Public/MyMod/Stats/Generated/Data/Passive.txt"),
            kind: SourceKind::PlainStats,
        },
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"StatusOnEquip\" \"SG_Charmed\"\ndata \"Boosts\" \"HasStatus('SG_Frightened') and HasStatus(PRONE)\"\n",
        &load_schema(&root),
        "English",
    )
    .unwrap();

    for name in ["SG_Charmed", "SG_Frightened"] {
        assert!(parsed.references.iter().any(|reference| {
            reference.target
                == SymbolTarget::Named {
                    kind: Some("StatusGroup".into()),
                    name: name.into(),
                }
        }));
    }
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: Some("StatusData".into()),
                name: "PRONE".into(),
            }
    }));
}

#[test]
fn classifies_explicit_target_function_overloads() {
    let root = fixtures();
    let parsed = parse_source(
        SourceFile {
            path: root.join("project/Public/MyMod/Stats/Generated/Data/Passive.txt"),
            kind: SourceKind::PlainStats,
        },
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ApplyStatus(OBSERVER_OBSERVER,SHIELD,100,1);UseSpell(OBSERVER_SOURCE,Target_Test,true,true,true);ApplyStatus(TEST_STATUS,100,1)\"\n",
        &load_schema(&root),
        "English",
    )
    .unwrap();

    for (name, kind) in [
        ("SHIELD", "StatusData"),
        ("Target_Test", "SpellData"),
        ("TEST_STATUS", "StatusData"),
    ] {
        assert!(parsed.references.iter().any(|reference| {
            reference.target
                == SymbolTarget::Named {
                    kind: Some(kind.into()),
                    name: name.into(),
                }
        }));
    }
    for selector in ["OBSERVER_OBSERVER", "OBSERVER_SOURCE"] {
        assert!(parsed.references.iter().all(|reference| {
            !matches!(&reference.target, SymbolTarget::Named { name, .. } if name == selector)
        }));
    }
}

#[test]
fn warm_local_cache_preserves_osiris_database_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("Bindings.txt");
    fs::write(
        &source_path,
        concat!(
            "Version 1\n",
            "SubGoalCombiner SGC_AND\n",
            "INITSECTION\n",
            "KBSECTION\n",
            "IF\nDB_Source(_Caster)\nTHEN\nGoalCompleted;\n",
            "EXITSECTION\n",
            "ENDEXITSECTION\n",
        ),
    )
    .unwrap();
    let source = SourceFile {
        path: source_path,
        kind: SourceKind::Osiris,
    };
    let module = ModuleSpec {
        name: "Synthetic".into(),
        root: directory.path().into(),
        role: ModuleRole::Project,
    };
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();
    let (cold, cold_stats) = cache
        .build_module(
            &module,
            std::slice::from_ref(&source),
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
    let (warm, warm_stats) = cache
        .build_module(
            &module,
            std::slice::from_ref(&source),
            &SchemaCatalog::default(),
            "English",
        )
        .unwrap();
    assert_eq!(cold_stats.misses, 1);
    assert_eq!(warm_stats.hits, 1);
    let binding = cold[0]
        .osiris
        .as_ref()
        .unwrap()
        .variables
        .iter()
        .find(|variable| variable.name == "_Caster")
        .unwrap()
        .database_binding
        .clone();
    assert!(binding.is_some());
    let warm_binding = warm[0]
        .osiris
        .as_ref()
        .unwrap()
        .variables
        .iter()
        .find(|variable| variable.name == "_Caster")
        .unwrap()
        .database_binding
        .clone();
    assert_eq!(warm_binding, binding);
}

#[test]
fn indexes_tables_lsx_and_localization() {
    let root = fixtures();
    let game = root.join("game");
    let schema = load_schema(&root);
    let module = ModuleSpec {
        name: "Shared".into(),
        root: game.join("Editor/Mods/Shared"),
        role: ModuleRole::Base,
    };
    let files = discover_module(&module, &game, "English", true).unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = CacheStore::new(cache_dir.path().to_path_buf()).unwrap();
    let (parsed, cold) = cache
        .build_module(&module, &files, &schema, "English")
        .unwrap();
    let (_, warm) = cache
        .build_module(&module, &files, &schema, "English")
        .unwrap();
    let index = ModuleIndex::new(module, parsed);

    assert!(cold.misses > 0);
    assert_eq!(warm.hits, files.len());
    assert_eq!(
        index.resolve(&SymbolTarget::Named {
            kind: Some("ActionResource".into()),
            name: "ActionPoint".into(),
        })[0]
            .definition()
            .uuid
            .unwrap()
            .to_string(),
        "dddddddd-dddd-dddd-dddd-dddddddddddd"
    );
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Named {
                kind: Some("ActionResource".into()),
                name: "ActionPoint".into(),
            }
    }));
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Named {
                kind: Some("PassiveData".into()),
                name: "CHAINED".into(),
            }
    }));
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Uuid("dddddddd-dddd-dddd-dddd-dddddddddddd".parse().unwrap())
    }));
    assert!(index.functions.contains_key("SelectSpells"));
}

#[test]
fn indexes_typed_lsx_localization_handles_without_resource_identity() {
    let handle = "h111111111111111111111111111111111111";
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Public/MyMod/Progressions/ProgressionDescriptions.lsx"),
            kind: SourceKind::Lsx,
        },
        &format!(
            r#"<node id="ProgressionDescription">
  <attribute id="Description" type="TranslatedString" handle="{handle}" version="1" />
  <attribute id="TechnicalName" type="LSString" handle="h222222222222222222222222222222222222" />
</node>"#
        ),
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert!(parsed.definitions.is_empty());
    assert_eq!(parsed.references.len(), 1);
    assert_eq!(
        parsed.references[0].target,
        SymbolTarget::Named {
            kind: Some("Localization".into()),
            name: handle.into(),
        }
    );
    assert_eq!(parsed.references[0].context, "localization");
}

#[test]
fn caches_and_invalidates_osiris_goal_records() {
    let directory = tempfile::tempdir().unwrap();
    let goal_dir = directory.path().join("Mods/MyMod/Story/RawFiles/Goals");
    fs::create_dir_all(&goal_dir).unwrap();
    let goal = goal_dir.join("CacheGoal.txt");
    let first_source = "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nPROC\nFirstProc()\nTHEN\nDB_Cached(1);\nEXITSECTION\nENDEXITSECTION\n";
    fs::write(&goal, first_source).unwrap();
    let module = ModuleSpec {
        name: "MyMod".into(),
        root: directory.path().to_path_buf(),
        role: ModuleRole::Project,
    };
    let files = discover_module(&module, directory.path(), "English", false).unwrap();
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();

    let (cold_files, cold) = cache
        .build_module(&module, &files, &SchemaCatalog::default(), "English")
        .unwrap();
    let (warm_files, warm) = cache
        .build_module(&module, &files, &SchemaCatalog::default(), "English")
        .unwrap();
    assert_eq!(cold.misses, 1);
    assert_eq!(warm.hits, 1);
    assert!(cold_files[0].osiris.is_some());
    assert!(warm_files[0].definitions.iter().any(|definition| {
        definition.kind == OSIRIS_PROCEDURE_KIND && definition.name == "FirstProc"
    }));

    fs::write(&goal, first_source.replace("FirstProc", "ChangedProcedure")).unwrap();
    let (changed_files, changed) = cache
        .build_module(&module, &files, &SchemaCatalog::default(), "English")
        .unwrap();
    assert_eq!(changed.misses, 1);
    assert!(changed_files[0].definitions.iter().any(|definition| {
        definition.kind == OSIRIS_PROCEDURE_KIND && definition.name == "ChangedProcedure"
    }));
}

#[test]
fn reads_uncompressed_and_lz4_v18_localization_packages() {
    let handle = "h111111111111111111111111111111111111";
    let generated_handle = "hsynthetic_g_suffix_1";
    let loca = synthetic_loca(&[
        (handle, 7, "Synthetic <LSTag>preview</LSTag>"),
        (generated_handle, 3, "Generated handle"),
    ]);
    for compression in [0, 2] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("English.pak");
        fs::write(&path, synthetic_package("English", &loca, compression)).unwrap();

        let catalog = read_localization_package(&path, "English").unwrap();
        assert_eq!(catalog.language(), "English");
        assert_eq!(catalog.get(handle).unwrap().version, 7);
        assert_eq!(
            catalog.get(handle).unwrap().text,
            "Synthetic <LSTag>preview</LSTag>"
        );
        assert_eq!(
            catalog.get(generated_handle).unwrap().text,
            "Generated handle"
        );
    }
}

#[test]
fn keeps_the_last_duplicate_localization_handle() {
    let handle = "hsynthetic_g_suffix_1";
    let catalog = bg3_index::LocalizationCatalog::from_entries(
        "English",
        [
            (handle.into(), 1, "First".into()),
            (handle.into(), 2, "Second".into()),
        ],
    )
    .unwrap();

    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog.get(handle).unwrap().version, 2);
    assert_eq!(catalog.get(handle).unwrap().text, "Second");
}

#[test]
fn preserves_loose_localization_entities_and_cdata() {
    let source = SourceFile {
        path: PathBuf::from("Localization/English/english.xml"),
        kind: SourceKind::Localization,
    };
    let parsed = parse_source(
        source,
        r#"<contentList><content contentuid="h111111111111111111111111111111111111" version="1">A &amp; B&#33; <![CDATA[<tag>]]></content></contentList>"#,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.definitions[0].fields["Text"], "A & B! <tag>");
}

#[test]
fn parses_localization_tooltip_references_in_all_supported_spellings() {
    let text = r#"<contentList>
<content contentuid="h111111111111111111111111111111111111" version="1">Encoded &lt;LSTag Tooltip="AttackRoll"&gt;attack&lt;/LSTag&gt;; mixed &lt;LSTag Type="Status" Tooltip='SLOWED'>slow&lt;/LSTag&gt;; literal <LSTag Tooltip="TEST_PASSIVE" Type="Passive">passive</LSTag>; dynamic <LSTag Type="Unknown" Tooltip="IGNORED">ignored</LSTag>.</content>
</contentList>"#;
    let parsed = parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Localization/English/english.xml"),
            kind: SourceKind::Localization,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.references.len(), 3);
    assert_eq!(
        parsed.references[0].target,
        SymbolTarget::Tooltip {
            name: "AttackRoll".into()
        }
    );
    assert_eq!(
        parsed.references[1].target,
        SymbolTarget::Named {
            kind: Some("StatusData".into()),
            name: "SLOWED".into(),
        }
    );
    assert_eq!(
        parsed.references[2].target,
        SymbolTarget::Named {
            kind: Some("PassiveData".into()),
            name: "TEST_PASSIVE".into(),
        }
    );
    for reference in &parsed.references {
        assert_eq!(reference.range.start.line, 1);
        let line = text.lines().nth(1).unwrap();
        let start = usize::try_from(reference.range.start.character).unwrap();
        let end = usize::try_from(reference.range.end.character).unwrap();
        assert!(matches!(
            &line[start..end],
            "AttackRoll" | "SLOWED" | "TEST_PASSIVE"
        ));
    }
}

#[test]
fn classifies_localization_xml_as_an_open_document() {
    assert_eq!(
        source_kind_for_document(Path::new(
            "/workspace/Mods/MyMod/Localization/English/english.xml"
        )),
        Some(SourceKind::Localization)
    );
}

#[test]
fn reads_only_static_entries_from_the_packed_tooltip_glossary() {
    let title = "h111111111111111111111111111111111111";
    let description = "h222222222222222222222222222222222222";
    let xaml = format!(
        r#"<ResourceDictionary xmlns:ls="synthetic">
<Style><Style.Triggers>
<Trigger Property="TagTooltip" Value="AttackRoll"><Setter><Setter.Value><ls:LSTooltip Content="{description}" ls:AttachedProperties.InheritedTag="{title}"/></Setter.Value></Setter></Trigger>
<Trigger Property="TagTooltip" Value="Dynamic"><Setter><Setter.Value><ls:LSTooltip Content="{{Binding RuntimeValue}}"/></Setter.Value></Setter></Trigger>
<Trigger Property="TagTooltip" Value="TitleOnly"><Setter><Setter.Value><ls:LSTooltip Tag="{title}"/></Setter.Value></Setter></Trigger>
</Style.Triggers></Style>
</ResourceDictionary>"#
    );
    let parsed = parse_tooltip_catalog(xaml.as_bytes()).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        parsed.get("AttackRoll").unwrap().title.as_deref(),
        Some(title)
    );
    assert_eq!(
        parsed.get("AttackRoll").unwrap().description.as_deref(),
        Some(description)
    );
    assert!(parsed.get("Dynamic").is_none());

    for compression in [0, 2] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("Game.pak"),
            synthetic_package_entry(
                "Public/Game/GUI/Library/Tooltips.xaml",
                xaml.as_bytes(),
                compression,
            ),
        )
        .unwrap();
        let catalog = read_base_tooltip_catalog(directory.path())
            .unwrap()
            .unwrap();
        assert_eq!(catalog, parsed);
    }
}

#[test]
fn rejects_unsafe_localization_package_variants() {
    let loca = synthetic_loca(&[("h111111111111111111111111111111111111", 1, "Synthetic")]);
    let directory = tempfile::tempdir().unwrap();

    let mut bad_signature = synthetic_package("English", &loca, 0);
    bad_signature[0] = b'X';
    let path = directory.path().join("bad-signature.pak");
    fs::write(&path, bad_signature).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut solid = synthetic_package("English", &loca, 0);
    solid[20] = 0x04;
    let path = directory.path().join("solid.pak");
    fs::write(&path, solid).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut multipart = synthetic_package("English", &loca, 0);
    multipart[38..40].copy_from_slice(&2_u16.to_le_bytes());
    let path = directory.path().join("multipart.pak");
    fs::write(&path, multipart).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut too_many = synthetic_package("English", &loca, 0);
    let file_list_offset =
        usize::try_from(u64::from_le_bytes(too_many[8..16].try_into().unwrap())).unwrap();
    too_many[file_list_offset..file_list_offset + 4].copy_from_slice(&100_001_u32.to_le_bytes());
    let path = directory.path().join("too-many.pak");
    fs::write(&path, too_many).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let path = directory.path().join("unsupported-compression.pak");
    fs::write(&path, synthetic_package("English", &loca, 3)).unwrap();
    assert!(read_localization_package(&path, "English").is_err());
}

#[test]
fn caches_and_invalidates_the_base_localization_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path().join("game");
    let localization = game.join("Localization");
    fs::create_dir_all(&localization).unwrap();
    let package = localization.join("English.pak");
    let handle = "h111111111111111111111111111111111111";
    fs::write(
        &package,
        synthetic_package("English", &synthetic_loca(&[(handle, 1, "First")]), 0),
    )
    .unwrap();
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();

    let (first, first_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(!first_hit);
    assert_eq!(first.get(handle).unwrap().text, "First");
    let (_, second_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(second_hit);

    fs::write(
        &package,
        synthetic_package(
            "English",
            &synthetic_loca(&[(handle, 2, "Second value")]),
            0,
        ),
    )
    .unwrap();
    let (changed, changed_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(!changed_hit);
    assert_eq!(changed.get(handle).unwrap().text, "Second value");

    let missing = read_base_localization_package(directory.path(), "English").unwrap();
    assert!(missing.is_none());
}

#[test]
fn rejects_unsafe_localization_languages_without_probing_escaped_packages() {
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path().join("game");
    let localization = game.join("Localization");
    fs::create_dir_all(&localization).unwrap();
    let handle = "h111111111111111111111111111111111111";
    fs::write(
        localization.join("English.pak"),
        synthetic_package("English", &synthetic_loca(&[(handle, 1, "First")]), 0),
    )
    .unwrap();
    // A fully valid synthetic package also sits at the escaped traversal
    // target, so a loader that probed outside paths would succeed reading it.
    let escaped = directory.path().join("outside.pak");
    fs::write(
        &escaped,
        synthetic_package("English", &synthetic_loca(&[(handle, 1, "Escaped")]), 0),
    )
    .unwrap();
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();

    for language in [
        "../../outside",
        "../outside",
        "sub/English",
        "C:\\Evil",
        ":stream",
        "Eng\0lish",
        "English\n",
        " ",
    ] {
        assert!(
            cache.load_base_localization(&game, language).is_err(),
            "the cache loader accepted {language:?}"
        );
        assert!(
            read_base_localization_package(&game, language).is_err(),
            "the package reader accepted {language:?}"
        );
    }

    let catalog = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert_eq!(catalog.0.get(handle).unwrap().text, "First");
}

#[test]
fn caches_and_invalidates_the_base_tooltip_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path().join("game");
    fs::create_dir_all(&game).unwrap();
    let package = game.join("Game.pak");
    let first = br#"<Root xmlns:ls="synthetic"><Trigger Property="TagTooltip" Value="First"><ls:LSTooltip Content="h111111111111111111111111111111111111"/></Trigger></Root>"#;
    fs::write(
        &package,
        synthetic_package_entry("Public/Game/GUI/Library/Tooltips.xaml", first, 0),
    )
    .unwrap();
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();

    let (catalog, first_hit) = cache.load_base_tooltips(&game).unwrap().unwrap();
    assert!(!first_hit);
    assert!(catalog.get("First").is_some());
    let (_, second_hit) = cache.load_base_tooltips(&game).unwrap().unwrap();
    assert!(second_hit);

    let second = br#"<Root xmlns:ls="synthetic"><Trigger Property="TagTooltip" Value="SecondChanged"><ls:LSTooltip Content="h222222222222222222222222222222222222"/></Trigger></Root>"#;
    fs::write(
        &package,
        synthetic_package_entry("Public/Game/GUI/Library/Tooltips.xaml", second, 0),
    )
    .unwrap();
    let (changed, changed_hit) = cache.load_base_tooltips(&game).unwrap().unwrap();
    assert!(!changed_hit);
    assert!(changed.get("First").is_none());
    assert!(changed.get("SecondChanged").is_some());
}

#[test]
fn discards_corrupt_and_schema_obsolete_cache_objects() {
    let root = fixtures();
    let game = root.join("game");
    let mut schema = load_schema(&root);
    let module = ModuleSpec {
        name: "Shared".into(),
        root: game.join("Editor/Mods/Shared"),
        role: ModuleRole::Base,
    };
    let files = discover_module(&module, &game, "English", true).unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = CacheStore::new(cache_dir.path().to_path_buf()).unwrap();
    cache
        .build_module(&module, &files, &schema, "English")
        .unwrap();

    let object = fs::read_dir(cache_dir.path().join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(object, b"corrupt").unwrap();
    let (_, recovered) = cache
        .build_module(&module, &files, &schema, "English")
        .unwrap();
    assert_eq!(recovered.misses, 1);

    schema
        .enumerations
        .entry("YesNo".into())
        .or_default()
        .push("SchemaChanged".into());
    let (_, invalidated) = cache
        .build_module(&module, &files, &schema, "English")
        .unwrap();
    assert_eq!(invalidated.misses, files.len());
}

#[test]
fn caches_and_rebuilds_changed_thoth_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let helper = root.join("Mods/Test/Scripts/thoth/helpers/Test.khn");
    fs::create_dir_all(helper.parent().unwrap()).unwrap();
    fs::write(&helper, "function First(value)\n  return value\nend\n").unwrap();
    let module = ModuleSpec {
        name: "Test".into(),
        root: root.clone(),
        role: ModuleRole::Project,
    };
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();
    let schema = SchemaCatalog::default();

    let sources = discover_module(&module, directory.path(), "English", false).unwrap();
    let (_, cold) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    let (_, warm) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    assert_eq!(cold.misses, 1);
    assert_eq!(warm.hits, 1);

    fs::write(
        &helper,
        "function Changed(value, fallback)\n  return value or fallback\nend\n",
    )
    .unwrap();
    let (changed, changed_stats) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    assert_eq!(changed_stats.misses, 1);
    assert_eq!(changed[0].definitions[0].name, "Changed");

    fs::remove_file(&helper).unwrap();
    let removed_sources = discover_module(&module, directory.path(), "English", false).unwrap();
    let (removed, _) = cache
        .build_module(&module, &removed_sources, &schema, "English")
        .unwrap();
    assert!(removed.is_empty());
}
