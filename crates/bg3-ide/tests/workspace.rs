use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{DiagnosticSeverity, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    LocalizationCatalog, ModuleIndex, ModuleRole, ModuleSpec, PackagedStatsCatalog,
    PackagedStatsSource, PackagedThothCatalog, PackagedThothSource, Position, SchemaCatalog,
    SourceFile, SourceKind, SymbolTarget, discover_module, parse_packaged_thoth_facts,
    parse_source, parse_thoth_file, parse_tooltip_catalog,
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
            kind: match path.extension().and_then(|extension| extension.to_str()) {
                Some("lsx") => SourceKind::Lsx,
                Some("khn") => SourceKind::Thoth,
                Some("xml") => SourceKind::Localization,
                Some("txt")
                    if path
                        .to_string_lossy()
                        .replace('\\', "/")
                        .contains("/Story/RawFiles/Goals/") =>
                {
                    SourceKind::Osiris
                }
                _ => SourceKind::PlainStats,
            },
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

fn source_position_nth(text: &str, needle: &str, occurrence: usize) -> Position {
    text.lines()
        .enumerate()
        .filter_map(|(line, source)| source.find(needle).map(|character| (line, character)))
        .nth(occurrence)
        .map(|(line, character)| Position {
            line: u32::try_from(line).unwrap(),
            character: u32::try_from(character).unwrap(),
        })
        .unwrap()
}

#[test]
fn packaged_thoth_catalogs_remain_immutable_across_snapshot_replacement() {
    let schema = Arc::new(SchemaCatalog::default());
    let entry = "Mods/Shared/Scripts/thoth/helpers/WeaponMastery.khn";
    let old_catalog = Arc::new(
        PackagedThothCatalog::from_sources([PackagedThothSource::new(
            "Shared",
            "/game/Data/Shared.pak",
            entry,
            0,
            "function Old() end\n",
        )
        .unwrap()])
        .unwrap(),
    );
    let old_snapshot = WorkspaceSnapshot::new(Arc::clone(&schema), Vec::new(), 1, 200, 200)
        .with_packaged_thoth(Arc::clone(&old_catalog));

    let replacement_catalog = Arc::new(
        PackagedThothCatalog::from_sources([PackagedThothSource::new(
            "Shared",
            "/game/Data/Patch1.pak",
            entry,
            1,
            "function New() end\n",
        )
        .unwrap()])
        .unwrap(),
    );
    let replacement_snapshot = WorkspaceSnapshot::new(schema, Vec::new(), 2, 200, 200)
        .with_packaged_thoth(Arc::clone(&replacement_catalog));

    assert_eq!(old_snapshot.packaged_thoth_count(), 1);
    assert_eq!(replacement_snapshot.packaged_thoth_count(), 1);
    assert_eq!(
        old_snapshot
            .packaged_thoth()
            .sources()
            .next()
            .unwrap()
            .text(),
        "function Old() end\n"
    );
    assert_eq!(
        replacement_snapshot
            .packaged_thoth()
            .sources()
            .next()
            .unwrap()
            .text(),
        "function New() end\n"
    );
    assert_eq!(old_snapshot.packaged_thoth().as_ref(), old_catalog.as_ref());
    assert_eq!(
        replacement_snapshot.packaged_thoth().as_ref(),
        replacement_catalog.as_ref()
    );
}

#[test]
fn packaged_thoth_is_editor_evidence_without_fake_definition_locations() {
    let (workspace, stats_path) = fixture_workspace(200);
    let entry = "Mods/Shared/Scripts/thoth/helpers/Installed.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            PackagedThothSource::new(
                "Shared",
                "/game/Data/Shared.pak",
                entry,
                0,
                "function InstalledHelper(value)\n  return value\nend\nfunction ApplyNative(value)\n  return value\nend\n",
            )
            .unwrap(),
            PackagedThothSource::new(
                "Shared",
                "/game/Data/Patch1.pak",
                "Mods/Shared/Scripts/thoth/helpers/Ambiguous.khn",
                0,
                "function Ambiguous(first)\n  return first\nend\n",
            )
            .unwrap(),
            PackagedThothSource::new(
                "Shared",
                "/game/Data/Patch2.pak",
                "Mods/Shared/Scripts/thoth/helpers/Ambiguous.khn",
                0,
                "function Ambiguous(second)\n  return second\nend\n",
            )
            .unwrap(),
        ])
        .unwrap(),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test", |source| {
            parse_thoth_file(source.text())
        })
        .unwrap(),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Installed\"";
    let overlays = overlay(&workspace, &stats_path, text);
    let completion = workspace.completion(
        &stats_path,
        Position {
            line: 2,
            character: 28,
        },
        &overlays,
        true,
    );
    let installed = completion
        .items
        .iter()
        .find(|item| item.label == "InstalledHelper")
        .unwrap();
    assert_eq!(installed.detail.as_deref(), Some("installed Shared"));
    assert_eq!(installed.new_text, "InstalledHelper(${1:value})");

    let signature_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"InstalledHelper(value, ";
    let signature_overlays = overlay(&workspace, &stats_path, signature_text);
    let signature = workspace
        .signature_help(
            &stats_path,
            Position {
                line: 2,
                character: u32::try_from(signature_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &signature_overlays,
        )
        .unwrap();
    assert_eq!(signature.label, "InstalledHelper(value)");
    assert_eq!(signature.active_parameter, 1);
    assert!(signature.documentation.contains("Installed Thoth evidence"));

    let hover = workspace
        .language_hover(
            &stats_path,
            source_position(signature_text, "InstalledHelper"),
            &signature_overlays,
        )
        .unwrap();
    assert!(hover.contains("**Installed Thoth function** `InstalledHelper`"));
    assert!(hover.contains("Package entries:"));
    assert!(
        workspace
            .definitions_at(
                &stats_path,
                source_position(signature_text, "InstalledHelper"),
                &signature_overlays,
            )
            .is_empty()
    );

    let ambiguous_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Ambi\"";
    let ambiguous_overlays = overlay(&workspace, &stats_path, ambiguous_text);
    let ambiguous = workspace.completion(
        &stats_path,
        Position {
            line: 2,
            character: 23,
        },
        &ambiguous_overlays,
        true,
    );
    assert_eq!(
        ambiguous
            .items
            .iter()
            .filter(|item| item.label == "Ambiguous")
            .count(),
        2
    );
    assert!(ambiguous.items.iter().any(|item| {
        item.label == "Ambiguous"
            && item
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("same-rank ambiguity"))
    }));
}

#[test]
fn resolves_thoth_overrides_and_cross_format_calls() {
    let (workspace, stats_path) = fixture_workspace(200);
    let overlays = OverlaySet::default();
    let target = SymbolTarget::Named {
        kind: Some(bg3_index::THOTH_FUNCTION_KIND.into()),
        name: "OverrideHelper".into(),
    };
    let definitions = workspace.resolve(&target, &overlays);
    assert_eq!(
        definitions
            .iter()
            .map(|definition| definition.module.as_str())
            .collect::<Vec<_>>(),
        ["MyMod", "Item and Spell Bug Fixes", "Shared"]
    );
    assert_eq!(
        definitions[0].definition.fields["Parameters"],
        "projectValue, fallback"
    );

    let stats = fs::read_to_string(&stats_path).unwrap();
    let line = stats.lines().nth(7).unwrap();
    let column = line.find("OverrideHelper").unwrap();
    let from_stats = workspace.definitions_at(
        &stats_path,
        Position {
            line: 7,
            character: u32::try_from(column).unwrap(),
        },
        &overlays,
    );
    assert_eq!(from_stats.len(), 3);

    let thoth_path = fixtures().join("project/Mods/MyMod/Scripts/thoth/helpers/MyMod.khn");
    let thoth = fs::read_to_string(&thoth_path).unwrap();
    let call_line = thoth.lines().nth(5).unwrap();
    let call_column = call_line.find("DependencyOnly").unwrap();
    let from_thoth = workspace.definitions_at(
        &thoth_path,
        Position {
            line: 5,
            character: u32::try_from(call_column).unwrap(),
        },
        &overlays,
    );
    assert_eq!(from_thoth.len(), 1);
    assert_eq!(from_thoth[0].module, "Item and Spell Bug Fixes");

    let references = workspace.references_at(
        &definitions[0].path,
        definitions[0].definition.selection_range.start,
        false,
        &overlays,
    );
    assert_eq!(references.len(), 3);
    assert!(
        references
            .iter()
            .any(|reference| reference.path == stats_path)
    );
    assert!(references.iter().any(|reference| {
        reference
            .path
            .ends_with("Public/MyMod/Progressions/Progressions.lsx")
    }));
    assert!(
        references
            .iter()
            .any(|reference| reference.path == thoth_path)
    );
}

