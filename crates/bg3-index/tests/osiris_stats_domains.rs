use bg3_index::{
    OSIRIS_ARGUMENT_DOMAIN_RECORDS, OSIRIS_CONTRACTS, OsirisArgumentDisposition,
    OsirisContractKind, OsirisParameterDirection, SchemaCatalog, SourceFile, SourceKind,
    SymbolTarget, osiris_contract_by_kind, parse_source,
};
use std::path::PathBuf;

fn args(
    contract: &bg3_index::OsirisContractSpec,
    target: Option<usize>,
    target_name: &str,
) -> String {
    contract
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            if target == Some(index)
                && matches!(
                    parameter.direction,
                    OsirisParameterDirection::In | OsirisParameterDirection::InOut
                )
            {
                format!("L\"{target_name}\"")
            } else {
                format!("_Arg{index}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn call(kind: OsirisContractKind, name: &str, arguments: &str) -> String {
    match kind {
        OsirisContractKind::Event => format!("{name}({arguments})"),
        OsirisContractKind::Call => format!("{name}({arguments});"),
        OsirisContractKind::Query => format!("{name}({arguments})"),
        OsirisContractKind::Syscall | OsirisContractKind::Sysquery => {
            panic!("the reviewed Stats domains contain no system callable")
        }
    }
}

fn parse_goal(text: &str) -> bg3_index::ParsedFile {
    parse_source(
        SourceFile {
            path: PathBuf::from("Mods/Test/Story/RawFiles/Goals/StatsDomains.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .expect("synthetic Osiris goal parses")
}

fn reviewed_rows() -> Vec<&'static bg3_index::OsirisArgumentDomainRecord> {
    OSIRIS_ARGUMENT_DOMAIN_RECORDS
        .iter()
        .filter(|record| {
            matches!(
                record.disposition,
                OsirisArgumentDisposition::Resource(
                    bg3_index::OsirisResourceDomain::StatusData
                        | bg3_index::OsirisResourceDomain::SpellData
                        | bg3_index::OsirisResourceDomain::PassiveData
                        | bg3_index::OsirisResourceDomain::InterruptData
                        | bg3_index::OsirisResourceDomain::SpellSet
                        | bg3_index::OsirisResourceDomain::TreasureTable
                        | bg3_index::OsirisResourceDomain::Equipment
                )
            )
        })
        .collect()
}

#[test]
fn indexes_every_reviewed_stats_domain_at_its_exact_callable_position() {
    let rows = reviewed_rows();
    assert_eq!(
        rows.len(),
        48,
        "the reviewed activation set must stay bounded"
    );

    let mut source = String::from("Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n");
    for (row_number, row) in rows.iter().enumerate() {
        let contract = osiris_contract_by_kind(OSIRIS_CONTRACTS, row.kind, row.name, row.arity)
            .expect("every reviewed row has an exact generated contract");
        let value = format!("DOMAIN_{row_number}");
        let arguments = args(contract, Some(row.index), &value);
        match row.kind {
            OsirisContractKind::Event => {
                source.push_str("IF\n");
                source.push_str(&call(row.kind, row.name, &arguments));
                source.push_str("\nTHEN\nDB_Observed(_Head);\n");
            }
            OsirisContractKind::Query => {
                source.push_str("IF\nDied(_Head)\nAND\n");
                source.push_str(&call(row.kind, row.name, &arguments));
                source.push_str("\nTHEN\nDB_Observed(_Head);\n");
            }
            OsirisContractKind::Call => {
                source.push_str("IF\nDied(_Head)\nTHEN\n");
                source.push_str(&call(row.kind, row.name, &arguments));
                source.push('\n');
            }
            OsirisContractKind::Syscall | OsirisContractKind::Sysquery => {
                panic!("system callable is outside the reviewed set")
            }
        }
    }
    source.push_str("EXITSECTION\nENDEXITSECTION\n");

    let parsed = parse_goal(&source);
    assert!(
        parsed.issues.is_empty(),
        "synthetic rows are valid: {:?}",
        parsed.issues
    );
    for (row_number, row) in rows.iter().enumerate() {
        let expected_name = format!("DOMAIN_{row_number}");
        let expected_kind = row
            .disposition
            .resource_domain()
            .expect("reviewed row is an active resource");
        assert!(
            parsed.references.iter().any(|reference| {
                reference.target
                    == SymbolTarget::Named {
                        kind: Some(expected_kind.into()),
                        name: expected_name.clone(),
                    }
            }),
            "missing exact {expected_kind} reference for {:?} {}/{} argument {}",
            row.kind,
            row.name,
            row.arity,
            row.index
        );
    }
}

#[test]
fn ignores_adjacent_unreviewed_strings_wrong_roles_and_query_outputs() {
    let status_event = osiris_contract_by_kind(
        OSIRIS_CONTRACTS,
        OsirisContractKind::Event,
        "StatusApplied",
        4,
    )
    .expect("StatusApplied contract");
    let cast = osiris_contract_by_kind(OSIRIS_CONTRACTS, OsirisContractKind::Event, "CastSpell", 5)
        .expect("CastSpell contract");
    let spell_set = osiris_contract_by_kind(
        OSIRIS_CONTRACTS,
        OsirisContractKind::Query,
        "GetSpellFromSet",
        3,
    )
    .expect("GetSpellFromSet contract");
    let call_contract =
        osiris_contract_by_kind(OSIRIS_CONTRACTS, OsirisContractKind::Call, "AddSpell", 4)
            .expect("AddSpell contract");
    let output_args = spell_set
        .parameters
        .iter()
        .enumerate()
        .map(|(index, _)| match index {
            0 => "L\"SET\"",
            2 => "L\"OUTPUT\"",
            _ => "_A",
        })
        .collect::<Vec<_>>()
        .join(",");
    let cast_args = cast
        .parameters
        .iter()
        .enumerate()
        .map(|(index, _)| match index {
            1 => "L\"APPROVED\"",
            2 => "L\"ENUM\"",
            _ => "_A",
        })
        .collect::<Vec<_>>()
        .join(",");
    let valid = format!(
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n\
IF\n{}\nTHEN\n{}\n\
IF\nDied(_Head)\nAND\n{}\nTHEN\nDB_Observed(_Head);\n\
IF\n{}\nTHEN\nDB_Observed(_Head);\n\
EXITSECTION\nENDEXITSECTION\n",
        call(
            OsirisContractKind::Event,
            "StatusApplied",
            &args(status_event, Some(1), "STATUS"),
        ),
        call(
            OsirisContractKind::Call,
            "AddSpell",
            &args(call_contract, Some(1), "CALL"),
        ),
        call(OsirisContractKind::Query, "GetSpellFromSet", &output_args),
        call(OsirisContractKind::Event, "CastSpell", &cast_args),
    );

    let parsed = parse_goal(&valid);
    assert!(
        parsed.issues.is_empty(),
        "valid negative fixtures are parseable: {:?}",
        parsed.issues
    );
    let named = |name: &str| {
        parsed.references.iter().any(|reference| {
            matches!(&reference.target, SymbolTarget::Named { name: value, .. } if value == name)
        })
    };
    for accepted in ["STATUS", "CALL", "SET", "APPROVED"] {
        assert!(
            named(accepted),
            "approved literal {accepted} was not indexed"
        );
    }
    for rejected in ["OUTPUT", "ENUM"] {
        assert!(
            !named(rejected),
            "unreviewed literal {rejected} became a resource"
        );
    }

    let query = osiris_contract_by_kind(OSIRIS_CONTRACTS, OsirisContractKind::Query, "HasSpell", 3)
        .expect("HasSpell contract");
    let teleported =
        osiris_contract_by_kind(OSIRIS_CONTRACTS, OsirisContractKind::Event, "Teleported", 9)
            .expect("Teleported contract");
    let rejected = parse_goal(&format!(
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n\
IF\n{}\nTHEN\nDB_Observed(_Head);\n\
IF\nDied(_Head)\nTHEN\n{};\n\
IF\nDied(_Head)\nAND\n{}\nTHEN\nDB_Observed(_Head);\n\
IF\n{}\nTHEN\nDB_Observed(_Head);\n\
IF\nDied(_Head)\nAND\n{}\nTHEN\nDB_Observed(_Head);\n\
IF\nDied(_Head)\nTHEN\n{};\n\
IF\nDied(_Head)\nTHEN\nAddSpell(L\"WRONG_ARITY\",_A,_B);\n\
IF\nDied(_Head)\nTHEN\nAddSpell(_Variable,_A,_B,_C);\n\
IF\nDied(_Head)\nTHEN\nUserProcedure(L\"USER_CALL\");\n\
EXITSECTION\nENDEXITSECTION\n",
        call(
            OsirisContractKind::Call,
            "AddSpell",
            &args(call_contract, Some(1), "WRONG_HEAD"),
        )
        .trim_end_matches(';'),
        call(
            OsirisContractKind::Event,
            "StatusApplied",
            &args(status_event, Some(1), "WRONG_ACTION"),
        ),
        call(
            OsirisContractKind::Query,
            "HasSpell",
            &args(query, Some(1), "QUERY_CONDITION"),
        ),
        call(
            OsirisContractKind::Event,
            "Teleported",
            &args(teleported, Some(8), "INVALID"),
        ),
        call(
            OsirisContractKind::Event,
            "StatusApplied",
            &args(status_event, Some(1), "WRONG_CONDITION_EVENT"),
        ),
        call(
            OsirisContractKind::Query,
            "HasSpell",
            &args(query, Some(1), "WRONG_ACTION"),
        ),
    ));
    assert!(
        rejected
            .issues
            .iter()
            .all(|issue| issue.code == "osiris-invalid-callable-role")
    );
    assert_eq!(
        rejected.issues.len(),
        4,
        "only the four invalid placements are diagnosed: {:?}",
        rejected.issues
    );
    for (name, message) in [
        ("AddSpell", "cannot trigger an IF rule"),
        ("StatusApplied", "cannot be used as an action"),
        ("HasSpell", "cannot be used as an action"),
        ("StatusApplied", "cannot be used as a condition"),
    ] {
        assert!(
            rejected
                .issues
                .iter()
                .any(|issue| { issue.message.contains(name) && issue.message.contains(message) })
        );
    }
    assert!(
        rejected.references.iter().any(|reference| {
            matches!(&reference.target, SymbolTarget::Named { name, .. } if name == "QUERY_CONDITION")
        }),
        "a query condition is a valid positive control"
    );
    for literal in [
        "WRONG_HEAD",
        "WRONG_ACTION",
        "WRONG_CONDITION_EVENT",
        "INVALID",
        "WRONG_ARITY",
        "USER_CALL",
    ] {
        assert!(
            !rejected.references.iter().any(|reference| {
                matches!(&reference.target, SymbolTarget::Named { name, .. } if name == literal)
            }),
            "invalid literal {literal} became a resource"
        );
    }
}
