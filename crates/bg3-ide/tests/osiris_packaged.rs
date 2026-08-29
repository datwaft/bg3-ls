use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{CompletionKind, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, OSIRIS_FACTS_EXTRACTOR_VERSION, PackagedOsirisIndex,
    PackagedOsirisResolution, PackagedThothCatalog, PackagedThothSource, Position, SchemaCatalog,
    SourceFile, SourceKind, parse_osiris_goal_source, parse_packaged_thoth_facts, parse_source,
};

fn osiris_source(
    module: &str,
    entry: &str,
    package: &str,
    priority: u8,
    text: &str,
) -> PackagedThothSource {
    PackagedThothSource::new(module, package, entry, priority, text).expect("synthetic source")
}

fn goal_text(body: &str) -> String {
    format!(
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n{body}\nEXITSECTION\nENDEXITSECTION\n"
    )
}

fn proc_declaration(signature: &str) -> String {
    goal_text(&format!("PROC\n{signature}\nTHEN\nDB_Noop(1);"))
}

fn query_declaration(signature: &str) -> String {
    goal_text(&format!("QRY\n{signature}\nTHEN\nDB_Noop(1);"))
}

fn index(catalog: &PackagedThothCatalog) -> Arc<PackagedOsirisIndex> {
    let facts = parse_packaged_thoth_facts(
        catalog,
        OSIRIS_FACTS_EXTRACTOR_VERSION,
        parse_osiris_goal_source,
    )
    .expect("synthetic goal facts");
    Arc::new(PackagedOsirisIndex::from_catalog_and_facts(
        catalog,
        Arc::new(facts).as_ref(),
    ))
}

fn base_entry(name: &str) -> String {
    module_base_entry("Shared", name)
}

fn module_base_entry(module: &str, name: &str) -> String {
    format!("Mods/{module}/Story/RawFiles/Goals/{name}.txt")
}

fn workspace_with(project_goals: Option<&str>) -> (WorkspaceSnapshot, PathBuf) {
    workspace_with_base_modules(&["Shared"], project_goals)
}

fn workspace_with_base_modules(
    base_modules: &[&str],
    project_goals: Option<&str>,
) -> (WorkspaceSnapshot, PathBuf) {
    let schema = Arc::new(SchemaCatalog::default());
    let mut specs: Vec<_> = base_modules
        .iter()
        .map(|name| ModuleSpec {
            name: (*name).into(),
            root: PathBuf::from(format!("/synthetic/{name}")),
            role: ModuleRole::Base,
        })
        .collect();
    specs.push(ModuleSpec {
        name: "Project".into(),
        root: PathBuf::from("/synthetic/Project"),
        role: ModuleRole::Project,
    });
    let layers = specs
        .into_iter()
        .map(|spec| {
            let is_project = spec.role == ModuleRole::Project;
            let files = match (is_project, project_goals) {
                (true, Some(text)) => {
                    let path = spec.root.join("Story/RawFiles/Goals/ProjectGoal.txt");
                    vec![
                        parse_source(
                            SourceFile {
                                path,
                                kind: SourceKind::Osiris,
                            },
                            text,
                            &schema,
                            "English",
                        )
                        .expect("project goal"),
                    ]
                }
                _ => Vec::new(),
            };
            Arc::new(ModuleIndex::new(spec, files))
        })
        .collect();
    (
        WorkspaceSnapshot::new(schema, layers, 1, 200, 200),
        PathBuf::from("/synthetic/Project/Story/RawFiles/Goals/Calls.txt"),
    )
}

fn osiris_overlay(workspace: &WorkspaceSnapshot, path: &Path, text: &str) -> OverlaySet {
    let parsed = parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::Osiris,
        },
        text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic osiris source");
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: "Project".into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
    overlays
}

const CALLER: &str = concat!(
    "Version 1\n",
    "SubGoalCombiner SGC_AND\n",
    "INITSECTION\n",
    "KBSECTION\n",
    "IF\n",
    "TextEvent(\"go\")\n",
    "THEN\n",
    "ProcHeal(_Who, _Much);\n",
    "EXITSECTION\n",
    "ENDEXITSECTION\n"
);