#[test]
fn completes_and_describes_declared_thoth_helpers() {
    let (workspace, stats_path) = fixture_workspace(200);
    let stats_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Over";
    let stats_overlays = overlay(&workspace, &stats_path, stats_text);
    let completion = workspace.completion(
        &stats_path,
        Position {
            line: 2,
            character: 20,
        },
        &stats_overlays,
        true,
    );
    let helper = completion
        .items
        .iter()
        .find(|item| item.label == "OverrideHelper")
        .unwrap();
    assert_eq!(helper.detail.as_deref(), Some("MyMod"));
    assert_eq!(
        helper.new_text,
        "OverrideHelper(${1:projectValue}, ${2:fallback})"
    );
    assert!(helper.snippet);

    let signature_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"OverrideHelper(value,";
    let signature_overlays = overlay(&workspace, &stats_path, signature_text);
    let signature = workspace
        .signature_help(
            &stats_path,
            Position {
                line: 2,
                character: u32::try_from(signature_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &signature_overlays,
        )
        .unwrap();
    assert_eq!(signature.label, "OverrideHelper(projectValue, fallback)");
    assert_eq!(signature.active_parameter, 1);

    let thoth_path = fixtures().join("project/Mods/MyMod/Scripts/thoth/helpers/MyMod.khn");
    let thoth_text = "function Caller(value)\n  return Dep\nend\n";
    let thoth_overlays = overlay(&workspace, &thoth_path, thoth_text);
    let thoth_completion = workspace.completion(
        &thoth_path,
        Position {
            line: 1,
            character: 12,
        },
        &thoth_overlays,
        false,
    );
    assert!(
        thoth_completion
            .items
            .iter()
            .any(|item| item.label == "DependencyOnly")
    );

    let definitions = workspace.resolve(
        &SymbolTarget::Named {
            kind: Some(bg3_index::THOTH_FUNCTION_KIND.into()),
            name: "OverrideHelper".into(),
        },
        &OverlaySet::default(),
    );
    let hover = workspace
        .hover(
            &definitions[0].path,
            definitions[0].definition.selection_range.start,
            &OverlaySet::default(),
        )
        .unwrap();
    assert!(hover.contains("**Thoth function** `OverrideHelper`"));
    assert!(hover.contains("Signature: `OverrideHelper(projectValue, fallback)`"));
}

#[test]
fn loose_thoth_hover_preserves_same_rank_ambiguity() {
    let schema = Arc::new(SchemaCatalog::default());
    let first_path = PathBuf::from("/synthetic/MyMod/Scripts/thoth/helpers/First.khn");
    let second_path = PathBuf::from("/synthetic/MyMod/Scripts/thoth/helpers/Second.khn");
    let parse_thoth = |path: &Path, text: &str| {
        parse_source(
            SourceFile {
                path: path.to_owned(),
                kind: SourceKind::Thoth,
            },
            text,
            &schema,
            "English",
        )
        .expect("synthetic Thoth source")
    };
    let module = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "MyMod".into(),
            root: PathBuf::from("/synthetic/MyMod"),
            role: ModuleRole::Project,
        },
        vec![
            parse_thoth(&first_path, "function Shared(first) end\n"),
            parse_thoth(&second_path, "function Shared(first, second) end\n"),
        ],
    ));
    let workspace = WorkspaceSnapshot::new(schema, vec![module], 1, 200, 200);
    let stats_path = PathBuf::from("/synthetic/MyMod/Stats/Passive.txt");
    let stats_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Shared(value\"";
    let parsed_stats = parse_source(
        SourceFile {
            path: stats_path.clone(),
            kind: SourceKind::PlainStats,
        },
        stats_text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic Stats source");
    let mut overlays = OverlaySet::default();
    overlays.insert(
        stats_path.clone(),
        OverlayDocument {
            module: "MyMod".into(),
            version: 1,
            text: stats_text.into(),
            parsed: Arc::new(parsed_stats),
        },
    );

    let hover = workspace
        .language_hover(
            &stats_path,
            source_position(stats_text, "Shared"),
            &overlays,
        )
        .expect("ambiguous loose Thoth hover");
    assert!(hover.contains("**Thoth function** `Shared`"));
    assert!(hover.contains("Module: `MyMod`"));
    assert!(hover.contains("Declarations: `2`"));
    assert!(hover.contains("Same-rank Thoth declarations are ambiguous"));
    assert!(!hover.contains("Signature: `Shared("));
}

#[test]
fn uses_explicit_thoth_annotations_for_editor_evidence() {
    let (workspace, stats_path) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Scripts/thoth/helpers/Annotated.khn");
    let text = "---@class Weapon\n---@field IsValid boolean\n---@alias Result string|nil\n---@param weapon Weapon?\n---@return Result\nfunction Annotated(weapon)\n---@type Result\nlocal result = Annotated(weapon)\nreturn result\nend\n";
    let signature_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Annotated(value\"";
    let mut overlays = overlay(&workspace, &stats_path, signature_text);
    let parsed = parse_source(
        SourceFile {
            path: path.clone(),
            kind: SourceKind::Thoth,
        },
        text,
        &workspace.schema,
        "English",
    )
    .unwrap();
    overlays.insert(
        path.clone(),
        OverlayDocument {
            module: "MyMod".into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );

    let completion = workspace.completion(
        &path,
        Position {
            line: 7,
            character: 22,
        },
        &overlays,
        false,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "Annotated")
        .expect("annotated completion");
    assert_eq!(
        item.detail.as_deref(),
        Some("Annotated(weapon: Weapon|nil): Result")
    );
    assert_eq!(
        item.documentation.as_deref(),
        Some("Explicit Thoth annotation.")
    );

    let signature = workspace
        .signature_help(
            &stats_path,
            Position {
                line: 2,
                character: u32::try_from(signature_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &overlays,
        )
        .expect("annotated signature");
    assert_eq!(signature.label, "Annotated(weapon: Weapon|nil): Result");
    assert_eq!(signature.parameters, vec!["weapon: Weapon|nil"]);

    let hover = workspace
        .hover(&path, source_position(text, "Annotated"), &overlays)
        .expect("annotated hover");
    assert!(hover.contains("Weapon|nil"));
    assert!(hover.contains("Result"));

    let variable_hover = workspace
        .language_hover(&path, source_position(text, "result ="), &overlays)
        .expect("@type hover");
    assert!(variable_hover.contains("Type: `Result`"));

    let class_hover = workspace
        .language_hover(&path, source_position(text, "Weapon"), &overlays)
        .expect("class hover");
    assert!(class_hover.contains("**Thoth class** `Weapon`"));
    assert!(class_hover.contains("Fields:\n"));
    assert!(class_hover.contains("IsValid"));
}

#[test]
fn documents_declared_helpers_from_prose_and_returns_alias() {
    let schema = Arc::new(SchemaCatalog::default());
    let helper_path = PathBuf::from("/synthetic/MyMod/Scripts/thoth/helpers/Documented.khn");
    let helper_text = concat!(
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
    let module = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "MyMod".into(),
            root: PathBuf::from("/synthetic/MyMod"),
            role: ModuleRole::Project,
        },
        vec![
            parse_source(
                SourceFile {
                    path: helper_path,
                    kind: SourceKind::Thoth,
                },
                helper_text,
                &schema,
                "English",
            )
            .unwrap(),
        ],
    ));
    let workspace = WorkspaceSnapshot::new(schema, vec![module], 1, 200, 200);

    let caller_path = PathBuf::from("/synthetic/MyMod/Stats/Generated/Data/Caller.txt");
    let caller_text = concat!(
        "new entry \"TEST\"\n",
        "type \"SpellData\"\n",
        "data \"RequirementConditions\" \"Helper(1) and Fallback(2);\"",
    );
    let overlays = overlay(&workspace, &caller_path, caller_text);

    let hover = workspace
        .hover(
            &caller_path,
            source_position(caller_text, "Helper"),
            &overlays,
        )
        .expect("documented hover");
    assert!(hover.contains("**Thoth function** `Helper`"), "{hover}");
    assert!(
        hover.contains("Signature: `Helper(value: number): boolean`"),
        "{hover}"
    );
    assert!(
        hover.contains("Returns whether the helper applies."),
        "{hover}"
    );
    assert!(hover.contains("Returns: `boolean`"), "{hover}");

    let fallback_hover = workspace
        .hover(
            &caller_path,
            source_position(caller_text, "Fallback"),
            &overlays,
        )
        .expect("prose-only hover");
    assert!(
        fallback_hover.contains("Signature: `Fallback(value)`"),
        "{fallback_hover}"
    );
    assert!(
        fallback_hover.contains("Documents the fallback without typing it."),
        "{fallback_hover}"
    );

    let signature_text = concat!(
        "new entry \"TEST\"\n",
        "type \"SpellData\"\n",
        "data \"RequirementConditions\" \"Helper(1\"",
    );
    let signature_overlays = overlay(&workspace, &caller_path, signature_text);
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line: 2,
                character: u32::try_from(signature_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &signature_overlays,
        )
        .expect("documented signature help");
    assert_eq!(signature.label, "Helper(value: number): boolean");
    assert!(
        signature
            .documentation
            .to_string()
            .contains("Returns whether the helper applies.")
    );

    let completion_text = concat!(
        "new entry \"TEST\"\n",
        "type \"SpellData\"\n",
        "data \"RequirementConditions\" \"Fall",
    );
    let completion_overlays = overlay(&workspace, &caller_path, completion_text);
    let completion = workspace.completion(
        &caller_path,
        Position {
            line: 2,
            character: u32::try_from(completion_text.lines().nth(2).unwrap().len()).unwrap(),
        },
        &completion_overlays,
        false,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "Fallback")
        .expect("prose-only completion");
    assert_eq!(item.detail.as_deref(), Some("MyMod"));
    assert!(item.documentation.as_deref().is_some_and(|documentation| {
        documentation.contains("Documents the fallback without typing it.")
    }));
}

#[test]
fn annotation_precedence_masks_lower_and_conflicting_contracts() {
    let schema = Arc::new(SchemaCatalog::default());
    let lower_path = PathBuf::from("/synthetic/Lower/Annotated.khn");
    let higher_path = PathBuf::from("/synthetic/Higher/Annotated.khn");
    let caller_path = PathBuf::from("/synthetic/Higher/Caller.khn");
    let lower_text = "---@param value string\n---@return boolean\nfunction Annotated(value) end\n";
    let higher_text = "function Annotated(value) end\nlocal result = Annotated(value)\n";
    let parse = |path: &Path, text: &str| {
        parse_source(
            SourceFile {
                path: path.to_owned(),
                kind: SourceKind::Thoth,
            },
            text,
            &schema,
            "English",
        )
        .unwrap()
    };
    let lower = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Lower".into(),
            root: PathBuf::from("/synthetic/Lower"),
            role: ModuleRole::Base,
        },
        vec![parse(&lower_path, lower_text)],
    ));
    let higher = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Higher".into(),
            root: PathBuf::from("/synthetic/Higher"),
            role: ModuleRole::Base,
        },
        vec![parse(&higher_path, higher_text)],
    ));
    let workspace = WorkspaceSnapshot::new(schema.clone(), vec![lower, higher], 1, 200, 200);
    let mut overlays = OverlaySet::default();
    let caller_text = "local result = Annotated(value, ";
    overlays.insert(
        caller_path.clone(),
        OverlayDocument {
            module: "Higher".into(),
            version: 1,
            text: caller_text.into(),
            parsed: Arc::new(parse(&caller_path, caller_text)),
        },
    );
    let signature = workspace
        .signature_help(
            &caller_path,
            Position {
                line: 0,
                character: u32::try_from(caller_text.len()).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert_eq!(signature.label, "Annotated(value)");
    assert!(!signature.documentation.contains("Explicit"));

    let same_a = PathBuf::from("/synthetic/Higher/A.khn");
    let same_b = PathBuf::from("/synthetic/Higher/B.khn");
    let same_caller = PathBuf::from("/synthetic/Higher/C.khn");
    let a_text = "---@param value string\nfunction Conflicting(value) end\n";
    let b_text = "---@param value number\nfunction Conflicting(value) end\n";
    let c_text = "local result = Conflicting(value, ";
    let higher = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Higher".into(),
            root: PathBuf::from("/synthetic/Higher"),
            role: ModuleRole::Base,
        },
        vec![
            parse(&same_a, a_text),
            parse(&same_b, b_text),
            parse(&same_caller, c_text),
        ],
    ));
    let workspace = WorkspaceSnapshot::new(
        schema.clone(),
        vec![
            Arc::new(ModuleIndex::new(
                ModuleSpec {
                    name: "Lower".into(),
                    root: PathBuf::from("/synthetic/Lower"),
                    role: ModuleRole::Base,
                },
                Vec::new(),
            )),
            higher,
        ],
        1,
        200,
        200,
    );
    let mut overlays = OverlaySet::default();
    overlays.insert(
        same_caller.clone(),
        OverlayDocument {
            module: "Higher".into(),
            version: 1,
            text: c_text.into(),
            parsed: Arc::new(parse(&same_caller, c_text)),
        },
    );
    let signature = workspace
        .signature_help(
            &same_caller,
            Position {
                line: 0,
                character: u32::try_from(c_text.len()).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(!signature.label.contains("string"));
    assert!(!signature.label.contains("number"));
    assert!(!signature.documentation.contains("Explicit"));
}

#[test]
fn annotation_overlay_replaces_disk_contract_and_type_hover_is_range_limited() {
    let schema = Arc::new(SchemaCatalog::default());
    let path = PathBuf::from("/synthetic/Project/Annotated.khn");
    let disk_text = "---@param value string\nfunction Annotated(value) end\nlocal result = value\n";
    let overlay_text =
        "---@param value number\nfunction Annotated(value) end\nlocal result = value\n";
    let parse = |text: &str| {
        parse_source(
            SourceFile {
                path: path.clone(),
                kind: SourceKind::Thoth,
            },
            text,
            &schema,
            "English",
        )
        .unwrap()
    };
    let workspace = WorkspaceSnapshot::new(
        schema.clone(),
        vec![Arc::new(ModuleIndex::new(
            ModuleSpec {
                name: "Project".into(),
                root: PathBuf::from("/synthetic/Project"),
                role: ModuleRole::Project,
            },
            vec![parse(disk_text)],
        ))],
        1,
        200,
        200,
    );
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.clone(),
        OverlayDocument {
            module: "Project".into(),
            version: 2,
            text: overlay_text.into(),
            parsed: Arc::new(parse(overlay_text)),
        },
    );
    let hover = workspace
        .hover(&path, source_position(overlay_text, "Annotated"), &overlays)
        .unwrap();
    assert!(hover.contains("number"));
    assert!(!hover.contains("string"));
    let later_use = workspace.language_hover(
        &path,
        Position {
            line: 2,
            character: 16,
        },
        &overlays,
    );
    assert!(later_use.is_none());
}

