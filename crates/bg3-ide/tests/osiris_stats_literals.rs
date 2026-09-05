use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{DiagnosticSeverity, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, PackagedStatsCatalog, PackagedStatsSource, Position,
    SchemaCatalog, SourceFile, SourceKind, SymbolTarget, parse_source,
};

fn position(text: &str, needle: &str, occurrence: usize) -> Position {
    let offset = text
        .match_indices(needle)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("missing occurrence {occurrence} of {needle:?}"))
        .0;
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let character = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count();
    Position {
        line: u32::try_from(line).unwrap(),
        character: u32::try_from(character).unwrap(),
    }
}

fn stats_source(schema: &SchemaCatalog, path: &Path, text: &str) -> bg3_index::ParsedFile {
    parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::PlainStats,
        },
        text,
        schema,
        "English",
    )
    .expect("synthetic Stats source")
}

fn osiris_source(path: &Path, text: &str, schema: &SchemaCatalog) -> bg3_index::ParsedFile {
    parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::Osiris,
        },
        text,
        schema,
        "English",
    )
    .expect("synthetic Osiris source")
}

fn module(
    schema: &SchemaCatalog,
    name: &str,
    role: ModuleRole,
    files: &[(&str, SourceKind, &str)],
) -> Arc<ModuleIndex> {
    let parsed = files
        .iter()
        .map(|(relative, kind, text)| {
            let path = PathBuf::from(format!("/synthetic/{name}/{relative}"));
            parse_source(SourceFile { path, kind: *kind }, text, schema, "English")
                .expect("synthetic source")
        })
        .collect();
    Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: name.into(),
            root: PathBuf::from(format!("/synthetic/{name}")),
            role,
        },
        parsed,
    ))
}

fn overlay(path: &Path, module: &str, text: &str, schema: &SchemaCatalog) -> OverlaySet {
    let parsed = stats_source(schema, path, text);
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: module.into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
    overlays
}

fn resource_stats() -> String {
    concat!(
        "new entry \"STATUS_READY\"\n",
        "type \"StatusData\"\n\n",
        "new entry \"SPELL_READY\"\n",
        "type \"SpellData\"\n\n",
        "new entry \"PASSIVE_READY\"\n",
        "type \"PassiveData\"\n\n",
        "new entry \"INTERRUPT_READY\"\n",
        "type \"InterruptData\"\n\n",
        "new spellset \"SPELL_SET_READY\"\n\n",
        "new treasuretable \"TREASURE_READY\"\n\n",
        "new equipment \"EQUIPMENT_READY\"\n",
    )
    .into()
}

