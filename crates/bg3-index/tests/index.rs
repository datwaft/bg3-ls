use std::fs;
use std::path::{Path, PathBuf};

use bg3_index::{
    CacheStore, ModuleIndex, ModuleRole, ModuleSpec, SchemaCatalog, SourceFile, SourceKind,
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
