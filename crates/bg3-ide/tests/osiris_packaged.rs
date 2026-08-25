use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{CompletionKind, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, OSIRIS_FACTS_EXTRACTOR_VERSION, PackagedOsirisIndex,
    PackagedThothCatalog, PackagedThothSource, Position, SchemaCatalog, SourceFile, SourceKind,
    parse_osiris_goal_source, parse_packaged_thoth_facts, parse_source,
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
    format!("Mods/Shared/Story/RawFiles/Goals/{name}.txt")
}

fn workspace_with(project_goals: Option<&str>) -> (WorkspaceSnapshot, PathBuf) {
    let schema = Arc::new(SchemaCatalog::default());
    let specs = [
        ModuleSpec {
            name: "Shared".into(),
            root: PathBuf::from("/synthetic/Shared"),
            role: ModuleRole::Base,
        },
        ModuleSpec {
            name: "Project".into(),
            root: PathBuf::from("/synthetic/Project"),
            role: ModuleRole::Project,
        },
    ];
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
