use bg3_index::{context_properties, context_property};

#[test]
fn finds_documented_keywords_and_attested_weapon_data() {
    let weapon = context_property("MainMeleeWeapon").expect("attested weapon property");
    assert_eq!(weapon.kind, "weapon");

    let damage_type =
        context_property("MainMeleeWeaponDamageType").expect("attested damage type property");
    assert_eq!(damage_type.kind, "damage type");

    let level = context_property("Level").expect("documented keyword");
    assert_eq!(level.kind, "character level");
}

#[test]
fn expands_ability_and_skill_suffix_families() {
    let ability = context_property("Strength").expect("bare ability check");
    assert_eq!(ability.kind, "ability check");

    let modifier = context_property("StrengthModifier").expect("ability modifier");
    assert_eq!(modifier.kind, "ability modifier");

    let saving_throw = context_property("WisdomSavingThrow").expect("ability saving throw");
    assert_eq!(saving_throw.kind, "ability saving throw");

    let skill = context_property("Athletics").expect("bare skill check");
    assert_eq!(skill.kind, "skill check");

    let skill_modifier = context_property("PerceptionModifier").expect("skill modifier");
    assert_eq!(skill_modifier.kind, "skill modifier");

    let selector =
        context_property("UnarmedMeleeAbilitySavingThrow").expect("selector ability saving throw");
    assert_eq!(selector.kind, "unarmed melee ability saving throw");
}

#[test]
fn rejects_undocumented_and_unknown_names() {
    // Skills document only bare checks, `Flat`, and `Modifier`.
    assert!(context_property("AthleticsSavingThrow").is_none());
    assert!(context_property("SG_Charmed").is_none());
    assert!(context_property("MainMeleeWeaponDamage").is_none());
    assert!(context_property("NotAProperty").is_none());
}

#[test]
fn lists_every_catalog_entry_for_completion() {
    // 9 attested keywords plus the ability, selector, and skill families.
    let expected = 9 + 6 * 4 + 2 * 4 + 16 * 3;
    assert_eq!(context_properties().count(), expected);
    assert!(context_properties().any(|property| property.name == "SpellCastingAbilityModifier"));
}
