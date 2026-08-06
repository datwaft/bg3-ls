use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{DiagnosticSeverity, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, Position, SchemaCatalog, SourceFile, SourceKind,
    SymbolTarget, discover_module, parse_source,
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

fn fixture_workspace(max_completion_items: usize) -> (WorkspaceSnapshot, PathBuf) {
    let root = fixtures();
    let game = root.join("game");
    let schema = Arc::new(load_schema(&root));
    let specs = [
        ModuleSpec {
            name: "Shared".into(),
            root: game.join("Editor/Mods/Shared"),
            role: ModuleRole::Base,
        },
        ModuleSpec {
            name: "Item and Spell Bug Fixes".into(),
            root: root.join("dependency"),
            role: ModuleRole::Dependency,
        },
        ModuleSpec {
            name: "MyMod".into(),
            root: root.join("project"),
            role: ModuleRole::Project,
        },
    ];
    let layers = specs
        .into_iter()
        .enumerate()
        .map(|(index, spec)| {
            let files = discover_module(&spec, &game, "English", index == 0).unwrap();
            let parsed = files
                .into_iter()
                .map(|source| {
                    let text = fs::read_to_string(&source.path).unwrap();
                    parse_source(source, &text, &schema, "English").unwrap()
                })
                .collect();
            Arc::new(ModuleIndex::new(spec, parsed))
        })
        .collect();
    (
        WorkspaceSnapshot::new(schema, layers, 1, 200, max_completion_items),
        root.join("project/Public/MyMod/Stats/Generated/Data/Passive.txt"),
    )
}

fn overlay(workspace: &WorkspaceSnapshot, path: &Path, text: &str) -> OverlaySet {
    let mut overlays = OverlaySet::default();
    let parsed = parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::PlainStats,
        },
        text,
        &workspace.schema,
        "English",
    )
    .unwrap();
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: "MyMod".into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
    overlays
}

#[test]
fn resolves_override_layers_from_highest_to_lowest() {
    let (workspace, _) = fixture_workspace(200);
    let definitions = workspace.resolve(
        &SymbolTarget::Named {
            kind: Some("PassiveData".into()),
            name: "CHAINED".into(),
        },
        &OverlaySet::default(),
    );

    assert_eq!(
        definitions
            .iter()
            .map(|definition| definition.module.as_str())
            .collect::<Vec<_>>(),
        ["MyMod", "Item and Spell Bug Fixes", "Shared"]
    );
}

#[test]
fn completes_fields_and_values_in_incomplete_syntax() {
    let (workspace, path) = fixture_workspace(200);
    let field_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"E";
    let field_overlays = overlay(&workspace, &path, field_text);
    let fields = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 7,
        },
        &field_overlays,
        false,
    );
    assert_eq!(
        fields
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["Enabled", "ExportedAmount"]
    );

    let enum_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Y";
    let enum_overlays = overlay(&workspace, &path, enum_text);
    let values = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 17,
        },
        &enum_overlays,
        false,
    );
    assert_eq!(values.items[0].label, "Yes");
}

#[test]
fn completes_typed_symbols_localization_and_functions() {
    let (workspace, path) = fixture_workspace(200);
    let spell_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"UnlockSpell(Target_T";
    let spell_overlays = overlay(&workspace, &path, spell_text);
    let spells = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 35,
        },
        &spell_overlays,
        true,
    );
    assert!(spells.items.iter().any(|item| item.label == "Target_Test"));

    let localization_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"DisplayName\" \"h000";
    let localization_overlays = overlay(&workspace, &path, localization_text);
    let localization = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 25,
        },
        &localization_overlays,
        false,
    );
    assert_eq!(
        localization.items[0].new_text,
        "h000000000000000000000000000000000001;2"
    );

    let function_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Apply";
    let function_overlays = overlay(&workspace, &path, function_text);
    let functions = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 21,
        },
        &function_overlays,
        true,
    );
    let function = functions
        .items
        .iter()
        .find(|item| item.label == "ApplyStatus")
        .unwrap();
    assert!(function.snippet);
    assert_eq!(function.new_text, "ApplyStatus(${1:status})");

    let resource_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"ResourceId\" \"Act";
    let resource_overlays = overlay(&workspace, &path, resource_text);
    let resources = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 22,
        },
        &resource_overlays,
        false,
    );
    let resource = resources
        .items
        .iter()
        .find(|item| item.label == "ActionPoint")
        .unwrap();
    assert_eq!(resource.new_text, "dddddddd-dddd-dddd-dddd-dddddddddddd");
}

