//! Synthetic integration coverage for Thoth expression type flow.
//!
//! The fixtures in this file are deliberately small.  The tests query exact
//! expression ranges so that a type inferred for one use cannot accidentally
//! be reused for another use of the same spelling.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, PackagedThothCatalog, PackagedThothSource, PrimitiveType,
    SchemaCatalog, SourceFile, SourceKind, TextRange, TypeExpression, parse_packaged_thoth_facts,
    parse_source, parse_thoth_file,
};

fn module(
    name: &str,
    root: &str,
    role: ModuleRole,
    files: &[(&str, SourceKind, &str)],
) -> Arc<ModuleIndex> {
    let schema = SchemaCatalog::default();
    let parsed = files
        .iter()
        .map(|(path, kind, text)| {
            parse_source(
                SourceFile {
                    path: PathBuf::from(path),
                    kind: *kind,
                },
                text,
                &schema,
                "English",
            )
            .expect("synthetic source")
        })
        .collect();
    Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: name.into(),
            root: root.into(),
            role,
        },
        parsed,
    ))
}

fn workspace(layers: Vec<Arc<ModuleIndex>>) -> WorkspaceSnapshot {
    WorkspaceSnapshot::new(Arc::new(SchemaCatalog::default()), layers, 1, 200, 200)
}

fn overlay(workspace: &WorkspaceSnapshot, path: &Path, module: &str, text: &str) -> OverlaySet {
    let mut overlays = OverlaySet::default();
    insert_overlay(workspace, &mut overlays, path, module, text);
    overlays
}

fn insert_overlay(
    workspace: &WorkspaceSnapshot,
    overlays: &mut OverlaySet,
    path: &Path,
    module: &str,
    text: &str,
) {
    let parsed = parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::Thoth,
        },
        text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic Thoth overlay");
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: module.into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
}

fn expression_range(text: &str, expression: &str) -> TextRange {
    let facts = parse_source(
        SourceFile {
            path: PathBuf::from("/synthetic/Query.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .expect("synthetic Thoth facts")
    .thoth
    .expect("Thoth facts");
    facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == expression)
        .map(|fact| fact.range)
        .unwrap_or_else(|| panic!("expression {expression:?} was not indexed"))
}

fn expression_range_at(text: &str, expression: &str, occurrence: usize) -> TextRange {
    let facts = parse_source(
        SourceFile {
            path: PathBuf::from("/synthetic/Query.khn"),
            kind: SourceKind::Thoth,
        },
        text,
        &SchemaCatalog::default(),
        "English",
    )
    .expect("synthetic Thoth facts")
    .thoth
    .expect("Thoth facts");
    facts
        .expression_facts
        .iter()
        .filter(|fact| fact.text == expression)
        .nth(occurrence)
        .map(|fact| fact.range)
        .unwrap_or_else(|| {
            panic!("expression {expression:?} occurrence {occurrence} was not indexed")
        })
}

fn expression_type(
    workspace: &WorkspaceSnapshot,
    path: &Path,
    text: &str,
    expression: &str,
    overlays: &OverlaySet,
) -> Option<TypeExpression> {
    workspace.thoth_expression_type(path, expression_range(text, expression), overlays)
}

fn expression_type_at(
    workspace: &WorkspaceSnapshot,
    path: &Path,
    text: &str,
    expression: &str,
    occurrence: usize,
    overlays: &OverlaySet,
) -> Option<TypeExpression> {
    workspace.thoth_expression_type(
        path,
        expression_range_at(text, expression, occurrence),
        overlays,
    )
}

fn packaged_workspace(
    workspace: WorkspaceSnapshot,
    sources: impl IntoIterator<Item = PackagedThothSource>,
) -> WorkspaceSnapshot {
    let catalog = Arc::new(PackagedThothCatalog::from_sources(sources).expect("catalog"));
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-thoth-flow", |source| {
            parse_thoth_file(source.text())
        })
        .expect("packaged facts"),
    );
    workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts)
}