#[test]
fn thoth_overlays_replace_and_restore_disk_declarations() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Scripts/thoth/helpers/MyMod.khn");
    let target = SymbolTarget::Named {
        kind: Some(bg3_index::THOTH_FUNCTION_KIND.into()),
        name: "OverrideHelper".into(),
    };
    let overlays = overlay(
        &workspace,
        &path,
        "function UnsavedHelper(value)\n  return value\nend\n",
    );

    let with_overlay = workspace.resolve(&target, &overlays);
    assert_eq!(
        with_overlay
            .iter()
            .map(|definition| definition.module.as_str())
            .collect::<Vec<_>>(),
        ["Item and Spell Bug Fixes", "Shared"]
    );
    assert_eq!(workspace.resolve(&target, &OverlaySet::default()).len(), 3);
}

#[test]
fn tracks_osiris_variables_by_rule_and_replaces_them_in_overlays() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell((CHARACTER)_Caster, \"Spell\", \"Context\", \"Arg\", 1)\n",
        "AND\n",
        "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _BonusActionPoints)\n",
        "THEN\n",
        "DB_Tracked(_Caster, _BonusActionPoints);\n",
        "IF\n",
        "UnknownEvent(1)\n",
        "AND\n",
        "_Caster >= 1\n",
        "THEN\n",
        "DB_Isolated(_Caster);\n",
        "IF\n",
        "UnknownEvent(1)\n",
        "AND\n",
        "_Unknown >= 1\n",
        "THEN\n",
        "DB_Unknown(_Unknown);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, text);

    let bound_use = source_position_nth(text, "_Caster", 1);
    let binding = source_position_nth(text, "_Caster", 0);
    let hover = workspace.hover(&path, bound_use, &overlays).unwrap();
    assert!(hover.contains("**Osiris variable** `_Caster`"), "{hover}");
    assert!(!hover.contains("Bound by:"), "{hover}");
    assert!(!hover.contains("Binding:"), "{hover}");
    assert!(hover.contains("Type: `CHARACTER`"), "{hover}");
    assert!(!hover.contains("Evidence:"), "{hover}");
    assert_eq!(
        hover.range,
        Some(bg3_index::TextRange {
            start: bound_use,
            end: Position {
                line: bound_use.line,
                character: bound_use.character + 7,
            },
        })
    );

    assert_eq!(
        workspace.definition_locations_at(&path, bound_use, &overlays),
        vec![bg3_ide::SourceLocation {
            path: path.clone(),
            range: bg3_index::TextRange {
                start: binding,
                end: Position {
                    line: binding.line,
                    character: binding.character + 7,
                },
            },
        }]
    );
    let references = workspace.references_at(&path, bound_use, false, &overlays);
    assert!(
        !references
            .iter()
            .any(|location| location.range.start == binding)
    );
    let references_with_binding = workspace.references_at(&path, bound_use, true, &overlays);
    assert!(
        references_with_binding
            .iter()
            .any(|location| location.range.start == binding)
    );

    let isolated_use = source_position_nth(text, "_Caster", 3);
    let isolated_references = workspace.references_at(&path, isolated_use, false, &overlays);
    assert!(!isolated_references.is_empty());
    assert!(
        isolated_references
            .iter()
            .all(|location| location.range.start.line >= isolated_use.line)
    );
    assert!(
        workspace
            .definition_locations_at(&path, isolated_use, &overlays)
            .is_empty()
    );
    let unknown = source_position_nth(text, "_Unknown", 0);
    let unknown_hover = workspace.hover(&path, unknown, &overlays).unwrap();
    assert!(!unknown_hover.contains("Bound by:"));
    assert!(!unknown_hover.contains("Binding:"));
    assert!(
        workspace
            .definition_locations_at(&path, unknown, &overlays)
            .is_empty()
    );

    let overlay_text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell((CHARACTER)_OverlayCaster, \"Spell\", \"Context\", \"Arg\", 1)\n",
        "THEN\n",
        "DB_Overlay(_OverlayCaster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlay_set = overlay(&workspace, &path, overlay_text);
    let overlay_use = source_position(overlay_text, "_OverlayCaster");
    let overlay_hover = workspace.hover(&path, overlay_use, &overlay_set).unwrap();
    assert!(
        overlay_hover.contains("`_OverlayCaster`"),
        "{overlay_hover}"
    );
    assert!(
        workspace
            .definition_locations_at(&path, overlay_use, &overlay_set)
            .iter()
            .all(|location| location.range.start.line == overlay_use.line)
    );
}

#[test]
fn database_help_uses_write_types_when_reads_have_input_evidence() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell((CHARACTER)_Caster, \"Spell\", \"Context\", \"Arg\", 1)\n",
        "THEN\n",
        "DB_ReadContaminated(_Caster);\n",
        "IF\n",
        "DB_ReadContaminated(_Caster)\n",
        "AND\n",
        "HasPassive(_Caster, \"SomePassive\", 0)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "NOT DB_ReadContaminated((GUIDSTRING)11111111-1111-1111-1111-111111111111);\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, text);

    let database = workspace
        .hover(
            &path,
            source_position(text, "DB_ReadContaminated"),
            &overlays,
        )
        .expect("database hover");
    assert!(
        database.contains("Signature: `DB_ReadContaminated(CHARACTER)`"),
        "{database}"
    );
    assert!(database.contains("Writes: `1`"), "{database}");
    assert!(database.contains("Reads: `1`"), "{database}");
    assert!(!database.contains("conflicting"), "{database}");

    let database_bound_variable = workspace
        .hover(&path, source_position_nth(text, "_Caster", 2), &overlays)
        .expect("database-bound variable hover");
    assert!(
        database_bound_variable.contains("Type: `CHARACTER`"),
        "{database_bound_variable}"
    );

    let read_line = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("DB_ReadContaminated(_Caster)"))
        .expect("database read");
    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: u32::try_from(read_line.0).unwrap(),
                character: u32::try_from(read_line.1.find(')').unwrap()).unwrap(),
            },
            &overlays,
        )
        .expect("database signature help");
    assert_eq!(signature.label, "DB_ReadContaminated(CHARACTER)");
}

