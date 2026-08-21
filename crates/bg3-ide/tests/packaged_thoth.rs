use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, PackagedThothCatalog, PackagedThothSource, Position,
    SchemaCatalog, SourceFile, SourceKind, parse_packaged_thoth_facts, parse_source,
};

fn source(
    module: &str,
    entry: &str,
    package: &str,
    priority: u8,
    text: &str,
) -> PackagedThothSource {
    PackagedThothSource::new(module, package, entry, priority, text).expect("synthetic source")
}

fn workspace() -> (WorkspaceSnapshot, PathBuf) {
    let schema = Arc::new(SchemaCatalog::default());
    let layers = [
        ModuleSpec {
            name: "Shared".into(),
            root: PathBuf::from("/synthetic/Shared"),
            role: ModuleRole::Base,
        },
        ModuleSpec {
            name: "OtherBase".into(),
            root: PathBuf::from("/synthetic/OtherBase"),
            role: ModuleRole::Base,
        },
        ModuleSpec {
            name: "Project".into(),
            root: PathBuf::from("/synthetic/Project"),
            role: ModuleRole::Project,
        },
    ]
    .into_iter()
    .map(|spec| Arc::new(ModuleIndex::new(spec, Vec::new())))
    .collect();
    (
        WorkspaceSnapshot::new(schema, layers, 1, 200, 200),
        PathBuf::from("/synthetic/Project/Stats/Test.txt"),
    )
}

fn overlay(workspace: &WorkspaceSnapshot, path: &Path, text: &str) -> OverlaySet {
    let parsed = parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::PlainStats,
        },
        text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic stats source");
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

#[test]
fn higher_configured_base_rank_wins_and_shadowed_package_is_not_hover_evidence() {
    let (workspace, path) = workspace();
    let shared_entry = "Mods/Shared/Scripts/thoth/helpers/Shared.khn";
    let extra_entry = "Mods/Shared/Scripts/thoth/helpers/Extra.khn";
    let other_entry = "Mods/OtherBase/Scripts/thoth/helpers/Other.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            source(
                "Shared",
                shared_entry,
                "Shared.pak",
                0,
                "function Shadowed(base) end\n",
            ),
            source(
                "Shared",
                shared_entry,
                "Patch1.pak",
                5,
                "function PatchOnly(value) end\n",
            ),
            source(
                "Shared",
                extra_entry,
                "Shared.pak",
                0,
                "function SharedExtra(value) end\nfunction Ranked(low) end\n",
            ),
            source(
                "OtherBase",
                other_entry,
                "OtherBase.pak",
                0,
                "function Ranked(high) end\n",
            ),
        ])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v1", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("facts"),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);

    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Pa";
    let overlays = overlay(&workspace, &path, text);
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 27,
        },
        &overlays,
        true,
    );
    assert!(
        completion
            .items
            .iter()
            .any(|item| item.label == "PatchOnly")
    );
    assert!(completion.items.iter().all(|item| item.label != "Shadowed"));

    let extra_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Sha";
    let extra_overlays = overlay(&workspace, &path, extra_text);
    let extra_completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 27,
        },
        &extra_overlays,
        true,
    );
    assert!(
        extra_completion
            .items
            .iter()
            .any(|item| item.label == "SharedExtra")
    );

    let ranked_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Ranked(value, ";
    let ranked_overlays = overlay(&workspace, &path, ranked_text);
    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: 2,
                character: u32::try_from(ranked_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &ranked_overlays,
        )
        .expect("installed signature");
    assert_eq!(signature.label, "Ranked(high)");

    let patch_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"PatchOnly";
    let patch_overlays = overlay(&workspace, &path, patch_text);
    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: 27,
            },
            &patch_overlays,
        )
        .expect("installed hover");
    assert!(hover.contains("PatchOnly"));
    assert!(hover.contains(shared_entry));
    assert!(!hover.contains("Shared.pak"));
    assert!(!hover.contains("Shadowed"));
}

#[test]
fn exposes_explicit_packaged_annotations_with_provenance() {
    let (workspace, path) = workspace();
    let entry = "Mods/Shared/Scripts/thoth/helpers/Annotated.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([source(
            "Shared",
            entry,
            "Shared.pak",
            0,
            "---@param value string\n---@return boolean\nfunction Annotated(value) end\n",
        )])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v2", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("facts"),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Annotated(value, \"";
    let overlays = overlay(&workspace, &path, text);

    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: 2,
                character: u32::try_from(text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &overlays,
        )
        .expect("packaged annotated signature");
    assert_eq!(signature.label, "Annotated(value: string): boolean");
    assert!(signature.documentation.contains(entry));

    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(text.lines().nth(2).unwrap().find("Annotated").unwrap())
                    .unwrap(),
            },
            &overlays,
        )
        .expect("packaged annotated hover");
    assert!(hover.contains("value: string"));
    assert!(hover.contains(entry));
    assert!(!hover.contains("/synthetic/"));

    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 48,
        },
        &overlays,
        false,
    );
    let item = completion
        .items
        .iter()
        .find(|item| item.label == "Annotated")
        .expect("packaged annotated completion");
    assert_eq!(
        item.detail.as_deref(),
        Some("Annotated(value: string): boolean (installed Shared)")
    );
}

#[test]
fn suppresses_conflicting_equal_priority_packaged_annotations() {
    let (workspace, path) = workspace();
    let first = "Mods/Shared/Scripts/thoth/helpers/First.khn";
    let second = "Mods/Shared/Scripts/thoth/helpers/Second.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            source(
                "Shared",
                first,
                "First.pak",
                0,
                "---@param value string\nfunction Conflicting(value) end\n",
            ),
            source(
                "Shared",
                second,
                "Second.pak",
                0,
                "---@param value number\nfunction Conflicting(value) end\n",
            ),
        ])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v2", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("facts"),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Conflicting(value\"";
    let overlays = overlay(&workspace, &path, text);
    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: 2,
                character: u32::try_from(text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &overlays,
        )
        .expect("ambiguous packaged signature");
    assert_eq!(signature.label, "Conflicting(value)");
    assert!(!signature.documentation.contains("Explicit"));
}

