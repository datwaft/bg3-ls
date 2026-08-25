use bg3_index::{
    context_member, context_members, context_properties, context_property, context_side,
    enum_value, field_documentation, function_spec, functor_prefix, functor_prefixes,
    member_enumeration, osiris_signature,
};

#[test]
fn preserves_the_original_engine_event_signatures() {
    let expected: &[(&str, u16, &[&str])] = &[
        ("AddedTo", 3, &["GUIDSTRING", "GUIDSTRING", "STRING"]),
        ("CharacterJoinedParty", 1, &["CHARACTER"]),
        ("CharacterLeftParty", 1, &["CHARACTER"]),
        ("CharacterLoadedInPreset", 1, &["CHARACTER"]),
        ("Died", 1, &["CHARACTER"]),
        ("Dying", 1, &["CHARACTER"]),
        ("EnteredCombat", 2, &["GUIDSTRING", "GUIDSTRING"]),
        ("LeftCombat", 2, &["GUIDSTRING", "GUIDSTRING"]),
        ("LevelGameplayStarted", 2, &["STRING", "INTEGER"]),
        ("RemovedFrom", 2, &["GUIDSTRING", "GUIDSTRING"]),
        (
            "TemplateAddedTo",
            4,
            &["ROOT", "GUIDSTRING", "GUIDSTRING", "STRING"],
        ),
        ("TextEvent", 1, &["STRING"]),
    ];
    for (name, arity, aliases) in expected {
        assert_eq!(
            osiris_signature(name, *arity),
            Some(*aliases),
            "{name}/{arity} drifted from the shipped signature"
        );
    }
}

#[test]
fn resolves_transcribed_engine_event_signatures() {
    // Spot checks transcribed from the generated reference, including the
    // primitive spellings that map to Osiris aliases.
    assert_eq!(
        osiris_signature("AttackedBy", 7),
        Some(
            &[
                "GUIDSTRING",
                "GUIDSTRING",
                "GUIDSTRING",
                "STRING",
                "INTEGER",
                "STRING",
                "INTEGER",
            ][..]
        )
    );
    assert_eq!(
        osiris_signature("DialogStarted", 2),
        Some(&["DIALOGRESOURCE", "INTEGER"][..])
    );
    assert_eq!(osiris_signature("LevelLoaded", 1), Some(&["STRING"][..]));
    assert_eq!(
        osiris_signature("LongRestStarted", 0),
        Some(&[][..]),
        "zero-parameter events stay resolvable"
    );

    // Arity is part of the key; wrong arities resolve to nothing so unknown
    // callables keep producing no evidence.
    assert_eq!(osiris_signature("Died", 2), None);
    assert_eq!(osiris_signature("NotAnEngineEvent", 1), None);
}

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

#[test]
fn finds_curated_functor_prefixes() {
    let ground = functor_prefix("GROUND").expect("attested position selector");
    assert_eq!(ground.kind, "position selector");
    assert_eq!(
        ground.documentation,
        "Runs the following functors at the ground position where the effect lands."
    );

    let conditional = functor_prefix("IF").expect("attested conditional");
    assert_eq!(conditional.kind, "conditional");

    assert_eq!(
        functor_prefix("AI_IGNORE").expect("AI flag").kind,
        "AI flag"
    );
    assert_eq!(
        functor_prefix("STATUS_HARD").expect("difficulty tier").kind,
        "difficulty tier"
    );
}

#[test]
fn rejects_unknown_functor_prefixes() {
    // Lowercase spellings stay uncataloged like every exact-name lookup.
    assert!(functor_prefix("ground").is_none());
    assert!(functor_prefix("NOT_A_PREFIX").is_none());
    assert!(functor_prefix("Movement").is_none());
}

#[test]
fn lists_every_functor_prefix() {
    assert_eq!(functor_prefixes().count(), 13);
}

#[test]
fn documents_attested_stats_properties() {
    let functors = field_documentation("SpellProperties").expect("attested property");
    assert!(functors.contains("execution position prefixes"));

    let targets = field_documentation("TargetConditions").expect("attested property");
    assert!(targets.contains("valid target"));

    // Only the curated set documents itself; nothing is invented.
    assert!(field_documentation("NotAProperty").is_none());
    assert!(field_documentation("").is_none());
}

#[test]
fn curates_weapon_and_damage_functors_with_enum_domains() {
    let deal_damage = function_spec("DealDamage").expect("attested functor");
    let damage_types = deal_damage
        .parameter_enum_values(1, 2, None)
        .expect("damage type domain");
    assert_eq!(damage_types.len(), 13);
    assert!(damage_types.contains(&"Fire"));
    assert!(deal_damage.parameter_enum_values(0, 2, None).is_none());

    let weapon_functors = function_spec("ExecuteWeaponFunctors").expect("attested functor");
    let hand_slots = weapon_functors
        .parameter_enum_values(0, 1, None)
        .expect("hand slot domain");
    assert_eq!(hand_slots, &["MainHand", "OffHand", "BothHands"]);
}

#[test]
fn resolves_enum_values_back_to_their_parameters() {
    let hand = enum_value("MainHand").expect("hand slot value");
    assert_eq!(hand.parameter, "eHandSlot");
    assert_eq!(hand.function, "ExecuteWeaponFunctors");

    let damage = enum_value("Fire").expect("damage type value");
    assert_eq!(damage.parameter, "eDamageType");
    assert_eq!(damage.function, "DealDamage");

    assert!(enum_value("NotAnEnumValue").is_none());
    assert!(enum_value("SELF").is_none());
}

#[test]
fn curates_member_enumerations_missing_from_the_toolkit_schema() {
    let attack_types = member_enumeration("AttackType").expect("attested attack types");
    assert_eq!(attack_types.len(), 8);
    assert!(attack_types.contains(&"MeleeWeaponAttack"));

    let damage_types = member_enumeration("DamageType").expect("damage types");
    assert_eq!(damage_types.len(), 13);

    assert!(member_enumeration("Ability").is_none());
    assert!(member_enumeration("NotAnEnumeration").is_none());
}

#[test]
fn documents_curated_context_members() {
    let source = context_member("Source").expect("attested context member");
    assert!(!source.function);
    assert!(source.documentation.contains("caused this evaluation"));

    let has_flag = context_member("HasContextFlag").expect("attested context function");
    assert!(has_flag.function);

    assert_eq!(context_members().count(), 8);
    assert!(context_member("NotAMember").is_none());

    let target = context_side("Target").expect("side selector");
    assert!(target.contains("from the target"));
    assert!(context_side("Source").is_none());
}