const GENERATED_ENGINE_CALLER: &str = concat!(
    "Version 1\n",
    "SubGoalCombiner SGC_AND\n",
    "INITSECTION\n",
    "KBSECTION\n",
    "IF\n",
    "CastedSpell(_Caster, \"Spell\", \"SpellType\", \"Fire\", 1)\n",
    "AND\n",
    "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _Amount)\n",
    "THEN\n",
    "DB_Noop(_Amount);\n",
    "EXITSECTION\n",
    "ENDEXITSECTION\n"
);

#[test]
fn generated_engine_contracts_provide_callable_hover_and_signature_help() {
    let (workspace, caller_path) = workspace_with(None);
    let overlays = osiris_overlay(&workspace, &caller_path, GENERATED_ENGINE_CALLER);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(GENERATED_ENGINE_CALLER, "GetActionResourceValuePersonal"),
            &overlays,
        )
        .expect("generated engine hover");
    assert!(
        hover.contains("**Osiris engine query** `GetActionResourceValuePersonal/4`"),
        "{hover}"
    );
    assert!(
        hover.contains(
            "```bg3_osiris\nGetActionResourceValuePersonal(\n    [in] CHARACTER _Player,\n    [in] STRING _ResourceName,\n    [in] INTEGER _ResourceLevel,\n    [out] REAL _Amount\n)\n```"
        ),
        "{hover}"
    );
    assert!(
        hover.contains("Returns a character's value for the named action resource"),
        "{hover}"
    );

    let (line, column) = call_position(GENERATED_ENGINE_CALLER, "GetActionResourceValuePersonal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("generated engine signature help");
    assert_eq!(
        signature.label,
        "GetActionResourceValuePersonal([in] CHARACTER _Player, [in] STRING _ResourceName, [in] INTEGER _ResourceLevel, [out] REAL _Amount)"
    );
    assert_eq!(signature.active_parameter, 1);
    assert!(
        signature
            .documentation
            .contains("Returns a character's value for the named action resource")
    );
}

#[test]
fn installed_engine_query_keeps_curated_description_across_language_features() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([osiris_source(
            "Shared",
            &base_entry("EngineQuery"),
            "Shared.pak",
            0,
            &query_declaration(
                "GetActionResourceValuePersonal((CHARACTER)_Player, (STRING)_ResourceName, (INTEGER)_ResourceLevel, (REAL)_Amount)",
            ),
        )])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(catalog.as_ref()));
    let overlays = osiris_overlay(&workspace, &caller_path, GENERATED_ENGINE_CALLER);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(GENERATED_ENGINE_CALLER, "GetActionResourceValuePersonal"),
            &overlays,
        )
        .expect("installed engine query hover");
    assert!(
        hover.contains("Returns a character's value for the named action resource"),
        "{hover}"
    );

    let (line, column) = call_position(GENERATED_ENGINE_CALLER, "GetActionResourceValuePersonal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("installed engine query signature help");
    assert!(
        signature
            .documentation
            .contains("Returns a character's value for the named action resource")
    );

    let completion_text = goal_text("IF\nTextEvent(\"go\")\nAND\nGetA\nTHEN\nGoalCompleted;");
    let completion_overlays = osiris_overlay(&workspace, &caller_path, &completion_text);
    let completion = workspace.completion(
        &caller_path,
        Position {
            line: 7,
            character: 4,
        },
        &completion_overlays,
        false,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "GetActionResourceValuePersonal")
        .expect("installed engine query completion");
    assert_eq!(
        item.documentation.as_deref(),
        Some(
            "Returns a character's value for the named action resource at the requested resource level."
        )
    );
}

#[test]
fn installed_has_passive_query_keeps_curated_hover_description() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([osiris_source(
            "Shared",
            &base_entry("HasPassiveQuery"),
            "Shared.pak",
            0,
            &query_declaration(
                "HasPassive((GUIDSTRING)_Entity, (STRING)_PassiveID, (INTEGER)_BoolHasPassive)",
            ),
        )])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(catalog.as_ref()));
    let caller = goal_text(
        "IF\nTextEvent(\"go\")\nAND\nHasPassive(_Entity, \"SomePassive\", _HasPassive)\nTHEN\nDB_Noop(_HasPassive);",
    );
    let overlays = osiris_overlay(&workspace, &caller_path, &caller);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(&caller, "HasPassive"),
            &overlays,
        )
        .expect("installed HasPassive query hover");
    assert!(
        hover.contains("Reports whether the specified entity has the named passive"),
        "{hover}"
    );
}

