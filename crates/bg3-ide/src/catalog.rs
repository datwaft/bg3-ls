/// One verified parameter in a curated Stats function signature.
#[derive(Clone, Copy, Debug)]
pub struct ParameterSpec {
    pub label: &'static str,
    pub kind: Option<&'static str>,
}

/// A curated function whose navigation and signature information is verified.
#[derive(Clone, Copy, Debug)]
pub struct FunctionSpec {
    pub name: &'static str,
    pub documentation: &'static str,
    pub parameters: &'static [ParameterSpec],
    pub variadic: bool,
}

const PASSIVE: &[ParameterSpec] = &[ParameterSpec {
    label: "passive",
    kind: Some("PassiveData"),
}];
const STATUS: &[ParameterSpec] = &[ParameterSpec {
    label: "status",
    kind: Some("StatusData"),
}];
const SPELL: &[ParameterSpec] = &[ParameterSpec {
    label: "spell",
    kind: Some("SpellData"),
}];
const INTERRUPT: &[ParameterSpec] = &[ParameterSpec {
    label: "interrupt",
    kind: Some("InterruptData"),
}];
const RESOURCE: &[ParameterSpec] = &[ParameterSpec {
    label: "resource",
    kind: Some("ActionResource"),
}];

/// Curated functions shared by navigation, completion, signatures, and diagnostics.
pub const FUNCTIONS: &[FunctionSpec] = &[
    function("AddPassive", "Adds a passive to the target.", PASSIVE),
    function("ApplyStatus", "Applies a status to the target.", STATUS),
    function(
        "ForceStatus",
        "Applies a status without the normal checks.",
        STATUS,
    ),
    function(
        "HasAnyStatus",
        "Tests whether the target has a specified status.",
        STATUS,
    ),
    function(
        "HasPassive",
        "Tests whether the target has a passive.",
        PASSIVE,
    ),
    function(
        "HasStatus",
        "Tests whether the target has a status.",
        STATUS,
    ),
    function(
        "IsImmuneToStatus",
        "Tests whether the target is immune to a status.",
        STATUS,
    ),
    function(
        "RemovePassive",
        "Removes a passive from the target.",
        PASSIVE,
    ),
    function("RemoveSpell", "Removes a spell from the target.", SPELL),
    function("RemoveStatus", "Removes a status from the target.", STATUS),
    function("UnlockInterrupt", "Unlocks an interrupt.", INTERRUPT),
    function("UnlockPassive", "Unlocks a passive.", PASSIVE),
    function("UnlockSpell", "Unlocks a spell.", SPELL),
    function("UseSpell", "Uses a spell.", SPELL),
    function("ActionResource", "Changes an action resource.", RESOURCE),
];

/// Constructs the common single-parameter variadic catalog entry.
const fn function(
    name: &'static str,
    documentation: &'static str,
    parameters: &'static [ParameterSpec],
) -> FunctionSpec {
    FunctionSpec {
        name,
        documentation,
        parameters,
        variadic: true,
    }
}

/// Finds a curated function by its exact name.
pub fn function_spec(name: &str) -> Option<&'static FunctionSpec> {
    FUNCTIONS.iter().find(|function| function.name == name)
}

/// Returns the typed symbol kind for one known field.
pub fn field_kind(name: &str) -> Option<&'static str> {
    match name {
        "ContainerSpells" | "Spells" => Some("SpellData"),
        "InterruptPrototype" => Some("InterruptData"),
        "Passives" | "PassivesAdded" | "PassivesOnEquip" => Some("PassiveData"),
        "PersonalStatusImmunities" | "StatusImmunities" | "StatusInInventory" | "StatusOnEquip" => {
            Some("StatusData")
        }
        _ => None,
    }
}