#[test]
fn database_bound_variable_hover_is_conservative_and_overlay_aware() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let source = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "Died((CHARACTER)_Writer)\n",
        "THEN\n",
        "DB_Conflicting((CHARACTER)_Writer);\n",
        "IF\n",
        "Died((CHARACTER)_OtherWriter)\n",
        "THEN\n",
        "DB_Conflicting((GUIDSTRING)_OtherWriter);\n",
        "IF\n",
        "DB_Conflicting(_ConflictingRead)\n",
        "AND\n",
        "HasPassive(_ConflictingRead, \"SomePassive\", 0)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "DB_NoWrite(_ReadOnly)\n",
        "AND\n",
        "HasPassive(_ReadOnly, \"SomePassive\", 0)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "UsingSpell((CHARACTER)_EventCaster, \"Spell\", \"Type\", \"Element\", 1)\n",
        "AND\n",
        "DB_NoWrite(_EventCaster)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "IF\n",
        "UnknownEvent(_Negated)\n",
        "AND\n",
        "NOT DB_Conflicting(_Negated)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, source);

    let hover_at = |name: &str, occurrence: usize| {
        workspace
            .hover(
                &path,
                source_position_nth(source, name, occurrence),
                &overlays,
            )
            .unwrap_or_else(|| panic!("no hover for {name}"))
    };
    let conflicting = hover_at("_ConflictingRead", 0);
    assert!(!conflicting.contains("Type:"), "{conflicting}");
    let read_only = hover_at("_ReadOnly", 0);
    assert!(!read_only.contains("Type:"), "{read_only}");
    let event_bound = hover_at("_EventCaster", 0);
    assert!(event_bound.contains("Type: `CHARACTER`"), "{event_bound}");
    let negated = hover_at("_Negated", 0);
    assert!(!negated.contains("Type:"), "{negated}");

    let character_source = concat!(
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n",
        "IF\nDied((CHARACTER)_Writer)\nTHEN\nDB_Overlay((CHARACTER)_Writer);\n",
        "IF\nDB_Overlay(_Read)\nTHEN\nGoalCompleted;\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let character_overlays = overlay(&workspace, &path, character_source);
    let character = workspace
        .hover(
            &path,
            source_position_nth(character_source, "_Read", 0),
            &character_overlays,
        )
        .unwrap();
    assert!(character.contains("Type: `CHARACTER`"), "{character}");

    let guid_source = character_source.replace("(CHARACTER)_Writer", "(GUIDSTRING)_Writer");
    let guid_overlays = overlay(&workspace, &path, &guid_source);
    let guid = workspace
        .hover(
            &path,
            source_position_nth(&guid_source, "_Read", 0),
            &guid_overlays,
        )
        .unwrap();
    assert!(guid.contains("Type: `GUIDSTRING`"), "{guid}");
}

#[test]
fn database_binding_type_survives_later_engine_input_use() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let source = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "Died((CHARACTER)_SourceWriter)\n",
        "THEN\n",
        "DB_Source((CHARACTER)_SourceWriter);\n",
        "IF\n",
        "DB_Source(_Caster)\n",
        "AND\n",
        "HasPassive(_Caster, \"SomePassive\", 0)\n",
        "THEN\n",
        "DB_Target(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, source);

    let caster = workspace
        .hover(&path, source_position_nth(source, "_Caster", 0), &overlays)
        .expect("database-bound caster hover");
    assert!(caster.contains("Type: `CHARACTER`"), "{caster}");
    assert!(!caster.contains("GUIDSTRING"), "{caster}");

    let target = workspace
        .hover(&path, source_position(source, "DB_Target"), &overlays)
        .expect("target database hover");
    assert!(!target.contains("GUIDSTRING"), "{target}");
    assert!(!target.contains("conflicting"), "{target}");
}

#[test]
fn osiris_variable_hover_and_definition_follow_the_source_ordered_producer() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let source = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "(CHARACTER)_Receiver.Died((CHARACTER)_Writer)\n",
        "THEN\n",
        "DB_Source((CHARACTER)_Writer);\n",
        "IF\n",
        "UnknownEvent(_Before)\n",
        "AND\n",
        "DB_Source(_FromDb)\n",
        "AND\n",
        "IntegerSum(1, 2, (INTEGER)_TypedOut)\n",
        "THEN\n",
        "DB_Result(_Before, _FromDb, _TypedOut);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, source);

    let before = workspace
        .hover(&path, source_position_nth(source, "_Before", 0), &overlays)
        .unwrap();
    assert!(!before.contains("Type:"), "{before}");
    assert!(
        workspace
            .definition_locations_at(&path, source_position_nth(source, "_Before", 0), &overlays,)
            .is_empty()
    );

    let receiver = workspace
        .hover(
            &path,
            source_position_nth(source, "_Receiver", 0),
            &overlays,
        )
        .unwrap();
    assert!(receiver.contains("Type: `CHARACTER`"), "{receiver}");
    assert!(
        workspace
            .definition_locations_at(
                &path,
                source_position_nth(source, "_Receiver", 0),
                &overlays,
            )
            .is_empty()
    );

    let from_db = workspace
        .hover(&path, source_position_nth(source, "_FromDb", 0), &overlays)
        .unwrap();
    assert!(from_db.contains("Type: `CHARACTER`"), "{from_db}");
    let from_db_definition = workspace.definition_locations_at(
        &path,
        source_position_nth(source, "_FromDb", 1),
        &overlays,
    );
    assert_eq!(from_db_definition.len(), 1);
    assert_eq!(
        from_db_definition[0].range,
        bg3_index::TextRange {
            start: source_position_nth(source, "_FromDb", 0),
            end: Position {
                line: source_position_nth(source, "_FromDb", 0).line,
                character: source_position_nth(source, "_FromDb", 0).character + 7,
            },
        }
    );

    let typed_out = workspace
        .hover(
            &path,
            source_position_nth(source, "_TypedOut", 0),
            &overlays,
        )
        .unwrap();
    assert!(typed_out.contains("Type: `INTEGER`"), "{typed_out}");
    assert_eq!(
        workspace
            .definition_locations_at(
                &path,
                source_position_nth(source, "_TypedOut", 1),
                &overlays,
            )
            .len(),
        1
    );
}

#[test]
fn osiris_variable_hover_prefers_an_occurrence_cast_over_database_type() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let source = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "DB_Conflicting((CHARACTER)_Value)\n",
        "THEN\n",
        "DB_Conflicting((GUIDSTRING)_Value);\n",
        "IF\n",
        "DB_Missing((CHARACTER)_Missing)\n",
        "THEN\n",
        "GoalCompleted;\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, source);

    let conflicting = workspace
        .hover(&path, source_position_nth(source, "_Value", 0), &overlays)
        .expect("conflicting database variable hover");
    assert!(conflicting.contains("Type: `CHARACTER`"), "{conflicting}");

    let (missing_line, missing_source) = source
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("DB_Missing"))
        .unwrap();
    let missing_position = Position {
        line: u32::try_from(missing_line).unwrap(),
        character: u32::try_from(missing_source.rfind("_Missing").unwrap()).unwrap(),
    };
    let missing = workspace
        .hover(&path, missing_position, &overlays)
        .expect("missing database variable hover");
    assert!(missing.contains("Type: `CHARACTER`"), "{missing}");
}