#[test]
fn legacy_engine_events_remain_completable_when_missing_from_generated_catalog() {
    let (workspace, caller_path) = workspace_with(None);
    for name in [
        "CombatTurnTimedOut",
        "QuestAcceptReverted",
        "QuestCloseReverted",
    ] {
        let text = format!(
            "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\n{name}\nTHEN\nGoalCompleted;\nEXITSECTION\nENDEXITSECTION\n"
        );
        let overlays = osiris_overlay(&workspace, &caller_path, &text);
        let completion = workspace.completion(
            &caller_path,
            Position {
                line: 5,
                character: u32::try_from(name.len()).expect("name length fits"),
            },
            &overlays,
            false,
        );
        let item = completion
            .items
            .iter()
            .find(|item| item.label == name)
            .unwrap_or_else(|| panic!("missing completion for {name}"));
        assert_eq!(item.kind, CompletionKind::Function);
        assert!(
            item.detail
                .as_deref()
                .is_some_and(|detail| detail.contains("legacy event catalog")),
            "{name} detail: {:?}",
            item.detail
        );
    }

    let signature_text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "CombatTurnTimedOut(_Combat)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n"
    );
    let overlays = osiris_overlay(&workspace, &caller_path, signature_text);
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line: 5,
                character: u32::try_from("CombatTurnTimedOut(".len()).expect("position fits"),
            },
            &overlays,
        )
        .expect("legacy event signature help");
    assert_eq!(signature.label, "CombatTurnTimedOut(GUIDSTRING)");
}

#[test]
fn signature_help_survives_osiris_type_cast_before_later_arguments() {
    let (workspace, caller_path) = workspace_with(None);
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell((CHARACTER)_Caster, \"Target_MainHandAttack\", -, -, _StoryActionID)\n",
        "THEN\n",
        "DB_Noop(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n"
    );
    let overlays = osiris_overlay(&workspace, &caller_path, text);
    let line = text.lines().nth(5).expect("UsingSpell line");

    for (suffix, expected_parameter) in [
        (", ", 1),
        (", \"Target_MainHandAttack\", ", 2),
        (", -, ", 3),
        (", -, _StoryActionID", 4),
    ] {
        let prefix = if expected_parameter == 1 {
            "UsingSpell((CHARACTER)_Caster"
        } else {
            "UsingSpell((CHARACTER)_Caster, \"Target_MainHandAttack\", -,"
        };
        let character = if expected_parameter == 1 {
            line.find(prefix).expect("casted first argument") + prefix.len() + suffix.len()
        } else {
            line.find(suffix).expect("later argument") + suffix.len()
        };
        let signature = workspace
            .signature_help(
                &caller_path,
                Position {
                    line: 5,
                    character: u32::try_from(character).expect("position fits"),
                },
                &overlays,
            )
            .expect("UsingSpell signature help");
        assert_eq!(signature.active_parameter, expected_parameter);
        assert!(signature.label.starts_with("UsingSpell("));
    }
}

const GENERATED_SHORT_CALLER: &str = concat!(
    "Version 1\n",
    "SubGoalCombiner SGC_AND\n",
    "INITSECTION\n",
    "KBSECTION\n",
    "IF\n",
    "Exists(_Object, _Bool)\n",
    "THEN\n",
    "DB_Noop(_Bool);\n",
    "EXITSECTION\n",
    "ENDEXITSECTION\n"
);

#[test]
fn generated_engine_callable_hover_uses_compact_code_for_short_signatures() {
    let (workspace, caller_path) = workspace_with(None);
    let overlays = osiris_overlay(&workspace, &caller_path, GENERATED_SHORT_CALLER);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(GENERATED_SHORT_CALLER, "Exists"),
            &overlays,
        )
        .expect("generated short engine hover");
    assert!(
        hover.contains("```bg3_osiris\nExists([in] GUIDSTRING _Object, [out] INTEGER _Bool)\n```")
    );
    assert!(!hover.contains("Signature:"), "{hover}");
    assert!(
        hover.contains("**Catalog:** BG3 build `4.1.1.7398727`"),
        "{hover}"
    );
}

