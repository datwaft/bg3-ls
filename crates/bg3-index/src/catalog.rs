/// One verified parameter in a curated Stats function signature.
#[derive(Clone, Copy, Debug)]
pub struct ParameterSpec {
    pub label: &'static str,
    pub kind: Option<&'static str>,
}

/// One verified call form for a curated Stats function.
#[derive(Clone, Copy, Debug)]
pub struct FunctionForm {
    pub parameters: &'static [ParameterSpec],
    pub variadic: bool,
}

/// A curated function whose semantic arguments and signatures are verified.
#[derive(Clone, Copy, Debug)]
pub struct FunctionSpec {
    pub name: &'static str,
    pub documentation: &'static str,
    pub default_form: FunctionForm,
    pub target_form: Option<FunctionForm>,
    pub target_min_arity: usize,
}

impl FunctionSpec {
    /// Selects the call form from its arity and optional first argument.
    ///
    /// Complete calls use arity. Incomplete editor calls also use known BG3
    /// context selectors so completion works before the closing parenthesis.
    pub fn form_for_call(
        &self,
        argument_count: usize,
        first_argument: Option<&str>,
    ) -> FunctionForm {
        if let Some(form) = self.target_form
            && (argument_count >= self.target_min_arity
                || first_argument.is_some_and(is_context_selector))
        {
            return form;
        }
        self.default_form
    }

    /// Returns the semantic kind for one parameter in the selected call form.
    pub fn parameter_kind(
        &self,
        index: usize,
        argument_count: usize,
        first_argument: Option<&str>,
    ) -> Option<&'static str> {
        self.form_for_call(argument_count, first_argument)
            .parameters
            .get(index)
            .and_then(|parameter| parameter.kind)
    }
}

const PASSIVE: FunctionForm = form(&[ParameterSpec {
    label: "passive",
    kind: Some("PassiveData"),
}]);
const STATUS: FunctionForm = form(&[ParameterSpec {
    label: "status",
    kind: Some("StatusData"),
}]);
const SPELL: FunctionForm = form(&[ParameterSpec {
    label: "spell",
    kind: Some("SpellData"),
}]);
const INTERRUPT: FunctionForm = form(&[ParameterSpec {
    label: "interrupt",
    kind: Some("InterruptData"),
}]);
const RESOURCE: FunctionForm = form(&[ParameterSpec {
    label: "resource",
    kind: Some("ActionResource"),
}]);
const TARGET_STATUS: FunctionForm = form(&[
    ParameterSpec {
        label: "target",
        kind: None,
    },
    ParameterSpec {
        label: "status",
        kind: Some("StatusData"),
    },
]);
const TARGET_SPELL: FunctionForm = form(&[
    ParameterSpec {
        label: "target",
        kind: None,
    },
    ParameterSpec {
        label: "spell",
        kind: Some("SpellData"),
    },
]);

/// Curated functions shared by parsing and editor language operations.
pub const FUNCTIONS: &[FunctionSpec] = &[
    function("AddPassive", "Adds a passive to the target.", PASSIVE),
    targeted_function(
        "ApplyStatus",
        "Applies a status to the target.",
        STATUS,
        TARGET_STATUS,
        4,
    ),
    targeted_function(
        "ForceStatus",
        "Applies a status without the normal checks.",
        STATUS,
        TARGET_STATUS,
        4,
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
    targeted_function("UseSpell", "Uses a spell.", SPELL, TARGET_SPELL, 5),
    function("ActionResource", "Changes an action resource.", RESOURCE),
];

/// Constructs the common variadic call form.
const fn form(parameters: &'static [ParameterSpec]) -> FunctionForm {
    FunctionForm {
        parameters,
        variadic: true,
    }
}

/// Constructs a function with one verified call form.
const fn function(
    name: &'static str,
    documentation: &'static str,
    default_form: FunctionForm,
) -> FunctionSpec {
    FunctionSpec {
        name,
        documentation,
        default_form,
        target_form: None,
        target_min_arity: usize::MAX,
    }
}

/// Constructs a function that also accepts an explicit target argument.
const fn targeted_function(
    name: &'static str,
    documentation: &'static str,
    default_form: FunctionForm,
    target_form: FunctionForm,
    target_min_arity: usize,
) -> FunctionSpec {
    FunctionSpec {
        name,
        documentation,
        default_form,
        target_form: Some(target_form),
        target_min_arity,
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

/// Tests identifiers that select an explicit functor target instead of a resource.
fn is_context_selector(value: &str) -> bool {
    let value = value.trim_matches(|character: char| {
        character.is_ascii_whitespace() || character == '\'' || character == '"'
    });
    value.starts_with("OBSERVER_")
        || matches!(
            value,
            "SWAP" | "SELF" | "OWNER" | "SOURCE" | "TARGET" | "CASTER" | "CAUSE"
        )
}
