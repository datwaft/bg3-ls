use std::fs;
use std::path::{Path, PathBuf};

use bg3_index::{
    CacheStore, ModuleIndex, ModuleRole, ModuleSpec, SchemaCatalog, SourceFile, SourceKind,
    SymbolTarget, THOTH_FUNCTION_KIND, discover_module, parse_source,
    read_base_localization_package, read_localization_package,
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

fn synthetic_loca(entries: &[(&str, u16, &str)]) -> Vec<u8> {
    let table_size = 12 + entries.len() * 70;
    let mut bytes = Vec::with_capacity(
        table_size
            + entries
                .iter()
                .map(|(_, _, text)| text.len() + 1)
                .sum::<usize>(),
    );
    bytes.extend_from_slice(b"LOCA");
    bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(table_size).unwrap().to_le_bytes());
    for (handle, version, text) in entries {
        let mut key = [0_u8; 64];
        key[..handle.len()].copy_from_slice(handle.as_bytes());
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(text.len() + 1).unwrap().to_le_bytes());
    }
    for (_, _, text) in entries {
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn synthetic_package(language: &str, loca: &[u8], compression: u8) -> Vec<u8> {
    let stored = match compression {
        0 => loca.to_vec(),
        2 => lz4_flex::block::compress(loca),
        _ => loca.to_vec(),
    };
    let mut entry = vec![0_u8; 272];
    let name = format!(
        "Localization/{language}/{}.loca",
        language.to_ascii_lowercase()
    );
    entry[..name.len()].copy_from_slice(name.as_bytes());
    entry[256..260].copy_from_slice(&40_u32.to_le_bytes());
    entry[263] = compression;
    entry[264..268].copy_from_slice(&u32::try_from(stored.len()).unwrap().to_le_bytes());
    let uncompressed = if compression == 0 { 0 } else { loca.len() };
    entry[268..272].copy_from_slice(&u32::try_from(uncompressed).unwrap().to_le_bytes());

    let compressed_list = lz4_flex::block::compress(&entry);
    let mut file_list = Vec::with_capacity(8 + compressed_list.len());
    file_list.extend_from_slice(&1_u32.to_le_bytes());
    file_list.extend_from_slice(&u32::try_from(compressed_list.len()).unwrap().to_le_bytes());
    file_list.extend_from_slice(&compressed_list);

    let file_list_offset = 40 + stored.len();
    let mut package = Vec::with_capacity(file_list_offset + file_list.len());
    package.extend_from_slice(b"LSPK");
    package.extend_from_slice(&18_u32.to_le_bytes());
    package.extend_from_slice(&u64::try_from(file_list_offset).unwrap().to_le_bytes());
    package.extend_from_slice(&u32::try_from(file_list.len()).unwrap().to_le_bytes());
    package.push(0);
    package.push(0);
    package.extend_from_slice(&[0_u8; 16]);
    package.extend_from_slice(&1_u16.to_le_bytes());
    package.extend_from_slice(&stored);
    package.extend_from_slice(&file_list);
    package
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
fn infers_legacy_schemas_from_type_discriminators() {
    let root = fixtures();
    let schema = load_schema(&root);
    let status_fields = std::collections::BTreeMap::from([("StatusType".into(), "BOOST".into())]);
    let status = schema.infer_legacy(Path::new("Status.txt"), Some("StatusData"), &status_fields);
    assert_eq!(
        status
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        ["Status_BOOST"]
    );

    let spell_fields = std::collections::BTreeMap::from([("SpellType".into(), "Target".into())]);
    let spell = schema.infer_legacy(Path::new("Spell.txt"), Some("SpellData"), &spell_fields);
    assert_eq!(
        spell
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>(),
        ["Spell_Target"]
    );
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
fn parses_thoth_declarations_parameters_and_calls() {
    let source = SourceFile {
        path: PathBuf::from("Mods/MyMod/Scripts/thoth/helpers/Test.khn"),
        kind: SourceKind::Thoth,
    };
    let parsed = parse_source(
        source,
        "function ProjectOnly(entity, fallback)\n  try\n    return DependencyOnly(entity)\n  catch error then\n    return fallback\n  end\nend\n",
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.definitions.len(), 1);
    assert_eq!(parsed.definitions[0].kind, THOTH_FUNCTION_KIND);
    assert_eq!(parsed.definitions[0].name, "ProjectOnly");
    assert_eq!(
        parsed.definitions[0].fields["Parameters"],
        "entity, fallback"
    );
    assert!(parsed.references.iter().any(|reference| {
        reference.target
            == SymbolTarget::Named {
                kind: Some(THOTH_FUNCTION_KIND.into()),
                name: "DependencyOnly".into(),
            }
    }));
    assert!(parsed.issues.is_empty());
}

#[test]
fn discovers_thoth_helpers_only_below_mod_script_trees() {
    let root = fixtures();
    let module = ModuleSpec {
        name: "MyMod".into(),
        root: root.join("project"),
        role: ModuleRole::Project,
    };
    let files = discover_module(&module, &root.join("game"), "English", false).unwrap();

    assert!(files.iter().any(|file| {
        file.kind == SourceKind::Thoth
            && file
                .path
                .ends_with("Mods/MyMod/Scripts/thoth/helpers/MyMod.khn")
    }));
    assert!(files.iter().all(|file| {
        file.kind != SourceKind::Thoth
            || file.path.to_string_lossy().contains("/Mods/")
                && file.path.to_string_lossy().contains("/Scripts/thoth/")
    }));
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
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Named {
                kind: Some("ActionResource".into()),
                name: "ActionPoint".into(),
            }
    }));
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Named {
                kind: Some("PassiveData".into()),
                name: "CHAINED".into(),
            }
    }));
    assert!(index.references.iter().any(|reference| {
        reference.reference().target
            == SymbolTarget::Uuid("dddddddd-dddd-dddd-dddd-dddddddddddd".parse().unwrap())
    }));
    assert!(index.functions.contains_key("SelectSpells"));
}