fn resource_goal() -> String {
    concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "StatusApplied((GUIDSTRING)_Object,L\"STATUS_READY\",_Cause,1)\n",
        "THEN\n",
        "RemoveStatus((GUIDSTRING)_Object,L\"STATUS_READY\",_Cause);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "AddSpell((CHARACTER)_Object,L\"SPELL_READY\",1,0);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "HasPassive((GUIDSTRING)_Object,L\"PASSIVE_READY\",_HasPassive)\n",
        "THEN\n",
        "DB_Noop(_HasPassive);\n",
        "IF\n",
        "ReactionInterruptUsed((GUIDSTRING)_Object,L\"INTERRUPT_READY\",0)\n",
        "THEN\n",
        "DB_Noop(1);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "GetSpellFromSet(L\"SPELL_SET_READY\",1,_SpellID)\n",
        "THEN\n",
        "DB_Noop(1);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "GenerateTreasure((GUIDSTRING)_Object,L\"TREASURE_READY\",1,(CHARACTER)_Finder);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "CharacterGiveEquipmentSet((CHARACTER)_Object,L\"EQUIPMENT_READY\");\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    )
    .into()
}

#[test]
fn proven_osiris_resource_literals_support_loose_ide_navigation() {
    let schema = SchemaCatalog::default();
    let stats_path = PathBuf::from("/synthetic/Project/Stats/Resources.txt");
    let goal_path = PathBuf::from("/synthetic/Project/Story/RawFiles/Goals/Resources.txt");
    let stats = resource_stats();
    let goal = resource_goal();
    let project = module(
        &schema,
        "Project",
        ModuleRole::Project,
        &[
            ("Stats/Resources.txt", SourceKind::PlainStats, &stats),
            (
                "Story/RawFiles/Goals/Resources.txt",
                SourceKind::Osiris,
                &goal,
            ),
        ],
    );
    let workspace = WorkspaceSnapshot::new(Arc::new(schema), vec![project], 1, 200, 200);
    let expected = [
        ("STATUS_READY", "StatusData", 2),
        ("SPELL_READY", "SpellData", 1),
        ("PASSIVE_READY", "PassiveData", 1),
        ("INTERRUPT_READY", "InterruptData", 1),
        ("SPELL_SET_READY", "SpellSet", 1),
        ("TREASURE_READY", "TreasureTable", 1),
        ("EQUIPMENT_READY", "Equipment", 1),
    ];

    for (name, kind, reference_count) in expected {
        let reference_position = position(&goal, name, 0);
        let definitions =
            workspace.definitions_at(&goal_path, reference_position, &OverlaySet::default());
        assert_eq!(definitions.len(), 1, "definitions for {kind} {name}");
        assert_eq!(definitions[0].module, "Project");
        assert_eq!(definitions[0].path, stats_path);
        assert_eq!(definitions[0].definition.kind, kind);
        assert!(!definitions[0].ambiguous);
        let definition_locations = workspace.definition_locations_at(
            &goal_path,
            reference_position,
            &OverlaySet::default(),
        );
        assert_eq!(definition_locations.len(), 1, "locations for {kind} {name}");
        assert_eq!(definition_locations[0].path, stats_path);

        let hover = workspace
            .hover(&goal_path, reference_position, &OverlaySet::default())
            .unwrap_or_else(|| panic!("hover for {kind} {name}"));
        assert!(hover.contains(&format!("**{kind}** `{name}`")), "{hover}");

        let references = workspace.references_at(
            &goal_path,
            reference_position,
            false,
            &OverlaySet::default(),
        );
        assert_eq!(
            references.len(),
            reference_count,
            "references for {kind} {name}"
        );
        assert!(references.iter().all(|location| location.path == goal_path));
    }
}

#[test]
fn same_rank_resource_declarations_are_ambiguous_and_precedence_is_preserved() {
    let schema = SchemaCatalog::default();
    let caller_path = PathBuf::from("/synthetic/Project/Story/RawFiles/Goals/Caller.txt");
    let caller = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "HasPassive((GUIDSTRING)_Object,L\"AMBIGUOUS_PASSIVE\",_Has)\n",
        "THEN\n",
        "DB_Noop(_Has);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "HasPassive((GUIDSTRING)_Object,L\"OVERRIDE_PASSIVE\",_Has)\n",
        "THEN\n",
        "DB_Noop(_Has);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let duplicate_a = "new entry \"AMBIGUOUS_PASSIVE\"\ntype \"PassiveData\"\n";
    let duplicate_b = "new entry \"AMBIGUOUS_PASSIVE\"\ntype \"PassiveData\"\n";
    let base = module(
        &schema,
        "Base",
        ModuleRole::Base,
        &[
            ("Stats/A_Base.txt", SourceKind::PlainStats, duplicate_a),
            ("Stats/B_Base.txt", SourceKind::PlainStats, duplicate_b),
        ],
    );
    let dependency = module(
        &schema,
        "Dependency",
        ModuleRole::Dependency,
        &[(
            "Stats/Dependency.txt",
            SourceKind::PlainStats,
            "new entry \"OVERRIDE_PASSIVE\"\ntype \"PassiveData\"\n",
        )],
    );
    let project = module(
        &schema,
        "Project",
        ModuleRole::Project,
        &[
            (
                "Stats/Project.txt",
                SourceKind::PlainStats,
                "new entry \"OVERRIDE_PASSIVE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n",
            ),
            (
                "Story/RawFiles/Goals/Caller.txt",
                SourceKind::Osiris,
                caller,
            ),
        ],
    );
    let workspace = WorkspaceSnapshot::new(
        Arc::new(schema),
        vec![base, dependency, project],
        1,
        200,
        200,
    );

    let ambiguous_position = position(caller, "AMBIGUOUS_PASSIVE", 0);
    let ambiguous =
        workspace.definitions_at(&caller_path, ambiguous_position, &OverlaySet::default());
    assert_eq!(ambiguous.len(), 2);
    assert!(ambiguous.iter().all(|definition| definition.ambiguous));
    let ambiguous_hover = workspace
        .hover(&caller_path, ambiguous_position, &OverlaySet::default())
        .expect("ambiguous passive hover");
    assert!(
        ambiguous_hover.contains("same-rank ambiguity"),
        "{ambiguous_hover}"
    );

    let overlay_path = PathBuf::from("/synthetic/Project/Stats/Project.txt");
    let overlay_text =
        "new entry \"OVERRIDE_PASSIVE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"No\"\n";
    let overlays = overlay(
        &overlay_path,
        "Project",
        overlay_text,
        workspace.schema.as_ref(),
    );
    let override_position = position(caller, "OVERRIDE_PASSIVE", 0);
    let resolved = workspace.definitions_at(&caller_path, override_position, &overlays);
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].module, "Project");
    assert_eq!(resolved[0].path, overlay_path);
    assert_eq!(resolved[0].rank, 2);
    assert_eq!(resolved[1].module, "Dependency");
    assert_eq!(resolved[1].rank, 1);
    assert!(resolved.iter().all(|definition| !definition.ambiguous));
    let hover = workspace
        .hover(&caller_path, override_position, &overlays)
        .expect("overlay passive hover");
    assert!(hover.contains("data \"Enabled\" \"No\""), "{hover}");
    assert!(!hover.contains("data \"Enabled\" \"Yes\""), "{hover}");
}

fn packaged_spell(
    schema: &SchemaCatalog,
    package: &str,
    module: &str,
    name: &str,
) -> PackagedStatsSource {
    let entry = format!("Public/{module}/Stats/Generated/Data/Spell_{name}.txt");
    let text = format!("new entry \"{name}\"\ntype \"SpellData\"\n");
    let parsed = stats_source(schema, Path::new(&entry), &text);
    PackagedStatsSource::new(module, package, entry, 0, parsed).expect("packaged Stats source")
}

#[test]
fn packaged_resource_provenance_is_virtual_and_unconfigured_modules_stay_hidden() {
    let schema = Arc::new(SchemaCatalog::default());
    let caller_path = PathBuf::from("/synthetic/Project/Story/RawFiles/Goals/Caller.txt");
    let caller = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "AddSpell((CHARACTER)_Object,L\"PACKED_SPELL\",1,0);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "AddSpell((CHARACTER)_Object,L\"HIDDEN_SPELL\",1,0);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let shared = module(&schema, "Shared", ModuleRole::Base, &[]);
    let project = module(
        &schema,
        "Project",
        ModuleRole::Project,
        &[(
            "Story/RawFiles/Goals/Caller.txt",
            SourceKind::Osiris,
            caller,
        )],
    );
    let catalog = PackagedStatsCatalog::from_sources([
        packaged_spell(&schema, "/synthetic/Shared.pak", "Shared", "PACKED_SPELL"),
        packaged_spell(&schema, "/synthetic/Hidden.pak", "Hidden", "HIDDEN_SPELL"),
    ])
    .expect("packaged Stats catalog");
    let workspace = WorkspaceSnapshot::new(schema, vec![shared, project], 1, 200, 200)
        .with_packaged_stats(Arc::new(catalog))
        .with_incomplete_kinds(["SpellData"]);

    let packed_position = position(caller, "PACKED_SPELL", 0);
    let packed = workspace
        .definitions_at(&caller_path, packed_position, &OverlaySet::default())
        .pop()
        .expect("packaged definition");
    assert_eq!(packed.module, "Shared");
    assert_eq!(packed.path, PathBuf::from("/synthetic/Shared.pak"));
    assert_eq!(
        packed.packaged_entry.as_deref(),
        Some("Public/Shared/Stats/Generated/Data/Spell_PACKED_SPELL.txt")
    );
    let packed_hover = workspace
        .hover(&caller_path, packed_position, &OverlaySet::default())
        .expect("packaged hover");
    assert!(
        packed_hover
            .contains("Package entry: `Public/Shared/Stats/Generated/Data/Spell_PACKED_SPELL.txt`")
    );
    assert!(
        workspace
            .definition_locations_at(&caller_path, packed_position, &OverlaySet::default())
            .is_empty()
    );
    let packed_references =
        workspace.references_at(&caller_path, packed_position, true, &OverlaySet::default());
    assert_eq!(packed_references.len(), 1);
    assert_eq!(packed_references[0].path, caller_path);

    let hidden_position = position(caller, "HIDDEN_SPELL", 0);
    assert!(
        workspace
            .definitions_at(&caller_path, hidden_position, &OverlaySet::default())
            .is_empty()
    );
    assert!(
        workspace
            .hover(&caller_path, hidden_position, &OverlaySet::default())
            .is_none()
    );
    assert!(
        workspace
            .definition_locations_at(&caller_path, hidden_position, &OverlaySet::default())
            .is_empty()
    );
    let hidden_references =
        workspace.references_at(&caller_path, hidden_position, false, &OverlaySet::default());
    assert_eq!(hidden_references.len(), 1);
    assert_eq!(hidden_references[0].path, caller_path);
    assert!(
        workspace
            .diagnostics(
                &caller_path,
                &OverlaySet::default(),
                Some(DiagnosticSeverity::Warning),
            )
            .is_empty()
    );
}

#[test]
fn absent_resource_literals_keep_typed_references_without_missing_diagnostics() {
    let schema = Arc::new(SchemaCatalog::default());
    let caller_path = PathBuf::from("/synthetic/Project/Story/RawFiles/Goals/Missing.txt");
    let caller = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "StatusApplied((GUIDSTRING)_Object,L\"MISSING_STATUS\",_Cause,1)\n",
        "THEN\n",
        "RemoveStatus((GUIDSTRING)_Object,L\"MISSING_STATUS\",_Cause);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "AddSpell((CHARACTER)_Object,L\"MISSING_SPELL\",1,0);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "HasPassive((GUIDSTRING)_Object,L\"MISSING_PASSIVE\",_Has)\n",
        "THEN\n",
        "DB_Noop(_Has);\n",
        "IF\n",
        "ReactionInterruptUsed((GUIDSTRING)_Object,L\"MISSING_INTERRUPT\",0)\n",
        "THEN\n",
        "DB_Noop(1);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "AND\n",
        "GetSpellFromSet(L\"MISSING_SPELL_SET\",1,_SpellID)\n",
        "THEN\n",
        "DB_Noop(1);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "GenerateTreasure((GUIDSTRING)_Object,L\"MISSING_TREASURE\",1,(CHARACTER)_Finder);\n",
        "IF\n",
        "TextEvent(\"go\")\n",
        "THEN\n",
        "CharacterGiveEquipmentSet((CHARACTER)_Object,L\"MISSING_EQUIPMENT\");\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let parsed = osiris_source(&caller_path, caller, schema.as_ref());
    let expected = [
        ("MISSING_STATUS", "StatusData"),
        ("MISSING_SPELL", "SpellData"),
        ("MISSING_PASSIVE", "PassiveData"),
        ("MISSING_INTERRUPT", "InterruptData"),
        ("MISSING_SPELL_SET", "SpellSet"),
        ("MISSING_TREASURE", "TreasureTable"),
        ("MISSING_EQUIPMENT", "Equipment"),
    ];
    for (name, kind) in expected {
        let reference = parsed
            .references
            .iter()
            .find(|reference| {
                reference.context == "osiris-string-literal"
                    && reference.target
                        == (SymbolTarget::Named {
                            kind: Some(kind.into()),
                            name: name.into(),
                        })
            })
            .unwrap_or_else(|| panic!("typed reference for {kind} {name}"));
        assert_eq!(reference.range, position_range(caller, name));
    }
    let project = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Project".into(),
            root: PathBuf::from("/synthetic/Project"),
            role: ModuleRole::Project,
        },
        vec![parsed],
    ));
    let workspace = WorkspaceSnapshot::new(schema, vec![project], 1, 200, 200);
    for (name, kind) in expected {
        let at = position(caller, name, 0);
        assert!(
            workspace
                .definitions_at(&caller_path, at, &OverlaySet::default())
                .is_empty()
        );
        let references = workspace.references_at(&caller_path, at, false, &OverlaySet::default());
        let expected_references = usize::from(kind == "StatusData") + 1;
        assert_eq!(
            references.len(),
            expected_references,
            "references for {kind} {name}"
        );
    }
    assert!(
        workspace
            .diagnostics(
                &caller_path,
                &OverlaySet::default(),
                Some(DiagnosticSeverity::Warning),
            )
            .is_empty()
    );
}

fn position_range(text: &str, needle: &str) -> bg3_index::TextRange {
    let start = position(text, needle, 0);
    bg3_index::TextRange {
        start,
        end: Position {
            line: start.line,
            character: start.character + u32::try_from(needle.len()).unwrap(),
        },
    }
}