#[test]
fn completion_collapses_overrides_and_preserves_same_rank_ambiguity() {
    let (workspace, path) = fixture_workspace(200);
    let chain_text = "new entry \"CHAINED\"\ntype \"PassiveData\"\n\nnew entry \"TEST\"\ntype \"PassiveData\"\nusing \"CHA";
    let chain_overlays = overlay(&workspace, &path, chain_text);
    let chain = workspace.completion(
        &path,
        Position {
            line: 5,
            character: 10,
        },
        &chain_overlays,
        false,
    );
    assert_eq!(
        chain
            .items
            .iter()
            .filter(|item| item.label == "CHAINED")
            .count(),
        1
    );
    assert_eq!(chain.items[0].detail.as_deref(), Some("MyMod"));

    let duplicate_text = r#"new entry "DUPLICATE"
type "PassiveData"

new entry "DUPLICATE"
type "PassiveData"

new entry "TEST"
type "PassiveData"
using "DUP"
"#;
    let duplicate_overlays = overlay(&workspace, &path, duplicate_text);
    let duplicates = workspace.completion(
        &path,
        Position {
            line: 8,
            character: 10,
        },
        &duplicate_overlays,
        false,
    );
    assert_eq!(duplicates.items.len(), 2);
    assert!(duplicates.items.iter().all(|item| {
        item.detail
            .as_deref()
            .is_some_and(|detail| detail.contains("same-rank ambiguity"))
    }));
}

#[test]
fn reports_curated_signatures_and_caps_completion() {
    let (workspace, path) = fixture_workspace(1);
    let text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ApplyStatus(TEST_STATUS,";
    let overlays = overlay(&workspace, &path, text);
    let help = workspace
        .signature_help(
            &path,
            Position {
                line: 2,
                character: 39,
            },
            &overlays,
        )
        .unwrap();
    assert_eq!(help.active_parameter, 1);
    assert!(help.label.starts_with("ApplyStatus(status"));

    let type_text = "new entry \"TEST\"\ntype \"";
    let type_overlays = overlay(&workspace, &path, type_text);
    let types = workspace.completion(
        &path,
        Position {
            line: 1,
            character: 6,
        },
        &type_overlays,
        false,
    );
    assert!(types.incomplete);
    assert_eq!(types.items.len(), 1);
}

#[test]
fn completes_and_describes_explicit_target_overloads() {
    let (workspace, path) = fixture_workspace(200);
    let text = r#"new entry "SHIELD"
type "StatusData"

new entry "TEST"
type "PassiveData"
data "Boosts" "ApplyStatus(OBSERVER_OBSERVER,SHI"#;
    let overlays = overlay(&workspace, &path, text);
    let line = text.lines().nth(5).unwrap();
    let position = Position {
        line: 5,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &overlays, false);
    assert!(completion.items.iter().any(|item| item.label == "SHIELD"));

    let help = workspace
        .signature_help(&path, position, &overlays)
        .unwrap();
    assert_eq!(help.active_parameter, 1);
    assert!(help.label.starts_with("ApplyStatus(target, status"));

    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.code != "unresolved-reference"
            || !diagnostic.message.contains("OBSERVER_OBSERVER")
    }));
}

#[test]
fn reports_verified_schema_and_reference_problems() {
    let (workspace, path) = fixture_workspace(200);
    let text = r#"new entry "BROKEN"
type "PassiveData"
using "MISSING_PARENT"
data "Enabled" "Maybe"
data "Flag" "Sometimes"
data "Amount" "3"
data "UUID" "not-a-uuid"
data "DisplayName" "bad-handle"
data "Bogus" "value"
data "Boosts" "ApplyStatus(MISSING_STATUS,100,1);UnknownName"
data "Boosts" "Ability(Strength,1)"

new entry "UNKNOWN"
type "NotARealType"
"#;
    let overlays = overlay(&workspace, &path, text);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();

    for expected in [
        "missing-parent",
        "invalid-enum",
        "invalid-boolean",
        "number-out-of-range",
        "invalid-uuid",
        "invalid-translated-string",
        "unknown-field",
        "duplicate-field",
        "unresolved-reference",
        "unknown-entry-type",
    ] {
        assert!(codes.contains(&expected), "missing {expected}: {codes:?}");
    }
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "unresolved-reference")
            .count(),
        1,
        "generic expression identifiers must not produce diagnostics"
    );
}