#[test]
fn supports_source_backed_osiris_navigation_signatures_and_overlays() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let text = fs::read_to_string(&path).unwrap();
    let overlays = OverlaySet::default();
    let database_position = source_position(&text, "DB_Tracked");

    let definitions = workspace.definitions_at(&path, database_position, &overlays);
    assert_eq!(definitions.len(), 4);
    assert_eq!(definitions[0].module, "MyMod");
    assert!(definitions.iter().all(|definition| !definition.ambiguous));

    let references = workspace.references_at(&path, database_position, false, &overlays);
    assert_eq!(references.len(), 10);

    let hover = workspace
        .hover(&path, database_position, &overlays)
        .unwrap();
    assert!(hover.contains("**Osiris database** `DB_Tracked/2`"));
    assert!(hover.contains("Signature: `DB_Tracked(CHARACTER, INTEGER)`"));
    assert!(hover.contains("`MainGoal`"));
    assert!(hover.contains("`SecondaryGoal`"));

    let packed_position = source_position(&text, "DB_PackedOnly");
    assert!(
        workspace
            .definitions_at(&path, packed_position, &overlays)
            .is_empty()
    );
    let packed_hover = workspace.hover(&path, packed_position, &overlays).unwrap();
    assert!(packed_hover.contains("No write is visible"));

    let observed_callable_position = source_position(&text, "ApplyExample");
    let observed_callable = workspace
        .hover(&path, observed_callable_position, &overlays)
        .expect("call-only Osiris hover");
    assert!(observed_callable.contains("**Osiris callable** `ApplyExample/1`"));
    assert!(observed_callable.contains("Arity: `1`"));
    assert!(observed_callable.contains("Callable kind and parameter types are unknown"));
    assert!(!observed_callable.contains("Osiris procedure"));
    assert!(!observed_callable.contains("Osiris query"));

    let parent_position = source_position(&text, "SharedGoal");
    let parent = workspace.definitions_at(&path, parent_position, &overlays);
    assert_eq!(parent.len(), 1);
    assert_eq!(parent[0].definition.name, "SharedGoal");

    let callable_position = source_position(&text, "SharedProc");
    let callables = workspace.definitions_at(&path, callable_position, &overlays);
    assert_eq!(callables.len(), 4);
    assert_eq!(callables[0].module, "MyMod");
    assert_eq!(callables[1].module, "MyMod");
    assert!(callables[0].ambiguous);
    assert!(callables[1].ambiguous);
    assert!(
        callables[2..]
            .iter()
            .all(|definition| !definition.ambiguous)
    );

    let symbols = workspace.document_symbols(&path, &overlays);
    assert_eq!(symbols.len(), 5);
    assert!(symbols.iter().any(|symbol| symbol.name == "DB_Tracked"));
    assert!(!symbols.iter().any(|symbol| symbol.name == "DB_PackedOnly"));

    let open = overlay(&workspace, &path, &text);
    let signature_line = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("DB_Tracked(_Actor, _Count)"))
        .unwrap();
    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: u32::try_from(signature_line.0).unwrap(),
                character: u32::try_from(signature_line.1.find(',').unwrap() + 1).unwrap(),
            },
            &open,
        )
        .unwrap();
    assert_eq!(signature.label, "DB_Tracked(CHARACTER, INTEGER)");
    assert_eq!(signature.active_parameter, 1);

    let completion_text = "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\nEvent()\nTHEN\nDB_Tr\nEXITSECTION\nENDEXITSECTION\n";
    let completion_overlays = overlay(&workspace, &path, completion_text);
    let completion = workspace.completion(
        &path,
        Position {
            line: 7,
            character: 5,
        },
        &completion_overlays,
        true,
    );
    let database = completion
        .items
        .iter()
        .find(|item| item.label == "DB_Tracked")
        .unwrap();
    assert_eq!(database.new_text, "DB_Tracked(${1:column1}, ${2:column2})");

    let invalid_completion_text = "Version 1\nDB_Tr\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nEXITSECTION\nENDEXITSECTION\n";
    let invalid_completion_overlays = overlay(&workspace, &path, invalid_completion_text);
    assert!(
        workspace
            .completion(
                &path,
                Position {
                    line: 1,
                    character: 5,
                },
                &invalid_completion_overlays,
                true,
            )
            .items
            .is_empty()
    );

    let replacement = "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nPROC\nUnsavedProc((INTEGER)_Value)\nTHEN\nDB_Unsaved(_Value);\nEXITSECTION\nENDEXITSECTION\n";
    let replacement_overlays = overlay(&workspace, &path, replacement);
    let replacement_symbols = workspace.document_symbols(&path, &replacement_overlays);
    assert!(
        replacement_symbols
            .iter()
            .any(|symbol| symbol.name == "UnsavedProc")
    );
    assert!(
        replacement_symbols
            .iter()
            .any(|symbol| symbol.name == "DB_Unsaved")
    );
    assert!(
        replacement_symbols
            .iter()
            .all(|symbol| symbol.name != "SharedProc")
    );
}

#[test]
fn publishes_only_proven_osiris_syntax_diagnostics() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let malformed = "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\nBroken(\nEXITSECTION\nENDEXITSECTION\n";
    let overlays = overlay(&workspace, &path, malformed);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));

    assert!(!diagnostics.is_empty());
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "osiris-syntax-error")
    );
}

#[test]
fn diagnoses_proven_osiris_database_alias_conflicts() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let conflicting = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\nDied(_Char)\nTHEN\nDB_Conflict(_Char);\n",
        "IF\nAddedTo(_Item, _Holder, _How)\nTHEN\nDB_Conflict(_Item);\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, conflicting);
    let diagnostics = workspace.diagnostics(&path, &overlays, None);

    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code, "osiris-database-alias-mismatch");
    assert_eq!(diagnostic.severity, bg3_ide::DiagnosticSeverity::Error);
    assert!(
        diagnostic
            .message
            .contains("Column 1 of `DB_Conflict/1` is established as `CHARACTER`"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("supplies `GUIDSTRING`"),
        "{}",
        diagnostic.message
    );
    let conflict_line = conflicting
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("DB_Conflict(_Item)"))
        .unwrap();
    assert_eq!(
        diagnostic.range.start.line,
        u32::try_from(conflict_line.0).unwrap()
    );

    // An explicit cast at the conflicting argument removes the diagnostic,
    // and engine calls stay unchecked so specific values remain valid.
    let cleared = conflicting.replace("DB_Conflict(_Item);", "DB_Conflict((CHARACTER)_Item);");
    let cleared_overlays = overlay(&workspace, &path, &cleared);
    assert!(
        workspace
            .diagnostics(&path, &cleared_overlays, None)
            .is_empty()
    );

    // Unknown events contribute no evidence and never conflict.
    let unknown = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\nUnknownEvent(_Who)\nTHEN\nDB_Quiet(_Who);\n",
        "IF\nOtherUnknown(_Who)\nTHEN\nDB_Quiet(_Who);\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let unknown_overlays = overlay(&workspace, &path, unknown);
    assert!(
        workspace
            .diagnostics(&path, &unknown_overlays, None)
            .is_empty()
    );
}

#[test]
fn ignores_osiris_database_reads_when_checking_aliases() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/MainGoal.txt");
    let source = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        // This read precedes the first write and must not establish the
        // database column as GUIDSTRING.
        "IF\nDB_ReadBinding((GUIDSTRING)_Other)\nTHEN\nGoalCompleted;\n",
        "IF\nDied((CHARACTER)_Caster)\nTHEN\nDB_ReadBinding(_Caster);\n",
        // The DB condition binds _Caster from the matching row. The later
        // HasPassive input must not make this read appear to supply GUIDSTRING.
        "IF\nDB_ReadBinding(_Caster)\nAND\nHasPassive(_Caster, \"SomePassive\", 0)\nTHEN\nDB_ReadResult(_Caster);\n",
        // A read after the write with an explicit, incompatible cast also
        // does not conflict with the existing database column.
        "IF\nDB_ReadBinding((GUIDSTRING)_Other)\nTHEN\nGoalCompleted;\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let overlays = overlay(&workspace, &path, source);

    assert!(workspace.diagnostics(&path, &overlays, None).is_empty());
}

#[test]
fn toggles_osiris_alias_diagnostics_through_cross_goal_overlays() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Story/RawFiles/Goals/SecondaryGoal.txt");
    // MainGoal on disk establishes DB_Tracked column 1 as CHARACTER through an
    // explicit cast. The overlay supplies a generic GUIDSTRING from a curated
    // event signature in another goal of the same workspace.
    let conflicting = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\nAddedTo(_Item, _Holder, _How)\nTHEN\nDB_Tracked(_Item, 3);\n",
        "EXITSECTION\nENDEXITSECTION\n",
    );
    let mut overlays = overlay(&workspace, &path, conflicting);
    let diagnostics = workspace.diagnostics(&path, &overlays, None);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "osiris-database-alias-mismatch");
    assert!(diagnostics[0].message.contains("`DB_Tracked/2`"));

    let original = fs::read_to_string(&path).unwrap();
    let mut restored = overlay(&workspace, &path, &original)
        .get(&path)
        .unwrap()
        .clone();
    restored.version += 1;
    overlays.insert(path.clone(), restored);
    assert!(workspace.diagnostics(&path, &overlays, None).is_empty());
}

#[test]
fn publishes_thoth_syntax_diagnostics_and_skips_semantic_checks() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Mods/MyMod/Scripts/thoth/helpers/MyMod.khn");
    let malformed = "function Broken(entity)\n  @\nend\n";
    let mut overlays = overlay(&workspace, &path, malformed);
    let diagnostics = workspace.diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning));

    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.code, "thoth-syntax-error");
    assert_eq!(diagnostic.message, "The Thoth syntax is not valid.");
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    assert_eq!(
        diagnostic.range.start,
        Position {
            line: 1,
            character: 2
        }
    );
    assert_eq!(
        diagnostic.range.end,
        Position {
            line: 1,
            character: 3
        }
    );

    let valid = "function Valid(entity)\n  return MissingHelper(entity)\nend\n";
    let valid_document = overlay(&workspace, &path, valid)
        .get(&path)
        .unwrap()
        .clone();
    overlays.insert(
        path.clone(),
        OverlayDocument {
            version: 2,
            ..valid_document
        },
    );
    assert!(
        workspace
            .diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning))
            .is_empty(),
        "valid Thoth syntax must not produce unsupported semantic diagnostics"
    );

    overlays.remove(&path);
    assert!(
        workspace
            .diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning))
            .is_empty(),
        "clearing the overlay must restore the valid disk diagnostics"
    );
}

