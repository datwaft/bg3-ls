//! Synthetic integration coverage for explicit Thoth type resolution.
//!
//! These tests intentionally exercise the editor-facing API with loose and
//! packaged sources. They do not use installed game data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, PackagedThothCatalog, PackagedThothSource, SchemaCatalog,
    SourceFile, SourceKind, ThothScopeId, TypeExpression, parse_packaged_thoth_facts, parse_source,
    parse_thoth_file,
};

fn module(name: &str, root: &str, role: ModuleRole, files: &[(&str, &str)]) -> Arc<ModuleIndex> {
    let schema = SchemaCatalog::default();
    let parsed = files
        .iter()
        .map(|(path, text)| {
            parse_source(
                SourceFile {
                    path: PathBuf::from(path),
                    kind: SourceKind::Thoth,
                },
                text,
                &schema,
                "English",
            )
            .expect("synthetic Thoth source")
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

fn overlays(workspace: &WorkspaceSnapshot, path: &Path, module: &str, text: &str) -> OverlaySet {
    let parsed = parse_source(
        SourceFile {
            path: path.to_owned(),
            kind: SourceKind::Thoth,
        },
        text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic overlay");
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: module.into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
    overlays
}

fn packaged(
    module: &str,
    package: &str,
    entry: &str,
    priority: u8,
    text: &str,
) -> PackagedThothSource {
    PackagedThothSource::new(module, package, entry, priority, text).expect("synthetic package")
}

#[test]
fn resolves_alias_chains_nullable_types_and_class_fields() {
    let path = "/synthetic/project/Types.khn";
    let text = "---@class Weapon\n---@field IsValid boolean\n---@alias MaybeWeapon Weapon|nil\n---@alias WeaponAlias IntermediateWeapon\n---@alias IntermediateWeapon Weapon\n---@alias WeaponId integer\n";
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(path, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = OverlaySet::default();
    let facts = parse_thoth_file(text).unwrap();

    let weapon = workspace
        .resolve_thoth_class("Weapon", &overlays)
        .expect("class");
    assert_eq!(weapon.name, "Weapon");
    assert_eq!(weapon.name_range, facts.annotations.classes[0].name_range);
    assert_eq!(weapon.fields[0].name, "IsValid");
    assert_eq!(
        weapon.fields[0].range,
        facts.annotations.classes[0].fields[0].range
    );
    assert_eq!(
        weapon.fields[0].name_range,
        facts.annotations.classes[0].fields[0].name_range
    );
    assert_eq!(
        weapon.fields[0].type_range,
        facts.annotations.classes[0].fields[0].type_range
    );
    assert_eq!(
        weapon.fields[0].ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Boolean)
    );
    assert_eq!(
        weapon.source,
        bg3_ide::ThothTypeSource::Loose {
            module: "Project".into(),
            path: path.into(),
            range: facts.annotations.classes[0].range,
        }
    );

    let alias = workspace
        .resolve_thoth_alias("MaybeWeapon", &overlays)
        .expect("alias");
    assert_eq!(alias.ty.to_string(), "Weapon|nil");
    assert_eq!(alias.name_range, facts.annotations.aliases[0].name_range);
    assert_eq!(alias.type_range, facts.annotations.aliases[0].type_range);
    assert_eq!(
        alias.source,
        bg3_ide::ThothTypeSource::Loose {
            module: "Project".into(),
            path: path.into(),
            range: facts.annotations.aliases[0].range,
        }
    );
    let resolved =
        workspace.resolve_thoth_type(&TypeExpression::Name("WeaponAlias".into()), &overlays);
    assert_eq!(resolved, TypeExpression::Name("Weapon".into()));
    let id = workspace.resolve_thoth_type(&TypeExpression::Name("WeaponId".into()), &overlays);
    assert_eq!(
        id,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Integer)
    );
}

#[test]
fn missing_and_cyclic_aliases_are_unknown() {
    let path = "/synthetic/project/Types.khn";
    let text = "---@alias MissingAlias DoesNotExist\n---@alias PartiallyKnown string|DoesNotExist\n---@alias MissingArray DoesNotExist[]\n---@alias CycleA CycleB\n---@alias CycleB CycleA\n";
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(path, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = OverlaySet::default();

    assert!(
        workspace
            .resolve_thoth_type(&TypeExpression::Name("MissingAlias".into()), &overlays)
            .is_unknown()
    );
    assert!(
        workspace
            .resolve_thoth_type(&TypeExpression::Name("CycleA".into()), &overlays)
            .is_unknown()
    );
    assert_eq!(
        workspace.resolve_thoth_type(&TypeExpression::Name("PartiallyKnown".into()), &overlays),
        TypeExpression::union([
            TypeExpression::Primitive(bg3_index::PrimitiveType::String),
            TypeExpression::Unknown,
        ])
    );
    assert_eq!(
        workspace.resolve_thoth_type(&TypeExpression::Name("MissingArray".into()), &overlays),
        TypeExpression::Array(Box::new(TypeExpression::Unknown))
    );
}

#[test]
fn resolves_function_parameters_returns_and_exact_type_variable() {
    let path = "/synthetic/project/Functions.khn";
    let text = "---@param value string\n---@return boolean\nfunction IsValid(value) end\n---@type string\nlocal value = \"ok\"\n";
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(path, text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = OverlaySet::default();
    let facts = parse_thoth_file(text).unwrap();

    let function = workspace
        .resolve_thoth_function("IsValid", &overlays)
        .expect("function");
    assert_eq!(
        function.name_range,
        facts.annotations.functions[0].name_range.unwrap()
    );
    assert_eq!(
        function.source,
        bg3_ide::ThothTypeSource::Loose {
            module: "Project".into(),
            path: path.into(),
            range: facts.annotations.functions[0].range,
        }
    );
    assert_eq!(function.contracts.len(), 1);
    assert_eq!(
        function.contracts[0].parameters[0].ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::String)
    );
    assert_eq!(
        function.contracts[0].returns[0].ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Boolean)
    );

    let variable_range = facts.annotations.variables[0].target_range;
    let variable = workspace
        .resolve_thoth_variable(Path::new(path), variable_range, &overlays)
        .expect("variable");
    assert_eq!(
        variable.ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::String)
    );
    assert!(variable.local);
    assert!(!variable.global);
    assert_eq!(variable.statement.scope, ThothScopeId::File);
    assert_eq!(
        variable.target_range,
        facts.annotations.variables[0].target_range
    );
    assert_eq!(
        variable.type_range,
        facts.annotations.variables[0].type_range
    );
    assert_eq!(
        variable.source,
        bg3_ide::ThothTypeSource::Loose {
            module: "Project".into(),
            path: path.into(),
            range: facts.annotations.variables[0].range,
        }
    );
}

#[test]
fn resolves_explicit_global_variable_identity() {
    let path = "/synthetic/project/Globals.khn";
    let text = "---@type string\nglobal shared = \"value\"\n";
    let facts = parse_thoth_file(text).unwrap();
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(path, text)],
    );
    let variable = workspace(vec![project])
        .resolve_thoth_variable(
            Path::new(path),
            facts.annotations.variables[0].target_range,
            &OverlaySet::default(),
        )
        .expect("global variable");

    assert!(!variable.local);
    assert!(variable.global);
    assert_eq!(variable.statement.scope, ThothScopeId::File);
}