#[test]
fn generated_engine_event_contracts_provide_typed_hover() {
    let (workspace, caller_path) = workspace_with(None);
    let overlays = osiris_overlay(&workspace, &caller_path, GENERATED_ENGINE_CALLER);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(GENERATED_ENGINE_CALLER, "CastedSpell"),
            &overlays,
        )
        .expect("generated engine event hover");
    assert!(
        hover.contains("**Osiris engine event** `CastedSpell/5`"),
        "{hover}"
    );
    assert!(hover.contains("[in] GUIDSTRING _Caster"), "{hover}");
    assert!(hover.contains("[in] INTEGER _StoryActionID"), "{hover}");
    assert!(
        hover.contains("Event raised after a spell is cast"),
        "{hover}"
    );
}

fn installed_catalog() -> Arc<PackagedThothCatalog> {
    Arc::new(
        PackagedThothCatalog::from_sources([osiris_source(
            "Shared",
            &base_entry("Base"),
            "Shared.pak",
            0,
            &proc_declaration("ProcHeal((CHARACTER)_Target, (INTEGER)_Amount)"),
        )])
        .expect("catalog"),
    )
}

#[test]
fn hover_prefers_installed_declared_aliases_without_paths() {
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(installed_catalog().as_ref()));
    let overlays = osiris_overlay(&workspace, &caller_path, CALLER);

    let hover = workspace
        .hover(&caller_path, source_position(CALLER, "ProcHeal"), &overlays)
        .expect("installed procedure hover");
    assert!(
        hover.contains("**Installed Osiris procedure** `ProcHeal/2`"),
        "{hover}"
    );
    assert!(
        hover.contains("Signature: `ProcHeal(CHARACTER _Target, INTEGER _Amount)`"),
        "{hover}"
    );
    assert!(hover.contains("`Shared`"), "{hover}");
    assert!(!hover.contains(".pak"), "{hover}");
    assert!(!hover.contains("parameter types are unknown"), "{hover}");
}

#[test]
fn signature_help_inside_user_rules_uses_installed_aliases() {
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(installed_catalog().as_ref()));
    let overlays = osiris_overlay(&workspace, &caller_path, CALLER);

    let (line, column) = call_position(CALLER, "ProcHeal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("installed signature help");
    assert_eq!(
        signature.label,
        "ProcHeal(CHARACTER _Target, INTEGER _Amount)"
    );
    assert_eq!(signature.active_parameter, 1);
}

#[test]
fn loose_project_declaration_overrides_installed_evidence() {
    let project_goal = proc_declaration("ProcHeal((GUIDSTRING)_Other, (INTEGER)_N)");
    let (workspace, caller_path) = workspace_with(Some(&project_goal));
    let workspace = workspace.with_packaged_osiris(index(installed_catalog().as_ref()));
    let overlays = osiris_overlay(&workspace, &caller_path, CALLER);

    let hover = workspace
        .hover(&caller_path, source_position(CALLER, "ProcHeal"), &overlays)
        .expect("loose procedure hover");
    assert!(!hover.contains("Installed Osiris"), "{hover}");
    assert!(hover.contains("**Osiris procedure** `ProcHeal`"), "{hover}");

    let (line, column) = call_position(CALLER, "ProcHeal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("loose signature help");
    assert_eq!(signature.label, "ProcHeal(GUIDSTRING _Other, INTEGER _N)");
}

#[test]
fn ambiguous_installed_declarations_stay_untyped() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            osiris_source(
                "Shared",
                &base_entry("Alpha"),
                "Shared.pak",
                0,
                &proc_declaration("ProcSplit((CHARACTER)_X, (INTEGER)_Y)"),
            ),
            osiris_source(
                "Shared",
                &base_entry("Beta"),
                "Alt.pak",
                0,
                &proc_declaration("ProcSplit((GUIDSTRING)_X, (INTEGER)_Y)"),
            ),
        ])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(catalog.as_ref()));
    let conflicting = CALLER.replace("ProcHeal", "ProcSplit");
    let overlays = osiris_overlay(&workspace, &caller_path, &conflicting);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(&conflicting, "ProcSplit"),
            &overlays,
        )
        .expect("ambiguous hover");
    assert!(
        hover.contains("**Installed Osiris callable** `ProcSplit/2`"),
        "{hover}"
    );
    assert!(hover.contains("disagree"), "{hover}");
    assert!(!hover.contains("Signature:"), "{hover}");

    let (line, column) = call_position(&conflicting, "ProcSplit");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("ambiguous signature help");
    assert_eq!(signature.label, "ProcSplit(value1, value2)");
}