#[test]
fn supports_language_features_in_lsx_values_without_stats_diagnostics() {
    let (workspace, _) = fixture_workspace(200);
    let path = fixtures().join("project/Public/MyMod/Progressions/Progressions.lsx");
    let text = r#"<node id="Progression">
  <attribute id="Boosts" type="LSString" value="ActionResource(ActionPoint,1)"/>
  <attribute id="Name" type="LSString" value="UnsavedProgression"/>
  <attribute id="PassivesAdded" type="LSString" value="CHAINED"/>
  <attribute id="Selectors" type="LSString" value="SelectSpells(eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee,1,0)"/>
  <attribute id="UUID" type="guid" value="99999999-9999-9999-9999-999999999999"/>
</node>"#;
    let overlays = overlay(&workspace, &path, text);
    let line = text.lines().nth(1).unwrap();
    let cursor = line.find("ActionP").unwrap() + "ActionP".len();
    let position = Position {
        line: 1,
        character: u32::try_from(cursor).unwrap(),
    };

    let completion = workspace.completion(&path, position, &overlays, false);
    assert!(
        completion
            .items
            .iter()
            .any(|item| item.label == "ActionPoint")
    );
    let function_cursor = line.find("ActionR").unwrap() + "ActionR".len();
    let functions = workspace.completion(
        &path,
        Position {
            line: 1,
            character: u32::try_from(function_cursor).unwrap(),
        },
        &overlays,
        false,
    );
    assert!(
        functions
            .items
            .iter()
            .any(|item| item.label == "ActionResource")
    );
    let signature = workspace
        .signature_help(&path, position, &overlays)
        .unwrap();
    assert!(signature.label.starts_with("ActionResource(resource"));
    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 1,
                character: u32::try_from(line.find("ActionResource").unwrap()).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(hover.contains("**Stats function** `ActionResource`"));

    let selector_line = text.lines().nth(4).unwrap();
    let observed_hover = workspace
        .language_hover(
            &path,
            Position {
                line: 4,
                character: u32::try_from(selector_line.find("SelectSpells").unwrap()).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(observed_hover.contains("**Observed Thoth function** `SelectSpells`"));

    let passive_line = text.lines().nth(3).unwrap();
    let passive_column =
        u32::try_from(passive_line.find("CHAINED").unwrap() + "CHAINED".len() - 1).unwrap();
    let passive_completion_column =
        u32::try_from(passive_line.find("CHAI").unwrap() + "CHAI".len()).unwrap();
    let passives = workspace.completion(
        &path,
        Position {
            line: 3,
            character: passive_completion_column,
        },
        &overlays,
        false,
    );
    assert!(passives.items.iter().any(|item| item.label == "CHAINED"));
    let definitions = workspace.definitions_at(
        &path,
        Position {
            line: 3,
            character: passive_column,
        },
        &overlays,
    );
    assert_eq!(definitions.len(), 3);
    assert!(
        workspace
            .diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning))
            .is_empty()
    );
}

#[test]
fn hovers_typed_lsx_localization_handles_from_loose_and_packed_sources() {
    let (workspace, _) = fixture_workspace(200);
    let loose_handle = "h000000000000000000000000000000000001";
    let packed_handle = "h333333333333333333333333333333333333";
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [
            (loose_handle.into(), 1, "Shadowed packed text".into()),
            (
                packed_handle.into(),
                2,
                "Packed <LSTag Type=\"Status\">description</LSTag>".into(),
            ),
        ],
    )
    .unwrap();
    let workspace = workspace.with_base_localization(Arc::new(catalog));
    let path = fixtures().join("project/Public/MyMod/Progressions/ProgressionDescriptions.lsx");
    let text = format!(
        r#"<node id="ProgressionDescription">
  <attribute id="DisplayName" type="TranslatedString" handle="{loose_handle}" version="2" />
  <attribute id="Description" type="TranslatedString" handle="{packed_handle}" version="2" />
  <attribute id="TechnicalName" type="LSString" handle="{packed_handle}" />
</node>"#
    );
    let overlays = overlay(&workspace, &path, &text);

    let loose_hover = workspace
        .hover(&path, source_position(&text, loose_handle), &overlays)
        .unwrap();
    assert!(loose_hover.contains(&format!("**Localization** `{loose_handle}`")));
    assert!(loose_hover.contains("Test action & label"));
    assert!(!loose_hover.contains("Shadowed packed text"));

    let packed_hover = workspace
        .hover(&path, source_position(&text, packed_handle), &overlays)
        .unwrap();
    assert!(packed_hover.contains(&format!("**Localization** `{packed_handle}`")));
    assert!(packed_hover.contains("Packed description"));

    let ordinary_line = text.lines().nth(3).unwrap();
    let ordinary_position = Position {
        line: 3,
        character: u32::try_from(ordinary_line.find(packed_handle).unwrap()).unwrap(),
    };
    assert!(
        workspace
            .hover(&path, ordinary_position, &overlays)
            .is_none()
    );
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
fn hover_describes_curated_context_properties() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"TooltipDamageList\" \"DealDamage(MainMeleeWeapon-max(0,StrengthModifier),MainMeleeWeaponDamageType)\"";
    let overlays = overlay(&workspace, &path, text);

    let modifier = workspace
        .language_hover(&path, source_position(text, "StrengthModifier"), &overlays)
        .unwrap();
    assert!(modifier.contains("Context property"));
    assert!(modifier.contains("ability modifier"));

    let weapon = workspace
        .language_hover(&path, source_position(text, "MainMeleeWeapon"), &overlays)
        .unwrap();
    assert!(weapon.contains("Context property"));
    assert!(weapon.contains("main-hand melee weapon"));
}

#[test]
fn completion_offers_context_properties_in_value_positions() {
    let (workspace, path) = fixture_workspace(200);
    let text =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"TooltipDamageList\" \"DealDamage(Main\"";
    let overlays = overlay(&workspace, &path, text);
    let line = text.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };

    let completion = workspace.completion(&path, position, &overlays, false);
    let weapon = completion
        .items
        .iter()
        .find(|item| item.label == "MainMeleeWeapon")
        .expect("context property completion");
    assert_eq!(weapon.detail.as_deref(), Some("weapon"));
    assert!(
        completion
            .items
            .iter()
            .any(|item| item.label == "MainMeleeWeaponDamageType")
    );
}

#[test]
fn hover_describes_functor_execution_prefixes() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:DealDamage(MainMeleeWeapon,Fire);RemoveStatus(SELF,TEST_STATUS)\"";
    let overlays = overlay(&workspace, &path, text);

    let ground = workspace
        .language_hover(&path, source_position(text, "GROUND"), &overlays)
        .unwrap();
    assert!(ground.contains("Functor prefix"));
    assert!(ground.contains("`GROUND:`"));
    assert!(ground.contains("position selector"));

    let conditional_text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"AOE:IF(not Dead()):DealDamage(1d6,Fire)\"";
    let conditional_overlays = overlay(&workspace, &path, conditional_text);
    let conditional = workspace
        .language_hover(
            &path,
            source_position(conditional_text, "IF"),
            &conditional_overlays,
        )
        .unwrap();
    assert!(conditional.contains("Functor prefix"));
    assert!(conditional.contains("condition"));
}

#[test]
fn completion_offers_functor_prefixes_only_at_statement_starts() {
    let (workspace, path) = fixture_workspace(200);

    let statement_start = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"RemoveStatus(SELF,TEST_STATUS);GR";
    let overlays = overlay(&workspace, &path, statement_start);
    let line = statement_start.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &overlays, false);
    let ground = completion
        .items
        .iter()
        .find(|item| item.label == "GROUND")
        .expect("prefix completion after a top-level semicolon");
    assert_eq!(ground.detail.as_deref(), Some("position selector"));
    assert_eq!(ground.sort_text.as_deref(), Some("0GROUND"));

    let value_start = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"TARGET";
    let overlays = overlay(&workspace, &path, value_start);
    let line = value_start.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &overlays, false);
    assert!(completion.items.iter().any(|item| item.label == "TARGET"));

    let inside_call =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:ApplyStatus(\"";
    let overlays = overlay(&workspace, &path, inside_call);
    let line = inside_call.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &overlays, false);
    assert!(!completion.items.iter().any(|item| item.label == "GROUND"));

    let value_start_empty = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"";
    let overlays = overlay(&workspace, &path, value_start_empty);
    let line = value_start_empty.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &overlays, false);
    let apply_status = completion
        .items
        .iter()
        .find(|item| item.label == "ApplyStatus")
        .expect("curated function completion");
    assert_eq!(apply_status.sort_text.as_deref(), Some("0ApplyStatus"));
}

