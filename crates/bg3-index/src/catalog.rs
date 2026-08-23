use std::collections::BTreeMap;
use std::sync::OnceLock;

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

/// One curated functor statement prefix.
#[derive(Clone, Copy, Debug)]
pub struct FunctorPrefixSpec {
    pub name: &'static str,
    pub kind: &'static str,
    pub documentation: &'static str,
}

/// Functor execution-position prefixes attested as vanilla statement heads.
///
/// Every entry appears before a `:` at the head of one or more installed
/// base-module functor statements. Prefixes compose, for example
/// `AOE:IF(not SavingThrow(...)):DealDamage(...)`.
const FUNCTOR_PREFIXES: &[FunctorPrefixSpec] = &[
    prefix(
        "GROUND",
        "position selector",
        "Runs the following functors at the ground position where the effect lands.",
    ),
    prefix(
        "TARGET",
        "position selector",
        "Runs the following functors against the spell target.",
    ),
    prefix(
        "SELF",
        "position selector",
        "Runs the following functors on the caster.",
    ),
    prefix(
        "CAST",
        "position selector",
        "Runs the following functors on the caster.",
    ),
    prefix(
        "AOE",
        "position selector",
        "Runs the following functors from the center of the spell area.",
    ),
    prefix(
        "PROJECTILE",
        "position selector",
        "Runs the following functors at the projectile position.",
    ),
    prefix(
        "AI_ONLY",
        "AI flag",
        "Runs the following functors only when AI controls the caster.",
    ),
    prefix(
        "AI_IGNORE",
        "AI flag",
        "Skips the following functors when AI controls the caster.",
    ),
    prefix(
        "STATUS_EASY",
        "difficulty tier",
        "Selects the functor variant used for the easy difficulty tier.",
    ),
    prefix(
        "STATUS_NORMAL",
        "difficulty tier",
        "Selects the functor variant used for the normal difficulty tier.",
    ),
    prefix(
        "STATUS_MEDIUM",
        "difficulty tier",
        "Selects the functor variant used for the medium difficulty tier.",
    ),
    prefix(
        "STATUS_HARD",
        "difficulty tier",
        "Selects the functor variant used for the hard difficulty tier.",
    ),
    prefix(
        "IF",
        "conditional",
        "Runs the following functor only when the Stats condition evaluates to true.",
    ),
];

const fn prefix(
    name: &'static str,
    kind: &'static str,
    documentation: &'static str,
) -> FunctorPrefixSpec {
    FunctorPrefixSpec {
        name,
        kind,
        documentation,
    }
}