#[test]
fn follows_annotated_helper_results_through_chained_assignments() {
    let path = Path::new("/synthetic/Project/Flow.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@return Weapon\nfunction make_weapon() end\nlocal first = make_weapon()\nlocal second = first\nlocal observed = second\nreturn observed\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "second", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "observed", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
}

#[test]
fn merges_incompatible_reachable_assignments_and_poisoning() {
    let path = Path::new("/synthetic/Project/Union.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@class Armor\n---@field armor_only boolean\nlocal value = nil\n---@type Weapon\nlocal weapon = nil\n---@type Armor\nlocal armor = nil\nvalue = weapon\nvalue = armor\nlocal unknown = MissingHelper()\nlocal poisoned = weapon + unknown\nreturn value, poisoned\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "value", 3, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Name("Armor".into()),
        ]))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "poisoned", 1, &overlays),
        None
    );
}

#[test]
fn infers_reachable_return_unions() {
    let path = Path::new("/synthetic/Project/Returns.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@class Armor\n---@field armor_only boolean\n---@type Weapon\nlocal weapon = nil\n---@type Armor\nlocal armor = nil\nfunction choose(flag)\n  if flag then\n    return weapon\n  end\n  return armor\nend\nlocal chosen = choose(true)\nreturn chosen\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "chosen", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Name("Armor".into()),
        ]))
    );
}

#[test]
fn inferred_helpers_include_implicit_nil_fallthrough() {
    let path = Path::new("/synthetic/Project/Fallthrough.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@type Weapon\nlocal weapon = nil\nfunction maybe(flag)\n  if flag then\n    return weapon\n  end\nend\nlocal result = maybe(true)\nreturn result\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "result", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Nil,
        ]))
    );
}

#[test]
fn exact_nil_guards_narrow_only_the_dominated_remainder() {
    let path = Path::new("/synthetic/Project/Guards.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@param guarded Weapon?\nfunction guarded(guarded)\n  if guarded == nil then return end\n  local after_guard = guarded\n  guarded = nil\n  local after_reassignment = guarded\n  return after_guard, after_reassignment\nend\n---@param branch Weapon?\nfunction branch(branch)\n  if branch ~= nil then\n    local inside = branch\n  end\n  local outside = branch\n  if branch == nil then log() end\n  local fallthrough = branch\n  return inside, outside, fallthrough\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "after_guard", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "after_reassignment", 1, &overlays),
        Some(TypeExpression::Nil)
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "branch", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "outside", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Nil,
        ]))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "fallthrough", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Nil,
        ]))
    );
}

#[test]
fn truthiness_and_compound_guards_do_not_over_narrow() {
    let path = Path::new("/synthetic/Project/Truthiness.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@param truthy_value Weapon?\n---@param compound_value Weapon?\n---@param remainder_value Weapon?\nfunction inspect(truthy_value, compound_value, remainder_value)\n  if truthy_value then\n    local truthy_result = truthy_value\n  end\n  if compound_value and compound_value.damage then\n    local compound_result = compound_value\n  end\n  local remainder_result = remainder_value\n  return truthy_result, compound_result, remainder_result\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    for (name, source_name) in [
        ("truthy_result", "truthy_value"),
        ("compound_result", "compound_value"),
        ("remainder_result", "remainder_value"),
    ] {
        assert_eq!(
            expression_type_at(
                &workspace,
                path,
                text,
                source_name,
                if source_name == "remainder_value" {
                    0
                } else {
                    1
                },
                &overlays,
            ),
            Some(TypeExpression::Union(vec![
                TypeExpression::Name("Weapon".into()),
                TypeExpression::Nil,
            ])),
            "truthiness guard narrowed {name}"
        );
    }
}

#[test]
fn nested_writes_poison_outer_flow_but_local_shadows_do_not() {
    let path = Path::new("/synthetic/Project/NestedWrites.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@class Armor\n---@field armor integer\n---@type Armor\nlocal armor = nil\n---@param changed Weapon\nfunction changed(changed, flag)\n  if flag then\n    changed = armor\n  end\n  local after_change = changed\n  return after_change\nend\n---@param shadowed Weapon\nfunction shadowed(shadowed, flag)\n  if flag then\n    local shadowed = armor\n  end\n  local after_shadow = shadowed\n  return after_shadow\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "after_change", 1, &overlays),
        None
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "after_shadow", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
}

#[test]
fn guard_narrowing_respects_binding_identity_parentheses_and_early_exit_paths() {
    let path = Path::new("/synthetic/Project/GuardIdentity.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@type Weapon\nlocal weapon = nil\n---@param shadowed Weapon?\nfunction shadowed(shadowed)\n  if shadowed == nil then\n    local shadowed = weapon\n    local inner = shadowed\n    return inner\n  end\nend\n---@param nested Weapon?\nfunction nested(nested)\n  if (nested ~= nil) then\n    do\n      local inside = nested\n    end\n  end\nend\n---@param changed Weapon?\nfunction changed(changed)\n  if changed == nil then return end\n  do\n    changed = nil\n  end\n  local after_nested_write = changed\n  return after_nested_write\nend\n---@param jumped Weapon?\nfunction jumped(jumped)\n  if jumped == nil then\n    goto done\n    return\n  end\n  ::done::\n  local after_goto = jumped\n  return after_goto\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "inner", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "nested", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "after_nested_write", 1, &overlays,),
        None
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "after_goto", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Nil,
        ]))
    );
}

