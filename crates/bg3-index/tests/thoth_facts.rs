use bg3_index::{
    PackagedOsirisIndex, PackagedOsirisResolution, PackagedThothCatalog, PackagedThothResolution,
    PackagedThothSource, parse_osiris_goal_source, parse_packaged_thoth_facts, parse_thoth_file,
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

#[test]
fn package_priority_applies_only_to_one_module_and_entry() {
    let shared_entry = "Mods/Shared/Scripts/thoth/helpers/Shared.khn";
    let other_entry = "Mods/Shared/Scripts/thoth/helpers/Other.khn";
    let catalog = PackagedThothCatalog::from_sources([
        source("Shared", shared_entry, "Shared.pak", 0, "base"),
        source("Shared", shared_entry, "Patch1.pak", 5, "patch"),
        source("Shared", other_entry, "Other.pak", 0, "other"),
        source(
            "OtherBase",
            "Mods/OtherBase/Scripts/thoth/helpers/Shared.khn",
            "OtherBase.pak",
            9,
            "other module",
        ),
    ])
    .expect("catalog");

    assert!(matches!(
        catalog.resolve("Shared", shared_entry),
        PackagedThothResolution::Unique(source) if source.text() == "patch"
    ));
    assert!(matches!(
        catalog.resolve("Shared", other_entry),
        PackagedThothResolution::Unique(source) if source.text() == "other"
    ));
    assert!(matches!(
        catalog.resolve("OtherBase", "Mods/OtherBase/Scripts/thoth/helpers/Shared.khn"),
        PackagedThothResolution::Unique(source) if source.text() == "other module"
    ));
    assert!(matches!(
        catalog.resolve("Shared", "Mods/OtherBase/Scripts/thoth/helpers/Shared.khn"),
        PackagedThothResolution::Missing
    ));
}

#[test]
fn malformed_packaged_facts_are_rejected_without_discarding_valid_records() {
    let valid_entry = "Mods/Shared/Scripts/thoth/helpers/Valid.khn";
    let malformed_entry = "Mods/Shared/Scripts/thoth/helpers/Broken.khn";
    let catalog = PackagedThothCatalog::from_sources([
        source(
            "Shared",
            valid_entry,
            "Shared.pak",
            0,
            "function Valid(value) return value end\n",
        ),
        source(
            "Shared",
            malformed_entry,
            "Shared.pak",
            0,
            "function Broken(value)\n  @\nend\n",
        ),
    ])
    .expect("catalog");

    let facts = parse_packaged_thoth_facts(&catalog, "test-v1", |source| {
        parse_thoth_file(source.text())
    })
    .expect("one malformed package entry must not reject the complete catalog");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts.rejected_count(), 1);
    assert_eq!(facts.relevant_rejected_count(), 1);
    assert_eq!(facts.records()[0].source().entry(), valid_entry);
}

#[test]
fn lower_priority_rejections_do_not_mark_packaged_evidence_incomplete() {
    let entry = "Mods/Shared/Scripts/thoth/helpers/Shared.khn";
    let catalog = PackagedThothCatalog::from_sources([
        source("Shared", entry, "Shared.pak", 1, "valid"),
        source("Shared", entry, "Patch.pak", 0, "broken"),
    ])
    .expect("catalog");

    let facts = parse_packaged_thoth_facts(&catalog, "test-v1", |source| {
        if source.text() == "broken" {
            Err(bg3_index::Error::Parse("synthetic malformed source".into()))
        } else {
            Ok::<_, bg3_index::Error>(source.text().to_owned())
        }
    })
    .expect("lower-priority rejection must not fail the catalog");

    assert_eq!(facts.len(), 1);
    assert_eq!(facts.rejected_count(), 1);
    assert_eq!(facts.relevant_rejected_count(), 0);
}

#[test]
fn lower_priority_rejections_do_not_mark_ambiguous_packaged_evidence_incomplete() {
    let entry = "Mods/Shared/Scripts/thoth/helpers/Shared.khn";
    let catalog = PackagedThothCatalog::from_sources([
        source("Shared", entry, "PatchA.pak", 2, "valid-a"),
        source("Shared", entry, "PatchB.pak", 2, "valid-b"),
        source("Shared", entry, "PatchC.pak", 1, "broken"),
    ])
    .expect("catalog");

    let facts = parse_packaged_thoth_facts(&catalog, "test-v1", |source| {
        if source.text() == "broken" {
            Err(bg3_index::Error::Parse("synthetic malformed source".into()))
        } else {
            Ok::<_, bg3_index::Error>(source.text().to_owned())
        }
    })
    .expect("lower-priority rejection must not fail the catalog");

    assert_eq!(facts.len(), 2);
    assert_eq!(facts.rejected_count(), 1);
    assert_eq!(facts.relevant_rejected_count(), 0);
}