#[test]
fn precedence_overlay_and_ambiguity_are_conservative() {
    let base_path = "/synthetic/base/Helper.khn";
    let dependency_path = "/synthetic/dependency/Helper.khn";
    let project_path = "/synthetic/project/Helper.khn";
    let base = module(
        "Base",
        "/synthetic/base",
        ModuleRole::Base,
        &[(base_path, "---@param x string\nfunction Helper(x) end\n")],
    );
    let dependency = module(
        "Dependency",
        "/synthetic/dependency",
        ModuleRole::Dependency,
        &[(
            dependency_path,
            "---@param x number\nfunction Helper(x) end\n",
        )],
    );
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(project_path, "function Helper(x) end\n")],
    );
    let workspace = workspace(vec![base, dependency, project]);
    let current_overlays = OverlaySet::default();
    assert!(
        workspace
            .resolve_thoth_function("Helper", &current_overlays)
            .is_none(),
        "unannotated higher-rank declaration masks lower typed metadata"
    );

    let overlay = overlays(
        &workspace,
        Path::new(project_path),
        "Project",
        "---@param x boolean\nfunction Helper(x) end\n",
    );
    let function = workspace
        .resolve_thoth_function("Helper", &overlay)
        .expect("overlay function");
    assert_eq!(
        function.contracts[0].parameters[0].ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Boolean)
    );
}

#[test]
fn packaged_resolution_respects_module_visibility_priority_and_ambiguity() {
    let project_path = "/synthetic/project/Use.khn";
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(project_path, "")],
    );
    let configured = module("Configured", "/synthetic/configured", ModuleRole::Base, &[]);
    let catalog = PackagedThothCatalog::from_sources([
        packaged(
            "Configured",
            "low.pak",
            "Mods/Configured/Scripts/thoth/Types.khn",
            0,
            "---@alias Visible string\n",
        ),
        packaged(
            "Configured",
            "high.pak",
            "Mods/Configured/Scripts/thoth/Types.khn",
            1,
            "---@alias Visible number\n",
        ),
        packaged(
            "Hidden",
            "hidden.pak",
            "Mods/Hidden/Scripts/thoth/Types.khn",
            9,
            "---@alias Invisible boolean\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .expect("facts");
    let workspace = workspace(vec![configured, project])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));
    let overlays = OverlaySet::default();
    assert!(
        workspace
            .resolve_thoth_alias("Invisible", &overlays)
            .is_none()
    );
    let visible = workspace
        .resolve_thoth_alias("Visible", &overlays)
        .expect("visible package");
    assert_eq!(
        visible.ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Number)
    );
    let bg3_ide::ThothTypeSource::Packaged {
        module,
        package,
        entry,
        ..
    } = visible.source
    else {
        panic!("package evidence must remain virtual");
    };
    assert_eq!(module, "Configured");
    assert_eq!(package, PathBuf::from("high.pak"));
    assert_eq!(entry, "Mods/Configured/Scripts/thoth/Types.khn");
}

