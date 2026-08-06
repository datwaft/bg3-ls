use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{DiagnosticSeverity, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    LocalizationCatalog, ModuleIndex, ModuleRole, ModuleSpec, Position, SchemaCatalog, SourceFile,
    SourceKind, SymbolTarget, discover_module, parse_source,
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
fn treats_legacy_type_discriminators_as_schema_metadata() {
    let (workspace, path) = fixture_workspace(200);
    let status_path = path.with_file_name("Status_BOOST.txt");
    let status_text = r#"new entry "STATUS"
type "StatusData"
data "StatusType" "BOOST"
data "Boosts" ""
"#;
    let status_overlays = overlay(&workspace, &status_path, status_text);
    let status_diagnostics = workspace.diagnostics(
        &status_path,
        &status_overlays,
        Some(DiagnosticSeverity::Warning),
    );
    assert!(status_diagnostics.iter().all(|diagnostic| {
        diagnostic.code != "unknown-field" && diagnostic.code != "invalid-schema-discriminator"
    }));

    let spell_path = path.with_file_name("Spell_Target.txt");
    let spell_text = r#"new entry "SPELL"
type "SpellData"
data "SpellType" "Target"
data "SpellFlags" ""
"#;
    let spell_overlays = overlay(&workspace, &spell_path, spell_text);
    let spell_diagnostics = workspace.diagnostics(
        &spell_path,
        &spell_overlays,
        Some(DiagnosticSeverity::Warning),
    );
    assert!(spell_diagnostics.iter().all(|diagnostic| {
        diagnostic.code != "unknown-field" && diagnostic.code != "invalid-schema-discriminator"
    }));
}

#[test]
fn diagnoses_conflicting_schema_discriminators_without_field_cascades() {
    let (workspace, path) = fixture_workspace(200);
    let status_path = path.with_file_name("Status_BOOST.txt");
    let text = r#"new entry "CONFLICT"
type "StatusData"
data "StatusType" "HEAL"
data "Boosts" ""
data "Bogus" "value"
"#;
    let overlays = overlay(&workspace, &status_path, text);
    let diagnostics =
        workspace.diagnostics(&status_path, &overlays, Some(DiagnosticSeverity::Warning));

    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "invalid-schema-discriminator")
            .count(),
        1
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "unknown-field")
            .count(),
        1,
        "only the normal unknown field must produce unknown-field"
    );
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
fn diagnoses_localization_only_when_visible_sources_are_complete() {
    let handle = "hffffffffffffffffffffffffffffffffffff";
    let text = format!(
        "new entry \"LOCALIZATION\"\ntype \"PassiveData\"\ndata \"DisplayName\" \"{handle}\"\n"
    );

    let (complete, path) = fixture_workspace(200);
    let complete_overlays = overlay(&complete, &path, &text);
    let complete_diagnostics =
        complete.diagnostics(&path, &complete_overlays, Some(DiagnosticSeverity::Warning));
    assert!(complete_diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "unresolved-reference" && diagnostic.message.contains(handle)
    }));

    let (incomplete, path) = fixture_workspace(200);
    let incomplete = incomplete.with_incomplete_kinds(["Localization"]);
    let incomplete_overlays = overlay(&incomplete, &path, &text);
    let incomplete_diagnostics = incomplete.diagnostics(
        &path,
        &incomplete_overlays,
        Some(DiagnosticSeverity::Warning),
    );
    assert!(incomplete_diagnostics.iter().all(|diagnostic| {
        diagnostic.code != "unresolved-reference" || !diagnostic.message.contains(handle)
    }));

    let known = SymbolTarget::Named {
        kind: Some("Localization".into()),
        name: "h000000000000000000000000000000000001".into(),
    };
    assert!(
        !incomplete
            .resolve(&known, &OverlaySet::default())
            .is_empty()
    );
}

#[test]
fn diagnoses_named_symbols_only_when_visible_layers_are_complete() {
    let text = r#"new entry "PACKED_REFERENCES"
type "PassiveData"
data "Boosts" "ApplyStatus(BASE_STATUS,100,1);UnlockSpell(Target_BaseSpell)"
"#;

    let (complete, path) = fixture_workspace(200);
    let complete_overlays = overlay(&complete, &path, text);
    let complete_diagnostics =
        complete.diagnostics(&path, &complete_overlays, Some(DiagnosticSeverity::Warning));
    for name in ["BASE_STATUS", "Target_BaseSpell"] {
        assert!(complete_diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unresolved-reference" && diagnostic.message.contains(name)
        }));
    }

    let (incomplete, path) = fixture_workspace(200);
    let incomplete = incomplete.with_incomplete_kinds(["SpellData", "StatusData"]);
    let incomplete_overlays = overlay(&incomplete, &path, text);
    let incomplete_diagnostics = incomplete.diagnostics(
        &path,
        &incomplete_overlays,
        Some(DiagnosticSeverity::Warning),
    );
    for name in ["BASE_STATUS", "Target_BaseSpell"] {
        assert!(incomplete_diagnostics.iter().all(|diagnostic| {
            diagnostic.code != "unresolved-reference" || !diagnostic.message.contains(name)
        }));
    }

    let known = SymbolTarget::Named {
        kind: Some("SpellData".into()),
        name: "Target_Test".into(),
    };
    assert!(
        !incomplete
            .resolve(&known, &OverlaySet::default())
            .is_empty()
    );
}