#[test]
fn hover_describes_legacy_stats_property_names() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:DealDamage(MainMeleeWeapon,X);RemoveStatus(SELF,Y)\"";
    let overlays = overlay(&workspace, &path, text);
    let name_column = text
        .lines()
        .nth(2)
        .unwrap()
        .find("SpellProperties")
        .unwrap();

    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(name_column + 5).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(
        hover.contains("**Stats property** `SpellProperties`"),
        "{}",
        hover
    );
    assert!(hover.contains("Types: `StatsFunctor`"), "{}", hover);
    assert!(
        hover.contains("grouped by execution position prefixes"),
        "{}",
        hover
    );
    assert!(hover.contains("```bg3_stats_value"), "{}", hover);
    assert!(
        hover.contains("GROUND:DealDamage(MainMeleeWeapon,X)\nRemoveStatus(SELF,Y)"),
        "{}",
        hover
    );

    // A plain value on a typed field gets types but no expression preview.
    let plain_text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"Name\" \"OncePerTurn\"";
    let plain_overlays = overlay(&workspace, &path, plain_text);
    let plain_name = plain_text.lines().nth(2).unwrap().find("Name").unwrap() + 1;
    let plain = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(plain_name).unwrap(),
            },
            &plain_overlays,
        )
        .unwrap();
    assert!(plain.contains("**Stats property** `Name`"), "{}", plain);
    assert!(!plain.contains("```bg3_stats_value"), "{}", plain);

    // An uncataloged field stays silent about documentation.
    let unknown_text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"MadeUpField\" \"abc\"";
    let unknown_overlays = overlay(&workspace, &path, unknown_text);
    let unknown_name = unknown_text.lines().nth(2).unwrap().find("MadeUp").unwrap() + 2;
    let unknown = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(unknown_name).unwrap(),
            },
            &unknown_overlays,
        )
        .unwrap();
    assert!(
        unknown.contains("**Stats property** `MadeUpField`"),
        "{}",
        unknown
    );
    assert!(!unknown.contains("Types:"), "{}", unknown);
}

#[test]
fn hover_and_completion_describe_curated_enum_values() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:ExecuteWeaponFunctors(MainHand);DealDamage(1d6,Fire)\"";
    let overlays = overlay(&workspace, &path, text);

    let hand = workspace
        .language_hover(&path, source_position(text, "MainHand"), &overlays)
        .unwrap();
    assert!(hand.contains("**Enum value** `MainHand`"), "{}", hand);
    assert!(
        hand.contains("Parameter: `eHandSlot`")
            && hand.contains("Function: `ExecuteWeaponFunctors`"),
        "{}",
        hand
    );

    let fire = workspace
        .language_hover(&path, source_position(text, "Fire"), &overlays)
        .unwrap();
    assert!(fire.contains("**Enum value** `Fire`"), "{}", fire);
    assert!(
        fire.contains("Parameter: `eDamageType`") && fire.contains("Function: `DealDamage`"),
        "{}",
        fire
    );

    // Completion offers exactly the domain of the argument under the cursor.
    let hand_text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"ExecuteWeaponFunctors(Main";
    let hand_overlays = overlay(&workspace, &path, hand_text);
    let line = hand_text.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &hand_overlays, false);
    let labels: Vec<_> = completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, vec!["MainHand"]);

    // A closed quote leaves an empty prefix, so the whole domain is offered.
    let damage_text =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"DealDamage(1d6,Fire\"";
    let damage_overlays = overlay(&workspace, &path, damage_text);
    let line = damage_text.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: u32::try_from(line.len()).unwrap(),
    };
    let completion = workspace.completion(&path, position, &damage_overlays, false);
    assert_eq!(completion.items.len(), 13);
    assert!(
        completion
            .items
            .iter()
            .any(|item| item.label == "Fire" && item.detail.as_deref() == Some("enum value"))
    );
}

#[test]
fn hover_and_completion_describe_stats_member_expressions() {
    let (workspace, path) = fixture_workspace(200);
    let text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellRoll\" \"Attack(AttackType.MeleeWeaponAttack)\"\ndata \"SpellProperties\" \"RemoveStatus(SELF,X);IF(not HasStatus('S',context.Source)):DealDamage(1d6,Fire)\"";
    let overlays = overlay(&workspace, &path, text);

    // Schema-enum object and value.
    let enumeration = workspace
        .language_hover(&path, source_position(text, "AttackType."), &overlays)
        .unwrap();
    assert!(
        enumeration.contains("**Enumeration** `AttackType`"),
        "{}",
        enumeration
    );
    assert!(
        enumeration.contains("Documented values: `3`"),
        "{}",
        enumeration
    );

    let value_column = text
        .lines()
        .nth(2)
        .unwrap()
        .find("MeleeWeaponAttack")
        .unwrap()
        + 2;
    let value = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(value_column).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(
        value.contains("**Enum value** `MeleeWeaponAttack`"),
        "{}",
        value
    );
    assert!(value.contains("Enumeration: `AttackType`"), "{}", value);

    // Context object and curated members.
    let context_column = text.lines().nth(3).unwrap().find("context").unwrap() + 1;
    let context = workspace
        .language_hover(
            &path,
            Position {
                line: 3,
                character: u32::try_from(context_column).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(
        context.contains("**Context object** `context`"),
        "{}",
        context
    );

    let source_column =
        text.lines().nth(3).unwrap().find("context.Source").unwrap() + "context.".len();
    let source = workspace
        .language_hover(
            &path,
            Position {
                line: 3,
                character: u32::try_from(source_column).unwrap(),
            },
            &overlays,
        )
        .unwrap();
    assert!(source.contains("**Context member** `Source`"), "{}", source);
    assert!(source.contains("caused this evaluation"), "{}", source);

    // Completion after the dots.
    let enum_text =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellRoll\" \"Attack(AttackType.M\"";
    let enum_overlays = overlay(&workspace, &path, enum_text);
    let line = enum_text.lines().nth(2).unwrap();
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: u32::try_from(line.len()).unwrap(),
        },
        &enum_overlays,
        false,
    );
    let labels: Vec<_> = completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec![
            "MeleeOffHandWeaponAttack",
            "MeleeWeaponAttack",
            "RangedWeaponAttack"
        ],
        "closed quote offers the whole domain"
    );
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.detail.as_deref() == Some("enum value"))
    );

    // Without the closing quote the prefix filters the domain.
    let enum_text =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellRoll\" \"Attack(AttackType.R";
    let enum_overlays = overlay(&workspace, &path, enum_text);
    let line = enum_text.lines().nth(2).unwrap();
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: u32::try_from(line.len()).unwrap(),
        },
        &enum_overlays,
        false,
    );
    let labels: Vec<_> = completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, vec!["RangedWeaponAttack"]);

    let context_text =
        "new entry \"TEST\"\ntype \"SpellData\"\ndata \"TargetConditions\" \"not context.S\"";
    let context_overlays = overlay(&workspace, &path, context_text);
    let line = context_text.lines().nth(2).unwrap();
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: u32::try_from(line.len()).unwrap(),
        },
        &context_overlays,
        false,
    );
    let labels: Vec<_> = completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert!(labels.contains(&"Source"), "{}", labels.join(","));
    assert!(labels.contains(&"StatusId"), "{}", labels.join(","));
}

fn packaged_spell(
    package: &str,
    priority: u8,
    name: &str,
    schema: &SchemaCatalog,
) -> PackagedStatsSource {
    let entry = format!("Public/Shared/Stats/Generated/Data/Spell_{name}.txt");
    let text =
        format!("new entry \"{name}\"\ntype \"SpellData\"\ndata \"UseCosts\" \"ActionPoint:1\"\n");
    PackagedStatsSource::new(
        "Shared",
        package,
        entry.clone(),
        priority,
        parse_source(
            SourceFile {
                path: entry.into(),
                kind: SourceKind::PlainStats,
            },
            &text,
            schema,
            "English",
        )
        .expect("synthetic packaged parse"),
    )
    .expect("synthetic packaged source")
}

#[test]
fn packaged_base_declarations_resolve_hover_and_stay_unnavigable() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = PackagedStatsCatalog::from_sources(vec![
        packaged_spell("a.pak", 0, "SPELL_PACKED", &workspace.schema),
        packaged_spell("b.pak", 0, "SPELL_TIED", &workspace.schema),
        packaged_spell("c.pak", 0, "SPELL_TIED", &workspace.schema),
    ])
    .expect("packaged catalog");
    let workspace = workspace.with_packaged_stats(Arc::new(catalog));

    let consumer_path = path.with_file_name("Passive_PACKED.txt");
    let text = "new entry \"CONSUMER\"\ntype \"PassiveData\"\ndata \"Boosts\" \"UseSpell(SPELL_PACKED);UseSpell(SPELL_TIED)\"\n";
    let overlays = overlay(&workspace, &consumer_path, text);

    let packed = workspace
        .definitions_at(
            &consumer_path,
            source_position(text, "SPELL_PACKED"),
            &overlays,
        )
        .pop()
        .expect("packed resolution");
    assert_eq!(packed.module, "Shared");
    assert_eq!(packed.rank, 0);
    assert_eq!(
        packed.packaged_entry.as_deref(),
        Some("Public/Shared/Stats/Generated/Data/Spell_SPELL_PACKED.txt")
    );
    assert!(!packed.ambiguous);

    let tied = workspace
        .definitions_at(
            &consumer_path,
            source_position(text, "SPELL_TIED"),
            &overlays,
        )
        .pop()
        .expect("tied resolution");
    assert!(tied.ambiguous);

    let hover = workspace
        .hover(
            &consumer_path,
            source_position(text, "SPELL_PACKED"),
            &overlays,
        )
        .expect("packaged hover");
    assert!(hover.contains("```bg3_stats"), "{hover}");
    assert!(hover.contains("new entry \"SPELL_PACKED\""), "{hover}");
    assert!(
        hover
            .contains("Package entry: `Public/Shared/Stats/Generated/Data/Spell_SPELL_PACKED.txt`"),
        "{hover}"
    );
    assert!(!hover.contains("- **UseCosts:**"), "{hover}");

    assert!(
        workspace
            .definition_locations_at(
                &consumer_path,
                source_position(text, "SPELL_PACKED"),
                &overlays
            )
            .is_empty()
    );
    let references = workspace.references_at(
        &consumer_path,
        source_position(text, "SPELL_PACKED"),
        true,
        &overlays,
    );
    assert!(
        references
            .iter()
            .all(|location| location.path == consumer_path)
    );
}