#[test]
fn equal_priority_packaged_contracts_are_suppressed() {
    let configured = module("Configured", "/synthetic/configured", ModuleRole::Base, &[]);
    let catalog = PackagedThothCatalog::from_sources([
        packaged(
            "Configured",
            "a.pak",
            "Mods/Configured/Scripts/thoth/Types.khn",
            0,
            "---@alias Ambiguous string\n",
        ),
        packaged(
            "Configured",
            "b.pak",
            "Mods/Configured/Scripts/thoth/Types.khn",
            0,
            "---@alias Ambiguous number\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .expect("facts");
    let workspace = workspace(vec![configured])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));
    assert!(
        workspace
            .resolve_thoth_alias("Ambiguous", &OverlaySet::default())
            .is_none()
    );
}

#[test]
fn unannotated_packaged_function_masks_lower_typed_contract() {
    let lower = module("Lower", "/synthetic/lower", ModuleRole::Base, &[]);
    let higher = module("Higher", "/synthetic/higher", ModuleRole::Base, &[]);
    let catalog = PackagedThothCatalog::from_sources([
        packaged(
            "Lower",
            "lower.pak",
            "Mods/Lower/Scripts/thoth/Helper.khn",
            0,
            "---@param value string\nfunction Helper(value) end\n",
        ),
        packaged(
            "Higher",
            "higher.pak",
            "Mods/Higher/Scripts/thoth/Helper.khn",
            0,
            "function Helper(value) end\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .expect("facts");
    let workspace = workspace(vec![lower, higher])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));

    assert!(
        workspace
            .resolve_thoth_function("Helper", &OverlaySet::default())
            .is_none()
    );
}

#[test]
fn unrelated_packaged_entry_ambiguity_does_not_hide_a_unique_contract() {
    let configured = module("Configured", "/synthetic/configured", ModuleRole::Base, &[]);
    let catalog = PackagedThothCatalog::from_sources([
        packaged(
            "Configured",
            "configured.pak",
            "Mods/Configured/Scripts/thoth/Healthy.khn",
            0,
            "---@return boolean\nfunction Healthy() end\n",
        ),
        packaged(
            "Configured",
            "first.pak",
            "Mods/Configured/Scripts/thoth/Ambiguous.khn",
            0,
            "function Unrelated() end\n",
        ),
        packaged(
            "Configured",
            "second.pak",
            "Mods/Configured/Scripts/thoth/Ambiguous.khn",
            0,
            "function Unrelated() end\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .expect("facts");
    let workspace = workspace(vec![configured])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));

    let function = workspace
        .resolve_thoth_function("Healthy", &OverlaySet::default())
        .expect("unique packaged function");
    assert_eq!(
        function.contracts[0].returns[0].ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Boolean)
    );
    assert!(
        workspace
            .resolve_thoth_function("Unrelated", &OverlaySet::default())
            .is_none()
    );
}

#[test]
fn same_rank_loose_nominals_and_functions_are_suppressed() {
    let a = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(
            "/synthetic/project/A.khn",
            "---@alias SharedType string\n---@class SharedClass\n---@param value string\nfunction SharedFunction(value) end\n",
        )],
    );
    let b = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(
            "/synthetic/project/B.khn",
            "---@alias SharedType number\n---@class SharedClass\n---@param value number\nfunction SharedFunction(value) end\n",
        )],
    );
    let workspace = workspace(vec![Arc::new(ModuleIndex::new(
        ModuleSpec {
            name: "Project".into(),
            root: "/synthetic/project".into(),
            role: ModuleRole::Project,
        },
        a.files
            .values()
            .chain(b.files.values())
            .map(|file| file.as_ref().clone())
            .collect(),
    ))]);
    let overlays = OverlaySet::default();
    assert!(
        workspace
            .resolve_thoth_alias("SharedType", &overlays)
            .is_none()
    );
    assert!(
        workspace
            .resolve_thoth_class("SharedClass", &overlays)
            .is_none()
    );
    assert!(
        workspace
            .resolve_thoth_function("SharedFunction", &overlays)
            .is_none()
    );
}

