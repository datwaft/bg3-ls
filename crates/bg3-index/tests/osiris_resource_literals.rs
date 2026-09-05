use std::path::PathBuf;

use bg3_index::{SchemaCatalog, SourceFile, SourceKind, SymbolTarget, parse_source};

fn parse_goal(text: &str) -> bg3_index::ParsedFile {
    parse_source(
        SourceFile {
            path: PathBuf::from("Mods/MyMod/Story/RawFiles/Goals/ResourceLiterals.txt"),
            kind: SourceKind::Osiris,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .expect("synthetic Osiris goal parses")
}

fn resource_literal_names(parsed: &bg3_index::ParsedFile) -> Vec<(&str, &str)> {
    parsed
        .references
        .iter()
        .filter_map(|reference| match &reference.target {
            SymbolTarget::Named {
                kind: Some(kind),
                name,
            } => Some((kind.as_str(), name.as_str())),
            _ => None,
        })
        .collect()
}

#[test]
fn indexes_only_string_typed_resource_literals() {
    let parsed = parse_goal(concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "StatusApplied((CHARACTER)_Object,L\"UNCast\",_Cause,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "StatusApplied((CHARACTER)_Object,(STRING)L\"StringCast\",_Cause,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "StatusApplied((CHARACTER)_Object,(GUIDSTRING)L\"GuidCast\",_Cause,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "StatusApplied((CHARACTER)_Object,(CHARACTER)L\"AliasCast\",_Cause,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    ));

    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    assert_eq!(
        resource_literal_names(&parsed),
        [("StatusData", "UNCast"), ("StatusData", "StringCast")]
    );
}

#[test]
fn skips_empty_resource_literal_sentinels() {
    let parsed = parse_goal(concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "RandomCastProcessed(_Caster,1,L\"\",1,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "RandomCastProcessed(_Caster,1,L\"SPELL_FIREBALL\",1,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "ShapeshiftChanged(_Character,L\"RACE\",L\"GENDER\",L\"\")\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "ShapeshiftChanged(_Character,L\"RACE\",L\"GENDER\",L\"STATUS_SHAPE\")\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    ));

    assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    assert_eq!(
        resource_literal_names(&parsed),
        [
            ("SpellData", "SPELL_FIREBALL"),
            ("StatusData", "STATUS_SHAPE")
        ]
    );
}

#[test]
fn does_not_index_resource_literals_from_syntax_recovery() {
    let parsed = parse_goal(concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "StatusApplied((CHARACTER)_Object,L\"Recovered\",_Cause,1)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
        "BROKEN\n",
    ));

    assert!(!parsed.issues.is_empty());
    assert!(resource_literal_names(&parsed).is_empty());
}