#[test]
fn project_overrides_beat_packaged_base_declarations() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = PackagedStatsCatalog::from_sources(vec![packaged_spell(
        "a.pak",
        0,
        "SPELL_PACKED",
        &workspace.schema,
    )])
    .expect("packaged catalog");
    let workspace = workspace.with_packaged_stats(Arc::new(catalog));

    let override_path = path.with_file_name("Spell_OVERRIDE.txt");
    let override_text = "new entry \"SPELL_PACKED\"\ntype \"SpellData\"\ndata \"UseCosts\" \"BonusActionPoint:2\"\n";
    let overlays = overlay(&workspace, &override_path, override_text);
    let definitions = workspace.definitions_at(
        &override_path,
        source_position(override_text, "SPELL_PACKED"),
        &overlays,
    );
    assert_eq!(definitions.len(), 2);
    assert_eq!(definitions[0].rank, 2);
    assert_eq!(definitions[0].packaged_entry, None);
    assert_eq!(definitions[1].rank, 0);
    assert!(definitions[1].packaged_entry.is_some());
    assert!(!definitions[0].ambiguous && !definitions[1].ambiguous);

    let locations = workspace.definition_locations_at(
        &override_path,
        source_position(override_text, "SPELL_PACKED"),
        &overlays,
    );
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].path, override_path);
}

#[test]
fn completion_includes_packaged_base_symbols() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = PackagedStatsCatalog::from_sources(vec![packaged_spell(
        "a.pak",
        0,
        "SPELL_PACKED",
        &workspace.schema,
    )])
    .expect("packaged catalog");
    let workspace = workspace.with_packaged_stats(Arc::new(catalog));

    let consumer_path = path.with_file_name("Passive_COMPLETE.txt");
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"UnlockSpell(\"\n";
    let overlays = overlay(&workspace, &consumer_path, text);
    let completion = workspace.completion(
        &consumer_path,
        Position {
            line: 2,
            character: u32::try_from(text.lines().nth(2).unwrap().len()).unwrap(),
        },
        &overlays,
        false,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "SPELL_PACKED")
        .expect("packaged completion");
    assert_eq!(item.detail.as_deref(), Some("Shared (packaged)"));
    assert_eq!(item.new_text, "SPELL_PACKED");
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

    assert!(hover.contains("```bg3_stats"));
    assert!(hover.contains("data \"Boosts\" \"ObjectSize(+2)\""));
    assert!(hover.contains("\n\n---\n\n### Game text preview"));
    assert!(hover.contains("**Test action & label**"), "{hover}");
    assert!(hover.contains("Synthetic description"));
    assert!(!hover.contains("Second line"));
    assert!(hover.contains("Description parameters: `Distance(3)`"));
    assert!(hover.contains("Game logic and UI formatting are not evaluated"));
    assert!(hover.contains("**Override chain**"));
}

#[test]
fn hover_reconstructs_stats_entries_with_order_clamps_and_comments() {
    let (workspace, path) = fixture_workspace(200);
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [
            (
                "h11111111111111111111111111111111".into(),
                1,
                "Main Hand Attack".into(),
            ),
            (
                "h22222222222222222222222222222222".into(),
                2,
                "Make a melee attack.<br>Second line".into(),
            ),
        ],
    )
    .unwrap();
    let workspace = workspace.with_base_localization(Arc::new(catalog));
    let spell_path = path.with_file_name("Spell_MAIN.txt");
    let long_properties =
        "GROUND:DealDamage(MainMeleeWeapon, MainMeleeWeaponDamageType);GROUND:ExecuteWeaponFunctors(MainHand);"
            .repeat(3);
    let text = format!(
        "new entry \"Target_Test\"\ntype \"SpellData\"\ndata \"UseCosts\" \"ActionPoint:1\"\ndata \"SpellProperties\" \"{long_properties}\"\ndata \"DisplayName\" \"h11111111111111111111111111111111;1\"\ndata \"Description\" \"h22222222222222222222222222222222;2\"\n"
    );
    let overlays = overlay(&workspace, &spell_path, &text);
    let hover = workspace
        .hover(
            &spell_path,
            Position {
                line: 0,
                character: 13,
            },
            &overlays,
        )
        .unwrap();

    assert!(hover.contains("```bg3_stats\n"), "{hover}");
    assert!(
        hover.contains("new entry \"Target_Test\"\ntype \"SpellData\""),
        "{hover}"
    );
    let use_costs = hover.find("data \"UseCosts\"").unwrap();
    let properties = hover.find("data \"SpellProperties\"").unwrap();
    let display_name = hover.find("data \"DisplayName\"").unwrap();
    assert!(
        use_costs < properties && properties < display_name,
        "{hover}"
    );
    assert!(
        hover.contains(
            "data \"SpellProperties\" \"GROUND:DealDamage(MainMeleeWeapon, MainMeleeWeaponDamageType);GROUND:ExecuteWeaponFunctors(MainHand);…\""
        ),
        "{hover}"
    );
    assert!(!hover.contains(&long_properties), "{hover}");
    assert!(
        hover.contains("// Main Hand Attack\ndata \"DisplayName\""),
        "{hover}"
    );
    assert!(
        hover.contains("// Make a melee attack.\n// Second line\ndata \"Description\""),
        "{hover}"
    );

    let animation_path = path.with_file_name("Spell_ANIMATED.txt");
    let animation_text = "new entry \"Animated\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"DealDamage(MainMeleeWeapon, MainMeleeWeaponDamageType)\"\ndata \"SpellAnimation\" \"8b8bb757-21ce-4e02-a2f3-97d55cf2f90b,,;6606c30b-be1c-4f17-ae6b-1a591c80b18c,366693ee-d97f-4294-a4dd-a2145ddc4e6a,9f2d32b9-529a-4b75-b3df-6e1ae1395280;\"\ndata \"CastEffect\" \"8682067a-e523-40fb-b705-3112083b6b05\"\n";
    let animation_overlays = overlay(&workspace, &animation_path, animation_text);
    let animation_hover = workspace
        .hover(
            &animation_path,
            Position {
                line: 0,
                character: 13,
            },
            &animation_overlays,
        )
        .unwrap();
    assert!(
        !animation_hover.contains("data \"SpellAnimation\""),
        "{animation_hover}"
    );
    assert!(
        !animation_hover.contains("data \"CastEffect\""),
        "{animation_hover}"
    );
    assert!(
        animation_hover.contains("// … 2 hidden presentation fields"),
        "{animation_hover}"
    );
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
fn hover_resolves_static_and_typed_localization_tooltips() {
    let (workspace, _) = fixture_workspace(200);
    let title = "h111111111111111111111111111111111111";
    let description = "h222222222222222222222222222222222222";
    let catalog = LocalizationCatalog::from_entries(
        "English",
        [
            (title.into(), 1, "Attack roll".into()),
            (
                description.into(),
                1,
                "Determines whether an attack hits.<br>Runtime detail".into(),
            ),
            (
                "h000000000000000000000000000000000001".into(),
                2,
                "Test action & label".into(),
            ),
            (
                "h000000000000000000000000000000000002".into(),
                1,
                "Synthetic description".into(),
            ),
        ],
    )
    .unwrap();
    let xaml = format!(
        r#"<ResourceDictionary xmlns:ls="synthetic"><Trigger Property="TagTooltip" Value="AttackRoll"><ls:LSTooltip Content="{description}" ls:AttachedProperties.InheritedTag="{title}"/></Trigger><Trigger Property="TagTooltip" Value="Dynamic"><ls:LSTooltip Content="{{Binding Runtime}}"/></Trigger></ResourceDictionary>"#
    );
    let tooltips = parse_tooltip_catalog(xaml.as_bytes()).unwrap();
    let workspace = workspace
        .with_base_localization(Arc::new(catalog))
        .with_tooltips(Arc::new(tooltips));
    let path = fixtures().join("project/Mods/MyMod/Localization/English/english.xml");
    let text = r#"<contentList><content contentuid="h333333333333333333333333333333333333" version="1">An &lt;LSTag Tooltip="AttackRoll">attack&lt;/LSTag&gt;, <LSTag Type="Passive" Tooltip="CONSUMER">passive</LSTag>, and <LSTag Tooltip="Dynamic">dynamic</LSTag>.</content></contentList>"#;
    let overlays = overlay(&workspace, &path, text);

    let glossary_hover = workspace
        .hover(&path, source_position(text, "AttackRoll"), &overlays)
        .unwrap();
    assert!(glossary_hover.contains("**Game tooltip** `AttackRoll`"));
    assert!(glossary_hover.contains("**Attack roll**"));
    assert!(glossary_hover.contains("Determines whether an attack hits."));
    assert!(glossary_hover.contains("Runtime detail"));

    let typed_hover = workspace
        .hover(&path, source_position(text, "CONSUMER"), &overlays)
        .unwrap();
    assert!(typed_hover.contains("**PassiveData** `CONSUMER`"));
    assert!(typed_hover.contains("**Test action & label**"));
    assert!(typed_hover.contains("Synthetic description"));

    assert!(
        workspace
            .hover(&path, source_position(text, "Dynamic"), &overlays)
            .is_none()
    );
    assert!(
        workspace
            .diagnostics(&path, &overlays, Some(DiagnosticSeverity::Warning))
            .is_empty()
    );
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