#[test]
fn reads_uncompressed_and_lz4_v18_localization_packages() {
    let handle = "h111111111111111111111111111111111111";
    let generated_handle = "hsynthetic_g_suffix_1";
    let loca = synthetic_loca(&[
        (handle, 7, "Synthetic <LSTag>preview</LSTag>"),
        (generated_handle, 3, "Generated handle"),
    ]);
    for compression in [0, 2] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("English.pak");
        fs::write(&path, synthetic_package("English", &loca, compression)).unwrap();

        let catalog = read_localization_package(&path, "English").unwrap();
        assert_eq!(catalog.language(), "English");
        assert_eq!(catalog.get(handle).unwrap().version, 7);
        assert_eq!(
            catalog.get(handle).unwrap().text,
            "Synthetic <LSTag>preview</LSTag>"
        );
        assert_eq!(
            catalog.get(generated_handle).unwrap().text,
            "Generated handle"
        );
    }
}

#[test]
fn keeps_the_last_duplicate_localization_handle() {
    let handle = "hsynthetic_g_suffix_1";
    let catalog = bg3_index::LocalizationCatalog::from_entries(
        "English",
        [
            (handle.into(), 1, "First".into()),
            (handle.into(), 2, "Second".into()),
        ],
    )
    .unwrap();

    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog.get(handle).unwrap().version, 2);
    assert_eq!(catalog.get(handle).unwrap().text, "Second");
}

#[test]
fn preserves_loose_localization_entities_and_cdata() {
    let source = SourceFile {
        path: PathBuf::from("Localization/English/english.xml"),
        kind: SourceKind::Localization,
    };
    let parsed = parse_source(
        source,
        r#"<contentList><content contentuid="h111111111111111111111111111111111111" version="1">A &amp; B&#33; <![CDATA[<tag>]]></content></contentList>"#,
        &SchemaCatalog::default(),
        "English",
    )
    .unwrap();

    assert_eq!(parsed.definitions[0].fields["Text"], "A & B! <tag>");
}