#[test]
fn guard_evidence_stops_at_mutated_nested_scopes_and_function_boundaries() {
    let path = Path::new("/synthetic/Project/GuardBoundaries.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@param looped Weapon?\nfunction looped(looped)\n  if looped ~= nil then\n    for i = 1, 2 do\n      local observed = looped\n      looped = nil\n    end\n  end\nend\n---@param captured Weapon?\nfunction outer(captured)\n  if captured ~= nil then\n    function inner()\n      return captured\n    end\n  end\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "looped", 1, &overlays),
        Some(TypeExpression::Union(vec![
            TypeExpression::Name("Weapon".into()),
            TypeExpression::Nil,
        ]))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "captured", 1, &overlays),
        None
    );
}

#[test]
fn unknown_union_members_poison_fields_and_same_file_helpers_shadow_builtins() {
    let path = Path::new("/synthetic/Project/Poisoning.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@type Weapon|MissingClass\nlocal uncertain = nil\nlocal damage = uncertain.damage\nfunction ConditionResult()\n  return nil\nend\nlocal shadowed = ConditionResult()\nreturn damage, shadowed\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type(&workspace, path, text, "uncertain.damage", &overlays),
        None
    );
    assert_eq!(
        expression_type(&workspace, path, text, "ConditionResult()", &overlays),
        Some(TypeExpression::Nil)
    );
}

#[test]
fn classifies_literals_unary_binary_and_condition_result_expressions() {
    let path = Path::new("/synthetic/Project/Operators.khn");
    let text = "local bool_value = true\nlocal number_value = 3\nlocal string_value = \"x\"\nlocal nil_value = nil\nlocal not_value = not bool_value\nlocal negative_value = -number_value\nlocal sum = number_value + 1\nlocal comparison = number_value == 1\nlocal concatenated = string_value .. \"x\"\nlocal condition_a = ConditionResult(false)\nlocal condition_b = ConditionResult(true)\nlocal condition_and = condition_a & condition_b\nlocal condition_or = condition_a | condition_b\nlocal condition_xor = condition_a ~ condition_b\nreturn bool_value, number_value, string_value, nil_value, not_value, negative_value, sum, comparison, concatenated, condition_and, condition_or, condition_xor\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    for (name, expected) in [
        ("true", TypeExpression::Primitive(PrimitiveType::Boolean)),
        ("3", TypeExpression::Primitive(PrimitiveType::Number)),
        ("\"x\"", TypeExpression::Primitive(PrimitiveType::String)),
        ("nil", TypeExpression::Nil),
        (
            "not bool_value",
            TypeExpression::Primitive(PrimitiveType::Boolean),
        ),
        (
            "-number_value",
            TypeExpression::Primitive(PrimitiveType::Number),
        ),
        (
            "number_value + 1",
            TypeExpression::Primitive(PrimitiveType::Number),
        ),
        (
            "number_value == 1",
            TypeExpression::Primitive(PrimitiveType::Boolean),
        ),
        (
            "string_value .. \"x\"",
            TypeExpression::Primitive(PrimitiveType::String),
        ),
        (
            "ConditionResult(false)",
            TypeExpression::Name("ConditionResult".into()),
        ),
        (
            "condition_a & condition_b",
            TypeExpression::Name("ConditionResult".into()),
        ),
        (
            "condition_a | condition_b",
            TypeExpression::Name("ConditionResult".into()),
        ),
        (
            "condition_a ~ condition_b",
            TypeExpression::Name("ConditionResult".into()),
        ),
    ] {
        assert_eq!(
            expression_type(&workspace, path, text, name, &overlays),
            Some(expected),
            "unexpected type for {name}"
        );
    }
}