#[test]
fn diagnoses_parents_only_when_layers_are_complete_and_enabled() {
    let text = "new entry \"CHILD\"\ntype \"PassiveData\"\nusing \"MISSING_PARENT\"\n";

    let (complete, path) = fixture_workspace(200);
    let complete_overlays = overlay(&complete, &path, text);
    let complete_diagnostics =
        complete.diagnostics(&path, &complete_overlays, Some(DiagnosticSeverity::Hint));
    assert!(complete_diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "missing-parent" && diagnostic.severity == DiagnosticSeverity::Hint
    }));

    let disabled = complete.diagnostics(&path, &complete_overlays, None);
    assert!(
        disabled
            .iter()
            .all(|diagnostic| diagnostic.code != "missing-parent")
    );

    let (incomplete, path) = fixture_workspace(200);
    let incomplete = incomplete.with_incomplete_kinds(["PassiveData"]);
    let incomplete_overlays = overlay(&incomplete, &path, text);
    let incomplete_diagnostics = incomplete.diagnostics(
        &path,
        &incomplete_overlays,
        Some(DiagnosticSeverity::Warning),
    );
    assert!(
        incomplete_diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "missing-parent")
    );
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

#[test]
fn hover_previews_inherited_and_localized_game_text() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [
            (
                "h000000000000000000000000000000000001".into(),
                2,
                "Packed title".into(),
            ),
            (
                "h000000000000000000000000000000000002".into(),
                1,
                "Synthetic <LSTag Type=\"Status\">description</LSTag><br>Second line".into(),
            ),
        ],
    )
    .unwrap();
    let workspace = workspace.with_base_localization(Arc::new(catalog));
    let status_path = path.with_file_name("Status_BOOST.txt");
    let text = r#"new entry "ENLARGE"
type "StatusData"
data "StatusType" "BOOST"
using "ENLARGE"
data "Boosts" "ObjectSize(+2)"
"#;
    let overlays = overlay(&workspace, &status_path, text);
    let hover = workspace
        .hover(
            &status_path,
            Position {
                line: 0,
                character: 13,
            },
            &overlays,
        )
        .unwrap();

    assert!(hover.contains("**Boosts:** `ObjectSize(+2)`"));
    assert!(hover.contains("\n\n---\n\n### Game text preview"));
    assert!(hover.contains("**Test action & label**"), "{hover}");
    assert!(hover.contains("Synthetic description"));
    assert!(!hover.contains("Second line"));
    assert!(hover.contains("Description parameters: `Distance(3)`"));
    assert!(hover.contains("Game logic and UI formatting are not evaluated"));
    assert!(hover.contains("**Override chain**"));
}

#[test]
fn hover_uses_packed_base_localization_as_fallback() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [
            (
                "h000000000000000000000000000000000003".into(),
                1,
                "Packed title".into(),
            ),
            (
                "h000000000000000000000000000000000004".into(),
                1,
                "Packed <LSTag Type=\"Status\">description</LSTag><br>Second line".into(),
            ),
        ],
    )
    .unwrap();
    let workspace = workspace.with_base_localization(Arc::new(catalog));
    let status_path = path.with_file_name("Status_BOOST.txt");
    let text = r#"new entry "PACKED_TOOLTIP"
type "StatusData"
data "DisplayName" "h000000000000000000000000000000000003;1"
data "Description" "h000000000000000000000000000000000004;1"
"#;
    let overlays = overlay(&workspace, &status_path, text);
    let hover = workspace
        .hover(
            &status_path,
            Position {
                line: 0,
                character: 13,
            },
            &overlays,
        )
        .unwrap();

    assert!(hover.contains("**Packed title**"));
    assert!(hover.contains("Packed description\nSecond line"));
}

#[test]
fn empty_override_fields_clear_the_localized_preview() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [(
            "h000000000000000000000000000000000002".into(),
            1,
            "Synthetic description".into(),
        )],
    )
    .unwrap();
    let workspace = workspace.with_base_localization(Arc::new(catalog));
    let status_path = path.with_file_name("Status_BOOST.txt");
    let text = r#"new entry "ENLARGE"
type "StatusData"
data "StatusType" "BOOST"
using "ENLARGE"
data "DisplayName" ""
data "Description" ""
data "DescriptionParams" ""
"#;
    let overlays = overlay(&workspace, &status_path, text);
    let hover = workspace
        .hover(
            &status_path,
            Position {
                line: 0,
                character: 13,
            },
            &overlays,
        )
        .unwrap();

    assert!(!hover.contains("In-game text preview"));
    assert!(!hover.contains("Synthetic description"));
}