#[test]
fn suppresses_matching_equal_priority_packaged_contracts() {
    let (workspace, path) = workspace();
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            source(
                "Shared",
                "Mods/Shared/Scripts/thoth/helpers/First.khn",
                "First.pak",
                0,
                "---@return boolean\nfunction AmbiguousSame() end\n",
            ),
            source(
                "Shared",
                "Mods/Shared/Scripts/thoth/helpers/Second.khn",
                "Second.pak",
                0,
                "---@return boolean\nfunction AmbiguousSame() end\n",
            ),
        ])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v3", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("facts"),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"AmbiguousSame";
    let overlays = overlay(&workspace, &path, text);

    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(
                    text.lines()
                        .nth(2)
                        .expect("Boosts row")
                        .find("AmbiguousSame")
                        .expect("function name"),
                )
                .expect("position"),
            },
            &overlays,
        )
        .expect("untyped ambiguity evidence");
    assert!(hover.contains("AmbiguousSame"));
    assert!(!hover.contains("Returns: `boolean`"));
}

#[test]
fn unknown_observed_package_calls_keep_each_exact_arity() {
    let (workspace, path) = workspace();
    let entry = "Mods/OtherBase/Scripts/thoth/helpers/Observed.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([source(
            "OtherBase",
            entry,
            "OtherBase.pak",
            0,
            "function Caller()\n  ObservedNative(one)\n  ObservedNative(one, two)\nend\n",
        )])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v1", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("facts"),
    );
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);
    let text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ObservedN";
    let overlays = overlay(&workspace, &path, text);
    let completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 32,
        },
        &overlays,
        true,
    );
    let items: Vec<_> = completion
        .items
        .iter()
        .filter(|item| item.label == "ObservedNative")
        .collect();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|item| item.new_text == "ObservedNative(${1:unknown})")
    );
    assert!(
        items
            .iter()
            .any(|item| { item.new_text == "ObservedNative(${1:unknown}, ${2:unknown})" })
    );
    assert!(items.iter().all(|item| {
        item.documentation
            .as_deref()
            .is_some_and(|documentation| documentation.contains("types are not inferred"))
    }));
    assert!(items.iter().all(|item| {
        item.detail
            .as_deref()
            .is_some_and(|detail| !detail.contains("ambiguity"))
    }));

    let signature_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ObservedNative(one, ";
    let signature_overlays = overlay(&workspace, &path, signature_text);
    let signature = workspace
        .signature_help(
            &path,
            Position {
                line: 2,
                character: u32::try_from(signature_text.lines().nth(2).unwrap().len()).unwrap(),
            },
            &signature_overlays,
        )
        .expect("observed signature");
    assert_eq!(signature.label, "ObservedNative(unknown, unknown)");
    assert!(!signature.documentation.contains("ambiguity"));

    let hover = workspace
        .language_hover(
            &path,
            Position {
                line: 2,
                character: u32::try_from(
                    signature_text
                        .lines()
                        .nth(2)
                        .unwrap()
                        .find("ObservedNative")
                        .unwrap(),
                )
                .unwrap(),
            },
            &signature_overlays,
        )
        .expect("observed hover");
    assert!(hover.contains("Observed call arities: `1, 2`"));
}

#[test]
fn rejected_package_sources_still_control_priority_and_ambiguity() {
    let (workspace, path) = workspace();
    let shadowed_entry = "Mods/OtherBase/Scripts/thoth/helpers/Shadowed.khn";
    let ambiguous_entry = "Mods/OtherBase/Scripts/thoth/helpers/Ambiguous.khn";
    let catalog = Arc::new(
        PackagedThothCatalog::from_sources([
            source(
                "OtherBase",
                shadowed_entry,
                "OtherBase.pak",
                0,
                "function Hidden(lower) end\n",
            ),
            source(
                "OtherBase",
                shadowed_entry,
                "Patch1.pak",
                1,
                "function Broken(value)\n  @\nend\n",
            ),
            source(
                "OtherBase",
                ambiguous_entry,
                "Patch2.pak",
                2,
                "function Partial(value) end\n",
            ),
            source(
                "OtherBase",
                ambiguous_entry,
                "Patch3.pak",
                2,
                "function Broken(value)\n  @\nend\n",
            ),
        ])
        .expect("catalog"),
    );
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-v1", |source| {
            bg3_index::parse_thoth_file(source.text())
        })
        .expect("partial facts"),
    );
    assert_eq!(facts.rejected_count(), 2);
    let workspace = workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts);

    let hidden_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Hid\"";
    let hidden_overlays = overlay(&workspace, &path, hidden_text);
    let hidden_completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 24,
        },
        &hidden_overlays,
        true,
    );
    assert!(
        hidden_completion
            .items
            .iter()
            .all(|item| item.label != "Hidden")
    );

    let partial_text = "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Part\"";
    let partial_overlays = overlay(&workspace, &path, partial_text);
    let partial_completion = workspace.completion(
        &path,
        Position {
            line: 2,
            character: 25,
        },
        &partial_overlays,
        true,
    );
    let partial: Vec<_> = partial_completion
        .items
        .iter()
        .filter(|item| item.label == "Partial")
        .collect();
    assert_eq!(partial.len(), 1);
    assert!(
        partial[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("same-rank ambiguity"))
    );
}
