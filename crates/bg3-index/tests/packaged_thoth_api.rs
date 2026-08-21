use bg3_index::{
    Error, PackagedThothApiIndex, PackagedThothApiResolution, PackagedThothApiSymbol,
    PackagedThothApiSymbolKind, PackagedThothCatalog, PackagedThothSource,
    parse_packaged_thoth_facts, parse_thoth_file,
};

fn source(package: &str, priority: u8, entry: &str, text: &str) -> PackagedThothSource {
    PackagedThothSource::new(
        "Shared",
        format!("/synthetic/{package}"),
        entry,
        priority,
        text,
    )
    .expect("synthetic source")
}

fn api_index(sources: impl IntoIterator<Item = PackagedThothSource>) -> PackagedThothApiIndex {
    let catalog = PackagedThothCatalog::from_sources(sources).expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test-api-facts-v1", |source| {
        parse_thoth_file(source.text())
    })
    .expect("facts");
    PackagedThothApiIndex::from_catalog_and_facts(&catalog, &facts)
}

#[test]
fn rejected_higher_priority_entry_suppresses_lower_api_evidence() {
    let catalog = PackagedThothCatalog::from_sources([
        source(
            "Shared.pak",
            0,
            "Mods/Shared/Scripts/thoth/helpers/Masked.khn",
            "function Masked() end\n",
        ),
        source(
            "Patch1.pak",
            1,
            "Mods/Shared/Scripts/thoth/helpers/Masked.khn",
            "function Masked() end\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test-api-facts-v1", |source| {
        if source.priority() == 1 {
            return Err(Error::Parse("synthetic rejected source".into()));
        }
        parse_thoth_file(source.text())
    })
    .expect("facts");
    let index = PackagedThothApiIndex::from_catalog_and_facts(&catalog, &facts);

    assert_eq!(
        index
            .candidates_for("Shared", PackagedThothApiSymbolKind::Function, "Masked")
            .len(),
        1
    );
    assert!(matches!(
        index.resolve("Shared", PackagedThothApiSymbolKind::Function, "Masked"),
        PackagedThothApiResolution::Missing
    ));
}

#[test]
fn attaches_a_function_contract_to_its_exact_declaration() {
    let index = api_index([source(
        "Shared.pak",
        0,
        "Mods/Shared/Scripts/thoth/helpers/Duplicate.khn",
        "function Duplicate() end\n---@return integer\nfunction Duplicate() end\n",
    )]);

    let candidates =
        index.candidates_for("Shared", PackagedThothApiSymbolKind::Function, "Duplicate");
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.symbol(),
                    PackagedThothApiSymbol::Function {
                        annotation: Some(_),
                        ..
                    }
                )
            })
            .count(),
        1
    );
}

#[test]
fn indexes_source_backed_symbols_with_priority_and_ambiguity() {
    let sources = vec![
        source(
            "Shared.pak",
            0,
            "Mods/Shared/Scripts/thoth/helpers/Resolve.khn",
            "function ResolveWeapon(weapon) end\n",
        ),
        source(
            "Patch1.pak",
            5,
            "Mods/Shared/Scripts/thoth/helpers/Resolve.khn",
            "---@alias WeaponId integer\n---@class Weapon\n---@field id integer\n---@param weapon Weapon\n---@return WeaponId\nfunction ResolveWeapon(weapon) end\nfunction PatchOnly(value) end\n",
        ),
        source(
            "TieA.pak",
            3,
            "Mods/Shared/Scripts/thoth/helpers/Ambiguous.khn",
            "function Ambiguous(value) end\n",
        ),
        source(
            "TieB.pak",
            3,
            "Mods/Shared/Scripts/thoth/helpers/AmbiguousB.khn",
            "function Ambiguous(value) end\n",
        ),
    ];
    let index = api_index(sources.clone());

    assert_eq!(index.len(), 7);
    assert_eq!(
        index
            .candidates_for(
                "Shared",
                PackagedThothApiSymbolKind::Function,
                "ResolveWeapon"
            )
            .len(),
        2
    );
    let PackagedThothApiResolution::Unique(resolve_weapon) = index.resolve(
        "Shared",
        PackagedThothApiSymbolKind::Function,
        "ResolveWeapon",
    ) else {
        panic!("priority should resolve ResolveWeapon uniquely");
    };
    assert_eq!(resolve_weapon.source().priority(), 5);
    assert_eq!(
        resolve_weapon.source().entry(),
        "Mods/Shared/Scripts/thoth/helpers/Resolve.khn"
    );
    assert!(matches!(
        resolve_weapon.symbol(),
        PackagedThothApiSymbol::Function {
            annotation: Some(annotation),
            ..
        } if annotation.contracts.len() == 1
    ));

    let PackagedThothApiResolution::Unique(class) =
        index.resolve("Shared", PackagedThothApiSymbolKind::Class, "Weapon")
    else {
        panic!("class should resolve uniquely");
    };
    assert!(matches!(
        class.symbol(),
        PackagedThothApiSymbol::Class(class) if class.fields.len() == 1 && class.fields[0].name == "id"
    ));
    assert!(matches!(
        index.resolve("Shared", PackagedThothApiSymbolKind::Alias, "WeaponId"),
        PackagedThothApiResolution::Unique(_)
    ));
    assert!(matches!(
        index.resolve("Shared", PackagedThothApiSymbolKind::Function, "Ambiguous"),
        PackagedThothApiResolution::Ambiguous(candidates) if candidates.len() == 2
    ));
    assert!(matches!(
        index.resolve("Shared", PackagedThothApiSymbolKind::Function, "Missing"),
        PackagedThothApiResolution::Missing
    ));

    let reordered = api_index(sources.into_iter().rev());
    assert_eq!(reordered, index);
}