#[test]
fn preserves_schema_backed_enum_values() {
    let path = Path::new("/synthetic/Project/Enums.khn");
    let text = "local ability = Ability.Strength\nreturn ability\n";
    let mut schema = SchemaCatalog::default();
    schema
        .merge_enumerations(
            "<enumerations><enumeration name=\"Ability\"><item value=\"Strength\"/><item value=\"Dexterity\"/></enumeration></enumerations>",
        )
        .expect("schema enumeration");
    let parsed = parse_source(
        SourceFile {
            path: path.to_path_buf(),
            kind: SourceKind::Thoth,
        },
        text,
        &schema,
        "English",
    )
    .expect("synthetic Thoth source");
    let project = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Project".into(),
            root: PathBuf::from("/synthetic/Project"),
            role: ModuleRole::Project,
        },
        vec![parsed],
    ));
    let workspace = WorkspaceSnapshot::new(Arc::new(schema), vec![project], 1, 200, 200);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type(&workspace, path, text, "Ability.Strength", &overlays),
        Some(TypeExpression::Name("Ability".into()))
    );
    assert_eq!(
        expression_type_at(&workspace, path, text, "ability", 1, &overlays),
        Some(TypeExpression::Name("Ability".into()))
    );
}

#[test]
fn local_bindings_shadow_schema_enumeration_roots() {
    let path = Path::new("/synthetic/Project/EnumShadow.khn");
    let text = "---@class Weapon\n---@field Strength integer\n---@type Weapon\nlocal Ability = nil\nlocal value = Ability.Strength\nreturn value\n";
    let mut schema = SchemaCatalog::default();
    schema
        .merge_enumerations(
            "<enumerations><enumeration name=\"Ability\"><item value=\"Strength\"/></enumeration></enumerations>",
        )
        .expect("schema enumeration");
    let parsed = parse_source(
        SourceFile {
            path: path.to_path_buf(),
            kind: SourceKind::Thoth,
        },
        text,
        &schema,
        "English",
    )
    .expect("synthetic Thoth source");
    let project = Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Project".into(),
            root: PathBuf::from("/synthetic/Project"),
            role: ModuleRole::Project,
        },
        vec![parsed],
    ));
    let workspace = WorkspaceSnapshot::new(Arc::new(schema), vec![project], 1, 200, 200);
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type(&workspace, path, text, "Ability.Strength", &overlays),
        Some(TypeExpression::Primitive(PrimitiveType::Integer))
    );
}

#[test]
fn unknown_calls_and_operators_remain_silent() {
    let path = Path::new("/synthetic/Project/Unknown.khn");
    let text = "local call = MissingHelper()\nlocal operation = call + 1\nlocal comparison = call < 1\nlocal unknown = ???\nreturn call, operation, comparison, unknown\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    for name in ["call", "operation", "comparison", "unknown"] {
        assert_eq!(
            expression_type(&workspace, path, text, name, &overlays),
            None,
            "unknown expression {name} acquired a type"
        );
    }
}

#[test]
fn overlays_replace_disk_flow_facts() {
    let path = Path::new("/synthetic/Project/Overlay.khn");
    let disk = "---@class Weapon\n---@field damage integer\n---@type Weapon\nlocal value = nil\nreturn value\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, disk)],
    );
    let workspace = workspace(vec![project]);
    let open = "---@class Armor\n---@field armor_only boolean\n---@type Armor\nlocal value = nil\nreturn value\n";
    let overlays = overlay(&workspace, path, "Project", open);

    assert_eq!(
        expression_type_at(&workspace, path, open, "value", 1, &overlays),
        Some(TypeExpression::Name("Armor".into()))
    );
}