#[test]
fn accepts_empty_values_that_clear_inherited_fields() {
    let (workspace, path) = fixture_workspace(200);
    let text = r#"new entry "CLEAR_VALUES"
type "PassiveData"
data "Enabled" ""
data "Flag" ""
data "Amount" ""
data "UUID" ""
data "DisplayName" ""
"#;
    let overlays = overlay(&workspace, &path, text);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));

    assert!(
        diagnostics.iter().all(|diagnostic| !matches!(
            diagnostic.code.as_str(),
            "invalid-enum"
                | "invalid-boolean"
                | "invalid-number"
                | "invalid-uuid"
                | "invalid-translated-string"
        )),
        "empty values must clear inherited fields: {diagnostics:?}"
    );
}

#[test]
fn validates_each_enumeration_list_member() {
    let (workspace, path) = fixture_workspace(200);
    let valid_text = r#"new entry "VALID_LISTS"
type "PassiveData"
data "Modes" "Yes;No"
data "ImplicitModes" "No; Yes"
"#;
    let valid_overlays = overlay(&workspace, &path, valid_text);
    let valid_diagnostics =
        workspace.diagnostics(&path, &valid_overlays, Some(DiagnosticSeverity::Warning));
    assert!(
        valid_diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "invalid-enum"),
        "valid list members must pass enum validation: {valid_diagnostics:?}"
    );

    let invalid_text = r#"new entry "INVALID_LIST"
type "PassiveData"
data "Modes" "Yes;Maybe;No"
"#;
    let invalid_overlays = overlay(&workspace, &path, invalid_text);
    let invalid_diagnostics =
        workspace.diagnostics(&path, &invalid_overlays, Some(DiagnosticSeverity::Warning));
    assert!(invalid_diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "invalid-enum" && diagnostic.message.contains("`Maybe`")
    }));
}

#[test]
fn resolves_legacy_fields_by_schema_export_name() {
    let (workspace, path) = fixture_workspace(200);
    let text = r#"new entry "EXPORTED_FIELD"
type "PassiveData"
data "ExportedAmount" "1"
"#;
    let overlays = overlay(&workspace, &path, text);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "unknown-field"),
        "an export name must identify its schema field: {diagnostics:?}"
    );

    let completion_text = "new entry \"EXPORTED_FIELD\"\ntype \"PassiveData\"\ndata \"Export";
    let completion_overlays = overlay(&workspace, &path, completion_text);
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 12,
        },
        &completion_overlays,
        false,
    );
    assert_eq!(completion.items[0].label, "ExportedAmount");
}

#[test]
fn does_not_diagnose_status_groups_as_missing_statuses() {
    let (workspace, path) = fixture_workspace(200);
    let text = r#"new entry "STATUS_GROUPS"
type "PassiveData"
data "Boosts" "HasStatus('SG_Charmed');ApplyStatus(MISSING_STATUS,100,1)"
"#;
    let overlays = overlay(&workspace, &path, text);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));

    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.code != "unresolved-reference" || !diagnostic.message.contains("SG_Charmed")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "unresolved-reference" && diagnostic.message.contains("MISSING_STATUS")
    }));
}

#[test]
fn syntax_diagnostics_can_disable_unresolved_references() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"BROKEN\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ApplyStatus(MISSING";
    let overlays = overlay(&workspace, &path, text);
    let diagnostics = workspace.diagnostics(&path, &overlays, None);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax-error")
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "unresolved-reference")
    );
}

#[test]
fn hover_describes_enum_values_and_curated_functions() {
    let (workspace, path) = fixture_workspace(200);
    let enum_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"";
    let enum_overlays = overlay(&workspace, &path, enum_text);
    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: 17,
            },
            &enum_overlays,
        )
        .unwrap();
    assert!(hover.contains("Enum value"));
    assert!(hover.contains("YesNo"));

    let function_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ApplyStatus(TEST_STATUS,100,1)\"";
    let function_overlays = overlay(&workspace, &path, function_text);
    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: 20,
            },
            &function_overlays,
        )
        .unwrap();
    assert!(hover.contains("Applies a status"));
}