#[test]
fn rejects_unsafe_localization_package_variants() {
    let loca = synthetic_loca(&[("h111111111111111111111111111111111111", 1, "Synthetic")]);
    let directory = tempfile::tempdir().unwrap();

    let mut bad_signature = synthetic_package("English", &loca, 0);
    bad_signature[0] = b'X';
    let path = directory.path().join("bad-signature.pak");
    fs::write(&path, bad_signature).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut solid = synthetic_package("English", &loca, 0);
    solid[20] = 0x04;
    let path = directory.path().join("solid.pak");
    fs::write(&path, solid).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut multipart = synthetic_package("English", &loca, 0);
    multipart[38..40].copy_from_slice(&2_u16.to_le_bytes());
    let path = directory.path().join("multipart.pak");
    fs::write(&path, multipart).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let mut too_many = synthetic_package("English", &loca, 0);
    let file_list_offset =
        usize::try_from(u64::from_le_bytes(too_many[8..16].try_into().unwrap())).unwrap();
    too_many[file_list_offset..file_list_offset + 4].copy_from_slice(&100_001_u32.to_le_bytes());
    let path = directory.path().join("too-many.pak");
    fs::write(&path, too_many).unwrap();
    assert!(read_localization_package(&path, "English").is_err());

    let path = directory.path().join("unsupported-compression.pak");
    fs::write(&path, synthetic_package("English", &loca, 3)).unwrap();
    assert!(read_localization_package(&path, "English").is_err());
}

#[test]
fn caches_and_invalidates_the_base_localization_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let game = directory.path().join("game");
    let localization = game.join("Localization");
    fs::create_dir_all(&localization).unwrap();
    let package = localization.join("English.pak");
    let handle = "h111111111111111111111111111111111111";
    fs::write(
        &package,
        synthetic_package("English", &synthetic_loca(&[(handle, 1, "First")]), 0),
    )
    .unwrap();
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();

    let (first, first_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(!first_hit);
    assert_eq!(first.get(handle).unwrap().text, "First");
    let (_, second_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(second_hit);

    fs::write(
        &package,
        synthetic_package(
            "English",
            &synthetic_loca(&[(handle, 2, "Second value")]),
            0,
        ),
    )
    .unwrap();
    let (changed, changed_hit) = cache
        .load_base_localization(&game, "English")
        .unwrap()
        .unwrap();
    assert!(!changed_hit);
    assert_eq!(changed.get(handle).unwrap().text, "Second value");

    let missing = read_base_localization_package(directory.path(), "English").unwrap();
    assert!(missing.is_none());
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

#[test]
fn caches_and_rebuilds_changed_thoth_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let helper = root.join("Mods/Test/Scripts/thoth/helpers/Test.khn");
    fs::create_dir_all(helper.parent().unwrap()).unwrap();
    fs::write(&helper, "function First(value)\n  return value\nend\n").unwrap();
    let module = ModuleSpec {
        name: "Test".into(),
        root: root.clone(),
        role: ModuleRole::Project,
    };
    let cache = CacheStore::new(directory.path().join("cache")).unwrap();
    let schema = SchemaCatalog::default();

    let sources = discover_module(&module, directory.path(), "English", false).unwrap();
    let (_, cold) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    let (_, warm) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    assert_eq!(cold.misses, 1);
    assert_eq!(warm.hits, 1);

    fs::write(
        &helper,
        "function Changed(value, fallback)\n  return value or fallback\nend\n",
    )
    .unwrap();
    let (changed, changed_stats) = cache
        .build_module(&module, &sources, &schema, "English")
        .unwrap();
    assert_eq!(changed_stats.misses, 1);
    assert_eq!(changed[0].definitions[0].name, "Changed");

    fs::remove_file(&helper).unwrap();
    let removed_sources = discover_module(&module, directory.path(), "English", false).unwrap();
    let (removed, _) = cache
        .build_module(&module, &removed_sources, &schema, "English")
        .unwrap();
    assert!(removed.is_empty());
}