#[test]
fn mixed_role_installed_declarations_are_not_placed_by_guessing() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            osiris_source(
                "Shared",
                &base_entry("Procedure"),
                "Shared.pak",
                0,
                &proc_declaration("RoleMixed((INTEGER)_Value)"),
            ),
            osiris_source(
                "Shared",
                &base_entry("Query"),
                "Query.pak",
                0,
                &query_declaration("RoleMixed((INTEGER)_Value)"),
            ),
        ])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with(None);
    let packaged = index(catalog.as_ref());
    assert!(matches!(
        packaged.resolve("Shared", "RoleMixed", 1),
        PackagedOsirisResolution::Ambiguous(_)
    ));
    let workspace = workspace.with_packaged_osiris(packaged);

    let head = goal_text("IF\nRoleMixed\nTHEN\nGoalCompleted;");
    let head_overlays = osiris_overlay(&workspace, &caller_path, &head);
    let head_completion = workspace.completion(
        &caller_path,
        Position {
            line: 5,
            character: 9,
        },
        &head_overlays,
        false,
    );
    assert!(
        !head_completion
            .items
            .iter()
            .any(|item| item.label == "RoleMixed")
    );

    let action = goal_text("IF\nDied(_Who)\nTHEN\nRoleMixed\n");
    let action_overlays = osiris_overlay(&workspace, &caller_path, &action);
    let action_completion = workspace.completion(
        &caller_path,
        Position {
            line: 7,
            character: 9,
        },
        &action_overlays,
        false,
    );
    assert!(
        !action_completion
            .items
            .iter()
            .any(|item| item.label == "RoleMixed")
    );
}

#[test]
fn higher_precedence_invalid_installed_role_masks_lower_and_generated_completion() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            osiris_source(
                "A",
                &module_base_entry("A", "Lower"),
                "A.pak",
                0,
                &proc_declaration("Died((GUIDSTRING)_Lower)"),
            ),
            osiris_source(
                "B",
                &module_base_entry("B", "Higher"),
                "B.pak",
                0,
                &query_declaration("Died((GUIDSTRING)_Higher)"),
            ),
        ])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with_base_modules(&["A", "B"], None);
    let workspace = workspace.with_packaged_osiris(index(catalog.as_ref()));
    let caller = goal_text("IF\nDied(_Who)\nTHEN\nD");
    let overlays = osiris_overlay(&workspace, &caller_path, &caller);
    let line = caller
        .lines()
        .position(|line| line == "D")
        .expect("action prefix line");
    let completion = workspace.completion(
        &caller_path,
        Position {
            line: u32::try_from(line).expect("line fits"),
            character: 1,
        },
        &overlays,
        false,
    );
    assert!(
        !completion.items.iter().any(|item| item.label == "Died"),
        "shadowed labels: {:?}",
        completion
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn higher_precedence_base_module_masks_lower_ambiguity() {
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            osiris_source(
                "A",
                &module_base_entry("A", "Alpha"),
                "A.pak",
                0,
                &proc_declaration("ProcRank((CHARACTER)_Low, (INTEGER)_Amount)"),
            ),
            osiris_source(
                "A",
                &module_base_entry("A", "Beta"),
                "A-alt.pak",
                0,
                &proc_declaration("ProcRank((GUIDSTRING)_Other, (INTEGER)_Amount)"),
            ),
            osiris_source(
                "B",
                &module_base_entry("B", "Base"),
                "B.pak",
                0,
                &proc_declaration("ProcRank((STRING)_High, (INTEGER)_Count)"),
            ),
        ])
        .expect("catalog"),
    );
    let (workspace, caller_path) = workspace_with_base_modules(&["A", "B"], None);
    let workspace = workspace.with_packaged_osiris(index(catalog.as_ref()));
    let caller = CALLER.replace("ProcHeal", "ProcRank");
    let overlays = osiris_overlay(&workspace, &caller_path, &caller);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(&caller, "ProcRank"),
            &overlays,
        )
        .expect("higher-precedence installed procedure hover");
    assert!(hover.contains("Module: `B`"), "{hover}");
    assert!(
        hover.contains("Signature: `ProcRank(STRING _High, INTEGER _Count)`"),
        "{hover}"
    );
    assert!(!hover.contains("disagree"), "{hover}");

    let (line, column) = call_position(&caller, "ProcRank");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &overlays,
        )
        .expect("higher-precedence installed signature help");
    assert_eq!(signature.label, "ProcRank(STRING _High, INTEGER _Count)");
}

