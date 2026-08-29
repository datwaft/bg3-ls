//! Curated, human-readable descriptions for engine Osiris callables.
//!
//! `story_header.div` describes callable shape, but it does not document
//! behavior.  Keep behavioral text in a separate small overlay so generated
//! contract data remains reproducible and the overlay can grow independently.
//!
//! Descriptions are concise paraphrases reviewed against the official BG3
//! Modding documentation at `https://docs.baldursgate3.game/`. Do not infer
//! descriptions from callable names or apply one across unverified overloads.

/// Returns a concise description for a known engine callable.
///
/// The lookup is intentionally keyed by both name and arity.  Osiris permits
/// overloaded callable names, and a description must never be applied to a
/// different overload by accident.
pub fn osiris_callable_description(name: &str, arity: u16) -> Option<&'static str> {
    match (name, arity) {
        // https://docs.baldursgate3.game/index.php?title=GetActionResourceValuePersonal
        ("GetActionResourceValuePersonal", 4) => Some(
            "Returns a character's value for the named action resource at the requested resource level.",
        ),
        // https://docs.baldursgate3.game/index.php?title=CastedSpell
        ("CastedSpell", 5) => Some(
            "Event raised after a spell is cast, with the caster, spell, spell type, spell element, and story action ID.",
        ),
        // https://docs.baldursgate3.game/index.php?title=UsingSpell
        ("UsingSpell", 5) => Some(
            "Event raised when a character begins using a spell, with the caster, spell, spell type, spell element, and story action ID.",
        ),
        // https://docs.baldursgate3.game/index.php?title=Exists
        ("Exists", 2) => Some("Tests whether the specified object exists."),
        // https://docs.baldursgate3.game/index.php?title=HasPassive
        ("HasPassive", 3) => Some(
            "Reports whether the specified entity has the named passive (0 for false, 1 for true).",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::osiris_callable_description;

    #[test]
    fn returns_descriptions_for_curated_callables() {
        assert!(
            osiris_callable_description("GetActionResourceValuePersonal", 4)
                .expect("description")
                .contains("action resource")
        );
        assert!(
            osiris_callable_description("CastedSpell", 5)
                .expect("description")
                .contains("spell is cast")
        );
        assert!(
            osiris_callable_description("UsingSpell", 5)
                .expect("description")
                .contains("begins using")
        );
        assert!(
            osiris_callable_description("Exists", 2)
                .expect("description")
                .contains("exists")
        );
        assert!(
            osiris_callable_description("HasPassive", 3)
                .expect("description")
                .contains("named passive")
        );
    }

    #[test]
    fn does_not_apply_description_to_another_arity() {
        assert_eq!(
            osiris_callable_description("GetActionResourceValuePersonal", 3),
            None
        );
        assert_eq!(osiris_callable_description("Missing", 4), None);
        assert_eq!(osiris_callable_description("HasPassive", 2), None);
    }
}