#[test]
fn packaged_entries_and_rejected_higher_priority_sources_do_not_fall_through() {
    let configured = module("Configured", "/synthetic/configured", ModuleRole::Base, &[]);
    let catalog = PackagedThothCatalog::from_sources([
        packaged(
            "Configured",
            "low.pak",
            "Mods/Configured/Scripts/thoth/Low.khn",
            0,
            "---@alias Selected string\n",
        ),
        packaged(
            "Configured",
            "other.pak",
            "Mods/Configured/Scripts/thoth/Other.khn",
            0,
            "---@alias Selected number\n",
        ),
        packaged(
            "Configured",
            "broken.pak",
            "Mods/Configured/Scripts/thoth/Broken.khn",
            2,
            "---@alias Broken..Name string\n",
        ),
        packaged(
            "Configured",
            "valid-low.pak",
            "Mods/Configured/Scripts/thoth/Broken.khn",
            0,
            "---@alias BrokenType boolean\n",
        ),
    ])
    .expect("catalog");
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .expect("facts");
    let workspace = workspace(vec![configured])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));
    let overlays = OverlaySet::default();
    assert!(
        workspace
            .resolve_thoth_alias("Selected", &overlays)
            .is_none()
    );
    assert!(
        workspace
            .resolve_thoth_alias("BrokenType", &overlays)
            .is_none()
    );
}

#[test]
fn loose_base_evidence_wins_packaged_fallback() {
    let loose = module(
        "Base",
        "/synthetic/base",
        ModuleRole::Base,
        &[("/synthetic/base/Types.khn", "---@alias Selected string\n")],
    );
    let catalog = PackagedThothCatalog::from_sources([packaged(
        "Base",
        "base.pak",
        "Mods/Base/Scripts/thoth/Types.khn",
        0,
        "---@alias Selected number\n",
    )])
    .unwrap();
    let facts = parse_packaged_thoth_facts(&catalog, "test", |source| {
        bg3_index::parse_thoth_file(source.text())
    })
    .unwrap();
    let workspace = workspace(vec![loose])
        .with_packaged_thoth(Arc::new(catalog))
        .with_packaged_thoth_facts(Arc::new(facts));
    let selected = workspace
        .resolve_thoth_alias("Selected", &OverlaySet::default())
        .expect("loose alias");
    assert_eq!(
        selected.ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::String)
    );
}

#[test]
fn variable_resolution_requires_exact_single_direct_target_and_overlay_replaces_disk() {
    let path = "/synthetic/project/Variables.khn";
    let disk = "---@type string\nlocal value = \"disk\"\n---@type boolean\nobj.field = true\n---@type number\nlocal left, right = 1, 2\n";
    let project = module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(path, disk)],
    );
    let disk_workspace = workspace(vec![project]);
    let disk_facts = parse_thoth_file(disk).unwrap();
    let value_range = disk_facts.annotations.variables[0].target_range;
    let member_range = disk_facts.annotations.variables[1].target_range;
    let value = disk_workspace
        .resolve_thoth_variable(Path::new(path), value_range, &OverlaySet::default())
        .expect("disk value");
    assert!(value.local && !value.global);
    assert_eq!(
        value.target_range,
        disk_facts.annotations.variables[0].target_range
    );
    assert_eq!(
        value.type_range,
        disk_facts.annotations.variables[0].type_range
    );
    assert!(
        disk_workspace
            .resolve_thoth_variable(Path::new(path), member_range, &OverlaySet::default())
            .is_none()
    );
    let multi_text = "---@type number\nlocal left, right = 1, 2\n";
    let multi_facts = parse_thoth_file(multi_text).unwrap();
    assert!(multi_facts.annotations.variables.is_empty());
    let multi_range = multi_facts.assignments[0].targets[0].range;
    let multi_path = "/synthetic/project/Multi.khn";
    let multi_workspace = workspace(vec![module(
        "Project",
        "/synthetic/project",
        ModuleRole::Project,
        &[(multi_path, multi_text)],
    )]);
    assert!(
        multi_workspace
            .resolve_thoth_variable(Path::new(multi_path), multi_range, &OverlaySet::default())
            .is_none()
    );

    let overlay_text = "---@type boolean\nlocal value = true\n";
    let overlay = overlays(&disk_workspace, Path::new(path), "Project", overlay_text);
    let overlay_range = parse_thoth_file(overlay_text)
        .unwrap()
        .annotations
        .variables[0]
        .target_range;
    let replaced = disk_workspace
        .resolve_thoth_variable(Path::new(path), overlay_range, &overlay)
        .expect("overlay value");
    assert_eq!(
        replaced.ty,
        TypeExpression::Primitive(bg3_index::PrimitiveType::Boolean)
    );
}