/// Documentation for well-understood legacy Stats property names.
///
/// Every entry is attested inside installed base modules and described from
/// the public modding documentation. Names outside the table stay silent
/// about documentation and surface schema types only.
const FIELD_DOCUMENTATION: &[(&str, &str)] = &[
    (
        "DisplayName",
        "Localized name shown in tooltips and the UI.",
    ),
    (
        "Description",
        "Localized long description shown in tooltips.",
    ),
    (
        "ExtraDescription",
        "Additional localized paragraph appended to the description.",
    ),
    (
        "DescriptionParams",
        "Named values substituted into Description placeholders.",
    ),
    (
        "SpellType",
        "Which spell behaviour template the entry uses.",
    ),
    (
        "StatusType",
        "Which engine behaviour template the status uses, such as BOOST.",
    ),
    ("SpellSchool", "The school of magic used for rolls and DC."),
    (
        "SpellCastingAbility",
        "The ability that backs this spell's rolls and save DC.",
    ),
    (
        "SpellFlags",
        "Behaviour flags for the spell, like IsSpell or HasVerbalIntent.",
    ),
    (
        "VerbalIntent",
        "Spoken-line intent the caster performs while casting.",
    ),
    (
        "AIFlags",
        "Flags steering AI use of this spell, like CanNotUse.",
    ),
    ("PowerLevel", "Power tier used for scaling and upcasting."),
    ("Level", "Character level requirement or reference level."),
    (
        "Cooldown",
        "When the entry becomes available again after use.",
    ),
    (
        "UseCosts",
        "Action resources consumed on use, like ActionPoint:1.",
    ),
    (
        "DualWieldingUseCosts",
        "Action resources consumed when cast while dual wielding.",
    ),
    ("Icon", "UI icon shown for this entry."),
    (
        "SpellProperties",
        "Functors that run when the spell resolves, grouped by execution position prefixes.",
    ),
    ("StatsFunctors", "Status functors attached to this entry."),
    (
        "OnApplyFunctors",
        "Functors that run when the status is applied.",
    ),
    (
        "OnRemoveFunctors",
        "Functors that run when the status is removed.",
    ),
    (
        "SpellRoll",
        "The roll made when casting, such as an attack roll.",
    ),
    ("SpellSuccess", "Functors that run when the roll succeeds."),
    ("SpellFail", "Functors that run when the roll fails."),
    (
        "TooltipDamageList",
        "Damage entries shown in the tooltip, evaluated as Stats expressions.",
    ),
    (
        "TooltipStatusApply",
        "Tooltip text describing applied statuses.",
    ),
    (
        "TooltipAttackSave",
        "Tooltip text describing attack or save rolls.",
    ),
    (
        "RequirementConditions",
        "Conditions that must pass to select or use this entry.",
    ),
    (
        "RequirementEvents",
        "Events that re-evaluate the requirement conditions, like OnEquip;OnUnequip.",
    ),
    (
        "TargetConditions",
        "Conditions an object must meet to be a valid target.",
    ),
    ("Conditions", "Conditions gating this entry's activation."),
    (
        "RemoveConditions",
        "Conditions that remove the status when they stop passing.",
    ),
    (
        "RemoveEvents",
        "Events that re-evaluate the remove conditions.",
    ),
    ("TargetRadius", "Radius used to collect targets."),
    ("AreaRadius", "Radius of the spell's area of effect."),
    ("AuraRadius", "Radius of the aura emitted by this status."),
    (
        "AuraStatuses",
        "Statuses applied by the aura to affected characters.",
    ),
    (
        "Boosts",
        "Boosts applied while the status is active or item equipped.",
    ),
    (
        "StatusGroups",
        "Status groups this status belongs to, like SG_Charmed.",
    ),
    (
        "StatusPropertyFlags",
        "Flags controlling status behaviour, like ConsumingOnRoll.",
    ),
    (
        "StatusEffect",
        "Visual effect played while the status is active.",
    ),
    (
        "StackId",
        "Identifier grouping statuses or entries that stack together.",
    ),
    (
        "StackPriority",
        "Resolution order among entries sharing a StackId.",
    ),
    (
        "Stacking",
        "How repeated applications of the status combine.",
    ),
    (
        "Properties",
        "Passive flags granted by this entry, like Boost or ExtraAttack.",
    ),
    ("Passives", "Passives granted by this entry."),
    (
        "PassivesAdded",
        "Passives granted while this status is active.",
    ),
    (
        "PassivesOnEquip",
        "Passives granted while this item is equipped.",
    ),
    (
        "PassivesRemoved",
        "Passives removed while this entry is active.",
    ),
    ("Vitality", "Base vitality hit points of the character."),
    ("Rarity", "Item rarity tier."),
    ("Weight", "Item weight."),
];

/// Finds curated documentation for one legacy Stats property name.
pub fn field_documentation(name: &str) -> Option<&'static str> {
    FIELD_DOCUMENTATION
        .iter()
        .find(|candidate| candidate.0 == name)
        .map(|(_, documentation)| *documentation)
}

/// Finds a curated functor statement prefix by its exact name.
///
/// Names outside the catalog stay unreported because new engine prefixes can
/// appear between game patches.
pub fn functor_prefix(name: &str) -> Option<&'static FunctorPrefixSpec> {
    FUNCTOR_PREFIXES
        .iter()
        .find(|candidate| candidate.name == name)
}

/// Returns every curated functor statement prefix.
pub fn functor_prefixes() -> impl Iterator<Item = &'static FunctorPrefixSpec> {
    FUNCTOR_PREFIXES.iter()
}

/// Weapon context data attested inside vanilla functor arguments.
#[derive(Clone, Debug)]
pub struct ContextPropertySpec {
    pub name: String,
    pub kind: String,
    pub documentation: String,
}

/// Weapon context data attested inside vanilla functor arguments.
const CONTEXT_PROPERTIES: &[(&str, &str, &str)] = &[
    (
        "MainMeleeWeapon",
        "weapon",
        "The main-hand melee weapon of the context owner.",
    ),
    (
        "MainMeleeWeaponDamageType",
        "damage type",
        "The damage type of the context owner's main-hand melee weapon.",
    ),
    (
        "ProficiencyBonus",
        "proficiency bonus",
        "The proficiency bonus in the current context.",
    ),
    (
        "Level",
        "character level",
        "The character level in the current context.",
    ),
    (
        "MaxHP",
        "maximum hit points",
        "The maximum hit points of the context owner.",
    ),
    (
        "SpellDC",
        "difficulty class",
        "The spell save difficulty class in the current context.",
    ),
    (
        "WeaponActionDC",
        "difficulty class",
        "The weapon action difficulty class in the current context.",
    ),
    (
        "LockDC",
        "difficulty class",
        "The lockpicking difficulty class in the current context.",
    ),
    (
        "ClassLevel",
        "class level",
        "The level of one class of the context owner, for example `ClassLevel(Wizard)`.",
    ),
];