#[test]
fn module_precedence_and_same_rank_ambiguity_are_conservative() {
    let base_path = "/synthetic/Base/Types.khn";
    let project_path = "/synthetic/Project/Types.khn";
    let query_path = Path::new("/synthetic/Project/Query.khn");
    let query = "---@type Weapon\nlocal value = nil\nreturn value\n";
    let base = module(
        "Base",
        "/synthetic/Base",
        ModuleRole::Base,
        &[(
            base_path,
            SourceKind::Thoth,
            "---@class Weapon\n---@field base integer\n",
        )],
    );
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(
            project_path,
            SourceKind::Thoth,
            "---@class Weapon\n---@field project string\n",
        )],
    );
    let precedence = workspace(vec![base, project]);
    let overlays = overlay(&precedence, query_path, "Project", query);
    assert_eq!(
        expression_type_at(&precedence, query_path, query, "value", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );

    let ambiguous = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[
            (
                "/synthetic/Project/A.khn",
                SourceKind::Thoth,
                "---@class Weapon\n---@field left integer\n",
            ),
            (
                "/synthetic/Project/B.khn",
                SourceKind::Thoth,
                "---@class Weapon\n---@field right integer\n",
            ),
        ],
    );
    let ambiguous_workspace = workspace(vec![ambiguous]);
    let overlays = overlay(&ambiguous_workspace, query_path, "Project", query);
    assert_eq!(
        expression_type_at(
            &ambiguous_workspace,
            query_path,
            query,
            "value",
            1,
            &overlays,
        ),
        None
    );
}

#[test]
fn packaged_explicit_helper_is_available_without_a_fake_path() {
    let path = Path::new("/synthetic/Project/Query.khn");
    let text = "local value = PackagedWeapon()\nreturn value\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let base = module("BaseApi", "/synthetic/BaseApi", ModuleRole::Base, &[]);
    let workspace = packaged_workspace(
        workspace(vec![base, project]),
        [PackagedThothSource::new(
            "BaseApi",
            "/synthetic/base.pak",
            "Mods/BaseApi/Scripts/thoth/helpers/api.khn",
            0,
            "---@class Weapon\n---@field damage integer\n---@return Weapon\nfunction PackagedWeapon() end\n",
        )
        .expect("packaged source")],
    );
    let overlays = overlay(&workspace, path, "Project", text);

    assert_eq!(
        expression_type_at(&workspace, path, text, "value", 1, &overlays),
        Some(TypeExpression::Name("Weapon".into()))
    );
}

#[test]
fn stats_values_do_not_leak_into_thoth_flow() {
    let stats_path = "/synthetic/Project/Stats/Generated/Data/Passive.txt";
    let thoth_path = Path::new("/synthetic/Project/Query.khn");
    let thoth = "local value = HasStatus(SELF, 'Wet')\nreturn value\n";
    let stats = "new entry \"Passive\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\ndata \"Boosts\" \"HasStatus(SELF, 'Wet')\"\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[
            (stats_path, SourceKind::PlainStats, stats),
            (thoth_path.to_str().unwrap(), SourceKind::Thoth, thoth),
        ],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, thoth_path, "Project", thoth);

    let mut schema = SchemaCatalog::default();
    schema
        .merge_definitions(
            "<root><stat_object_definition id=\"passive\" name=\"Passive\" export_type=\"PassiveData\"><field_definition name=\"Enabled\" type=\"Enumeration\" enumeration_type_name=\"YesNo\" /></stat_object_definition></root>",
        )
        .expect("schema definition");
    schema
        .merge_enumerations(
            "<enumerations><enumeration name=\"YesNo\"><item value=\"No\"/><item value=\"Yes\"/></enumeration></enumerations>",
        )
        .expect("schema enumeration");
    assert_eq!(schema.enumerations["YesNo"], ["No", "Yes"]);
    let stats_file = parse_source(
        SourceFile {
            path: PathBuf::from(stats_path),
            kind: SourceKind::PlainStats,
        },
        stats,
        &schema,
        "English",
    )
    .expect("Stats fixture");
    assert!(stats_file.thoth.is_none());

    assert_eq!(
        expression_type_at(&workspace, thoth_path, thoth, "value", 1, &overlays),
        None
    );
    assert!(
        workspace
            .thoth_expression_type(Path::new(stats_path), TextRange::default(), &overlays)
            .is_none()
    );
}

#[test]
fn expression_ranges_are_exact_and_nested_facts_remain_distinct() {
    let path = Path::new("/synthetic/Project/Ranges.khn");
    let text = "local first = true\nlocal second = true\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), SourceKind::Thoth, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);
    let first = expression_range_at(text, "true", 0);
    let second = expression_range_at(text, "true", 1);
    assert_ne!(first, second);
    assert_eq!(
        workspace.thoth_expression_type(path, first, &overlays),
        Some(TypeExpression::Primitive(PrimitiveType::Boolean))
    );
    assert_eq!(
        workspace.thoth_expression_type(path, second, &overlays),
        Some(TypeExpression::Primitive(PrimitiveType::Boolean))
    );
}