#[test]
fn rejected_candidates_at_an_ambiguous_priority_are_relevant() {
    let entry = "Mods/Shared/Scripts/thoth/helpers/Shared.khn";
    let catalog = PackagedThothCatalog::from_sources([
        source("Shared", entry, "PatchA.pak", 2, "broken-a"),
        source("Shared", entry, "PatchB.pak", 2, "broken-b"),
        source("Shared", entry, "PatchC.pak", 1, "valid"),
    ])
    .expect("catalog");

    let facts = parse_packaged_thoth_facts(&catalog, "test-v1", |source| {
        if source.text().starts_with("broken") {
            Err(bg3_index::Error::Parse("synthetic malformed source".into()))
        } else {
            Ok::<_, bg3_index::Error>(source.text().to_owned())
        }
    })
    .expect("malformed candidates must not fail the catalog");

    assert_eq!(facts.len(), 1);
    assert_eq!(facts.rejected_count(), 2);
    assert_eq!(facts.relevant_rejected_count(), 2);
}

#[test]
fn packaged_thoth_parsing_is_path_free_and_matches_direct_facts() {
    let entry = "Mods/Shared/Scripts/thoth/helpers/Virtual.khn";
    let text = "function Virtual(value)\n  return value, Namespace.Member\nend\n";
    let catalog = PackagedThothCatalog::from_sources([source(
        "Shared",
        entry,
        "/does/not/exist/virtual.pak",
        4,
        text,
    )])
    .expect("catalog");
    let direct = parse_thoth_file(text).expect("direct facts");
    let packaged = parse_packaged_thoth_facts(&catalog, "test-v2", |source| {
        parse_thoth_file(source.text())
    })
    .expect("packaged facts");
    assert_eq!(packaged.len(), 1);
    assert_eq!(packaged.records()[0].facts(), &direct);
    assert_eq!(packaged.records()[0].source().entry(), entry);
    assert_eq!(
        packaged.records()[0].source().package().to_string_lossy(),
        "/does/not/exist/virtual.pak"
    );
}

fn osiris_goal(body: &str) -> String {
    format!(
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\n{body}\nEXITSECTION\nENDEXITSECTION\n"
    )
}

#[test]
fn malformed_packaged_osiris_goals_are_rejected_without_discarding_valid_facts() {
    let valid_entry = "Mods/Shared/Story/RawFiles/Goals/Valid.txt";
    let malformed_entry = "Mods/Shared/Story/RawFiles/Goals/Broken.txt";
    let catalog = PackagedThothCatalog::from_sources([
        source(
            "Shared",
            valid_entry,
            "Shared.pak",
            0,
            &osiris_goal("PROC\nValid()\nTHEN\nDB_Noop(1);"),
        ),
        source(
            "Shared",
            malformed_entry,
            "Shared.pak",
            0,
            "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\nBroken(\nEXITSECTION\nENDEXITSECTION\n",
        ),
    ])
    .expect("catalog");

    let facts = parse_packaged_thoth_facts(&catalog, "test-osiris-v1", parse_osiris_goal_source)
        .expect("one malformed package entry must not reject the complete catalog");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts.rejected_count(), 1);
    assert_eq!(facts.relevant_rejected_count(), 1);
    assert_eq!(facts.records()[0].source().entry(), valid_entry);
    assert_eq!(
        facts.records()[0].facts().osiris.as_ref().unwrap().goal,
        "Valid"
    );
}

#[test]
fn syntax_invalid_complete_packaged_osiris_goals_are_rejected() {
    let source = source(
        "Shared",
        "Mods/Shared/Story/RawFiles/Goals/Broken.txt",
        "Shared.pak",
        0,
        &osiris_goal("PROC\nBroken(\nTHEN\nDB_Noop(1);"),
    );

    assert!(parse_osiris_goal_source(&source).is_err());
}

#[test]
fn standalone_packaged_osiris_signatures_are_rejected_as_goal_facts() {
    let source = source(
        "Shared",
        "Mods/Shared/Story/RawFiles/Goals/Signature.txt",
        "Shared.pak",
        0,
        "Example([in] INTEGER _Value)\n",
    );

    assert!(parse_osiris_goal_source(&source).is_err());
}

#[test]
fn rejected_higher_priority_osiris_goals_do_not_fall_back_to_lower_facts() {
    let entry = "Mods/Shared/Story/RawFiles/Goals/Callable.txt";
    let catalog = PackagedThothCatalog::from_sources([
        source(
            "Shared",
            entry,
            "Shared.pak",
            0,
            &osiris_goal("PROC\nCallable((CHARACTER)_Target)\nTHEN\nDB_Noop(1);"),
        ),
        source(
            "Shared",
            entry,
            "Patch.pak",
            1,
            "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nPROC\nCallable(\nEXITSECTION\nENDEXITSECTION\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test-osiris-v1", parse_osiris_goal_source)
        .expect("malformed candidates are counted, not fatal");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts.rejected_count(), 1);
    assert_eq!(facts.relevant_rejected_count(), 1);

    let index = PackagedOsirisIndex::from_catalog_and_facts(&catalog, &facts);
    assert!(matches!(
        index.resolve("Shared", "Callable", 1),
        PackagedOsirisResolution::Missing
    ));
}