/// Abilities accept the bare check plus `Flat`, `Modifier`, and `SavingThrow`.
const ABILITY_BASES: &[&str] = &[
    "Strength",
    "Dexterity",
    "Constitution",
    "Intelligence",
    "Wisdom",
    "Charisma",
];

/// Skills accept the bare check plus `Flat` and `Modifier`.
const SKILL_BASES: &[&str] = &[
    "Athletics",
    "Acrobatics",
    "SleightOfHand",
    "Arcana",
    "History",
    "Investigation",
    "Nature",
    "Religion",
    "Perception",
    "Survival",
    "Deception",
    "Intimidation",
    "Performance",
    "Persuasion",
    "Medicine",
    "AnimalHandling",
];

/// Selector abilities behave like abilities for their suffix forms.
const SELECTOR_BASES: &[(&str, &str)] = &[
    ("SpellCastingAbility", "spellcasting ability"),
    ("UnarmedMeleeAbility", "unarmed melee ability"),
];

/// Finds a curated built-in context property by its exact name.
///
/// The catalog combines the documented Stats expression keywords with weapon
/// data attested inside installed base-module functor arguments. Names outside
/// the catalog stay unreported because the engine vocabulary is not fully
/// discoverable.
pub fn context_property(name: &str) -> Option<&'static ContextPropertySpec> {
    static CONTEXT_PROPERTY_INDEX: OnceLock<BTreeMap<String, ContextPropertySpec>> =
        OnceLock::new();
    CONTEXT_PROPERTY_INDEX
        .get_or_init(build_context_properties)
        .get(name)
}

/// Returns every curated built-in context property.
pub fn context_properties() -> impl Iterator<Item = &'static ContextPropertySpec> {
    static CONTEXT_PROPERTIES: OnceLock<BTreeMap<String, ContextPropertySpec>> = OnceLock::new();
    CONTEXT_PROPERTIES
        .get_or_init(build_context_properties)
        .values()
}

fn build_context_properties() -> BTreeMap<String, ContextPropertySpec> {
    let mut properties = BTreeMap::new();
    for (name, kind, documentation) in CONTEXT_PROPERTIES {
        insert_context_property(&mut properties, name, kind, documentation);
    }
    for base in ABILITY_BASES.iter().copied() {
        expand_context_property_family(&mut properties, base, "ability", true);
    }
    for (base, family) in SELECTOR_BASES.iter().copied() {
        expand_context_property_family(&mut properties, base, family, true);
    }
    for base in SKILL_BASES.iter().copied() {
        expand_context_property_family(&mut properties, base, "skill", false);
    }
    properties
}

fn expand_context_property_family(
    properties: &mut BTreeMap<String, ContextPropertySpec>,
    base: &str,
    family: &str,
    saving_throw: bool,
) {
    insert_context_property(
        properties,
        base,
        &format!("{family} check"),
        &format!("Resolves the {base} {family} check in the current context."),
    );
    insert_context_property(
        properties,
        &format!("{base}Flat"),
        &format!("{family} flat value"),
        &format!("Resolves {base} as a flat value."),
    );
    insert_context_property(
        properties,
        &format!("{base}Modifier"),
        &format!("{family} modifier"),
        &format!("Resolves the {base} modifier in the current context."),
    );
    if saving_throw {
        insert_context_property(
            properties,
            &format!("{base}SavingThrow"),
            &format!("{family} saving throw"),
            &format!("Resolves the {base} saving throw in the current context."),
        );
    }
}

fn insert_context_property(
    properties: &mut BTreeMap<String, ContextPropertySpec>,
    name: &str,
    kind: &str,
    documentation: &str,
) {
    properties.insert(
        name.to_string(),
        ContextPropertySpec {
            name: name.to_string(),
            kind: kind.to_string(),
            documentation: documentation.to_string(),
        },
    );
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
        "Passives" | "PassivesAdded" | "PassivesOnEquip" | "PassivesRemoved" => Some("PassiveData"),
        "PersonalStatusImmunities" | "StatusImmunities" | "StatusInInventory" | "StatusOnEquip" => {
            Some("StatusData")
        }
        _ => None,
    }
}

/// Returns whether an LSX attribute contains a supported Stats-value expression.
///
/// LSX has many free-form `LSString` fields. A conservative allowlist prevents
/// ordinary names and UI text from becoming speculative semantic references.
pub fn is_lsx_value_field(name: &str) -> bool {
    field_kind(name).is_some()
        || matches!(
            name,
            "Boosts" | "BoostsOnEquip" | "BoostsOnUnequip" | "Selectors"
        )
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
