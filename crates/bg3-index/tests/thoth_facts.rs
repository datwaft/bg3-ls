use bg3_index::{
    PackagedThothCatalog, PackagedThothResolution, PackagedThothSource, parse_packaged_thoth_facts,
    parse_thoth_file,
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
    assert_eq!(facts.records()[0].source().entry(), valid_entry);
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