#[test]
fn overlays_update_loose_declarations_live() {
    let (workspace, caller_path) = workspace_with(None);
    let workspace = workspace.with_packaged_osiris(index(installed_catalog().as_ref()));

    // Without any loose declaration the installed aliases win.
    let before = osiris_overlay(&workspace, &caller_path, CALLER);
    let (line, column) = call_position(CALLER, "ProcHeal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &before,
        )
        .expect("installed signature");
    assert!(signature.label.contains("CHARACTER _Target"));

    // Declaring the same procedure loose switches evidence to the overlay.
    let declared = CALLER.replacen(
        "KBSECTION\n",
        "KBSECTION\nPROC\nProcHeal((STRING)_Name, (INTEGER)_Qty)\nTHEN\nDB_Noop(1);\n",
        1,
    );
    let after = osiris_overlay(&workspace, &caller_path, &declared);
    let (line, column) = call_position(&declared, "ProcHeal");
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line,
                character: column,
            },
            &after,
        )
        .expect("overlay signature");
    assert_eq!(signature.label, "ProcHeal(STRING _Name, INTEGER _Qty)");

    // Completion offers the installed callable with provenance detail.
    let partial = CALLER.replacen("ProcHeal(_Who, _Much);", "ProcH", 1);
    let completion_overlays = osiris_overlay(&workspace, &caller_path, &partial);
    let completion = workspace.completion(
        &caller_path,
        Position {
            line: 7,
            character: 4,
        },
        &completion_overlays,
        true,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "ProcHeal")
        .expect("installed completion");
    assert!(
        item.detail
            .as_deref()
            .is_some_and(|detail| detail.contains("installed Shared"))
    );
    assert_eq!(item.kind, CompletionKind::Function);

    let lowercase = CALLER.replacen("ProcHeal(_Who, _Much);", "proch", 1);
    let lowercase_overlays = osiris_overlay(&workspace, &caller_path, &lowercase);
    let lowercase_completion = workspace.completion(
        &caller_path,
        Position {
            line: 7,
            character: 5,
        },
        &lowercase_overlays,
        false,
    );
    assert!(
        lowercase_completion
            .items
            .iter()
            .any(|item| item.label == "ProcHeal"),
        "packaged completion labels: {:?}",
        lowercase_completion
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>()
    );
}

fn source_position(text: &str, needle: &str) -> Position {
    text.lines()
        .enumerate()
        .find_map(|(line, source)| {
            source.find(needle).map(|character| Position {
                line: u32::try_from(line).unwrap(),
                character: u32::try_from(character).unwrap(),
            })
        })
        .unwrap()
}

fn call_position(text: &str, callable: &str) -> (u32, u32) {
    let call_prefix = format!("{callable}(_");
    text.lines()
        .enumerate()
        .find_map(|(line, source)| {
            if !source.starts_with(&call_prefix) {
                return None;
            }
            source.find(',').map(|comma| {
                (
                    u32::try_from(line).unwrap(),
                    u32::try_from(comma + 1).unwrap(),
                )
            })
        })
        .expect("an in-progress procedure call")
}
