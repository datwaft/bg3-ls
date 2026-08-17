//! Synthetic integration coverage for Thoth member navigation.
//!
//! These tests use only small, repository-local source strings. They cover
//! the editor-facing behavior requested by issue #64.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, SourceLocation, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, PackagedThothCatalog, PackagedThothSource, Position,
    SchemaCatalog, SourceFile, SourceKind, parse_packaged_thoth_facts, parse_source,
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

fn position(text: &str, needle: &str, after: bool) -> Position {
    let offset = if after {
        text.rfind(needle).expect("needle in synthetic source") + needle.len()
    } else {
        text.rfind(needle).expect("needle in synthetic source")
    };
    let line = text[..offset].bytes().filter(|byte| *byte == b'\n').count();
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    Position {
        line: u32::try_from(line).expect("synthetic line fits"),
        character: u32::try_from(offset - line_start).expect("synthetic character fits"),
    }
}

fn first_position(text: &str, needle: &str, after: bool) -> Position {
    let offset = if after {
        text.find(needle).expect("needle in synthetic source") + needle.len()
    } else {
        text.find(needle).expect("needle in synthetic source")
    };
    let line = text[..offset].bytes().filter(|byte| *byte == b'\n').count();
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    Position {
        line: u32::try_from(line).expect("synthetic line fits"),
        character: u32::try_from(offset - line_start).expect("synthetic character fits"),
    }
}

fn labels(completion: &bg3_ide::CompletionList) -> Vec<&str> {
    completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect()
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

fn packaged_workspace(
    workspace: WorkspaceSnapshot,
    sources: impl IntoIterator<Item = PackagedThothSource>,
) -> WorkspaceSnapshot {
    let catalog = Arc::new(PackagedThothCatalog::from_sources(sources).expect("catalog"));
    let facts = Arc::new(
        parse_packaged_thoth_facts(catalog.as_ref(), "test-thoth-members", |source| {
            parse_thoth_file(source.text())
        })
        .expect("packaged facts"),
    );
    workspace
        .with_packaged_thoth(catalog)
        .with_packaged_thoth_facts(facts)
}

#[test]
fn completes_hovers_and_navigates_typed_local_fields() {
    let path = Path::new("/synthetic/Project/Weapon.khn");
    let declarations = "---@class Weapon\n---@field damage integer\n---@field name string\n---@field attack fun(target: string): boolean\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), declarations)],
    );
    let workspace = workspace(vec![project]);
    let text = format!(
        "{declarations}---@type Weapon\nlocal weapon = nil\nlocal partial = weapon.da\nlocal action = weapon.at\nlocal result = weapon.damage\n"
    );
    let overlays = overlay(&workspace, path, "Project", &text);

    let completion =
        workspace.completion(path, position(&text, "weapon.da", true), &overlays, false);
    assert!(labels(&completion).contains(&"damage"));
    assert!(!labels(&completion).contains(&"name"));

    let attack = workspace
        .completion(path, position(&text, "weapon.at", true), &overlays, false)
        .items
        .into_iter()
        .find(|item| item.label == "attack")
        .expect("function-shaped member");
    assert_eq!(attack.kind, bg3_ide::CompletionKind::Function);
    assert!(attack.detail.unwrap().starts_with("fun("));

    let hover = workspace
        .language_hover(path, position(&text, "damage", false), &overlays)
        .expect("field hover");
    assert!(hover.contains("damage"));
    assert!(hover.contains("integer"));

    let facts = parse_thoth_file(&text).expect("facts");
    let field = &facts.annotations.classes[0].fields[0];
    let locations =
        workspace.definition_locations_at(path, position(&text, "damage", false), &overlays);
    assert_eq!(
        locations,
        vec![SourceLocation {
            path: path.to_path_buf(),
            range: field.name_range,
        }]
    );
}

#[test]
fn completes_an_empty_member_prefix_in_incomplete_syntax() {
    let path = Path::new("/synthetic/Project/Incomplete.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@field name string\n---@type Weapon\nlocal weapon = nil\nlocal result = weapon.\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    let completion = workspace.completion(path, position(text, "weapon.", true), &overlays, false);
    let labels = labels(&completion);
    assert!(labels.contains(&"damage"));
    assert!(labels.contains(&"name"));
}

#[test]
fn parameter_types_are_scoped_and_do_not_leak() {
    let path = Path::new("/synthetic/Project/Scope.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@param item Weapon\nfunction inspect(item)\n  return item.da\nend\nreturn item.da\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    let inside = workspace.completion(
        path,
        first_position(text, "item.da", true),
        &overlays,
        false,
    );
    assert!(labels(&inside).contains(&"damage"));

    let outside = workspace.completion(path, position(text, "item.da", true), &overlays, false);
    assert!(!labels(&outside).contains(&"damage"));
}

#[test]
fn parameters_shadow_outer_bindings_and_untyped_locals_mask_parameters() {
    let path = Path::new("/synthetic/Project/Shadows.khn");
    let text = "---@class Armor\n---@field armor_only boolean\n---@class Weapon\n---@field damage integer\n---@type Armor\nlocal item = nil\n---@param item Weapon\nfunction inspect(item)\n  local result = item.da\nend\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);
    let completion = workspace.completion(path, position(text, "item.da", true), &overlays, false);
    let labels = labels(&completion);
    assert!(labels.contains(&"damage"));
    assert!(!labels.contains(&"armor_only"));

    let masked = "---@class Weapon\n---@field damage integer\n---@param item Weapon\nfunction inspect(item)\n  local item = nil\n  local result = item.da\nend\n";
    let overlays = overlay(&workspace, path, "Project", masked);
    assert!(
        workspace
            .completion(path, position(masked, "item.da", true), &overlays, false,)
            .items
            .is_empty()
    );
}

#[test]
fn follows_annotated_helper_returns_through_nested_members() {
    let path = Path::new("/synthetic/Project/Returns.khn");
    let text = "---@class Inner\n---@field value string\n---@class Outer\n---@field inner Inner\n---@return Outer\nfunction make_outer() end\nlocal partial = make_outer().inner.va\nlocal full = make_outer().inner.value\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);

    let completion = workspace.completion(
        path,
        position(text, "make_outer().inner.va", true),
        &overlays,
        false,
    );
    assert!(labels(&completion).contains(&"value"));
    assert!(!labels(&completion).contains(&"inner"));

    let facts = parse_thoth_file(text).expect("facts");
    assert_eq!(
        workspace.definition_locations_at(path, position(text, "value", false), &overlays),
        vec![SourceLocation {
            path: path.to_path_buf(),
            range: facts.annotations.classes[0].fields[0].name_range,
        }]
    );
}

#[test]
fn union_completion_keeps_common_fields_and_ignores_nil() {
    let path = Path::new("/synthetic/Project/Union.khn");
    let text = "---@class Left\n---@field common string\n---@field left_only integer\n---@class Right\n---@field common string\n---@field right_only boolean\n---@type Left|Right|nil\nlocal value = nil\nreturn value.co\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let overlays = overlay(&workspace, path, "Project", text);
    let completion = workspace.completion(path, position(text, "value.co", true), &overlays, false);
    let labels = labels(&completion);
    assert!(labels.contains(&"common"));
    assert!(!labels.contains(&"left_only"));
    assert!(!labels.contains(&"right_only"));
}

#[test]
fn overlay_precedence_and_same_rank_ambiguity_stay_conservative() {
    let low_path = "/synthetic/Base/Types.khn";
    let high_path = "/synthetic/Project/Types.khn";
    let low = module(
        "Base",
        "/synthetic/Base",
        ModuleRole::Base,
        &[(low_path, "---@class Weapon\n---@field low integer\n")],
    );
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(high_path, "---@class Weapon\n---@field project string\n")],
    );
    let precedence_workspace = workspace(vec![low, project]);
    let query_path = Path::new("/synthetic/Project/Query.khn");
    let text = "---@type Weapon\nlocal weapon = nil\nreturn weapon.pr\n";
    let overlays = overlay(&precedence_workspace, query_path, "Project", text);
    let completion = precedence_workspace.completion(
        query_path,
        position(text, "weapon.pr", true),
        &overlays,
        false,
    );
    let project_labels = labels(&completion);
    assert!(project_labels.contains(&"project"));
    assert!(!project_labels.contains(&"low"));

    let overlay_text = "---@class Weapon\n---@field overlay boolean\n";
    let query = "---@type Weapon\nlocal weapon = nil\nreturn weapon.ov\n";
    let mut overlays = overlay(&precedence_workspace, query_path, "Project", query);
    insert_overlay(
        &precedence_workspace,
        &mut overlays,
        Path::new(high_path),
        "Project",
        overlay_text,
    );
    let completion = precedence_workspace.completion(
        query_path,
        position(query, "weapon.ov", true),
        &overlays,
        false,
    );
    assert!(labels(&completion).contains(&"overlay"));
    assert!(!labels(&completion).contains(&"project"));

    let ambiguous_module = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[
            (
                "/synthetic/Project/A.khn",
                "---@class Weapon\n---@field left integer\n",
            ),
            (
                "/synthetic/Project/B.khn",
                "---@class Weapon\n---@field right integer\n",
            ),
        ],
    );
    let ambiguous_workspace = workspace(vec![ambiguous_module]);
    let query_path = Path::new("/synthetic/Project/Query.khn");
    let query = "---@type Weapon\nlocal weapon = nil\nreturn weapon.le\n";
    let query_overlays = overlay(&ambiguous_workspace, query_path, "Project", query);
    let completion = ambiguous_workspace.completion(
        query_path,
        position(query, "weapon.le", true),
        &query_overlays,
        false,
    );
    assert!(!labels(&completion).contains(&"left"));
    assert!(!labels(&completion).contains(&"right"));
}

#[test]
fn packaged_fields_have_completion_and_hover_but_no_locations() {
    let project_path = Path::new("/synthetic/Project/Query.khn");
    let workspace = packaged_workspace(
        workspace(vec![
            module("Shared", "/synthetic/Shared", ModuleRole::Base, &[]),
            module("Project", "/synthetic/Project", ModuleRole::Project, &[]),
        ]),
        [packaged(
            "Shared",
            "Shared.pak",
            "Mods/Shared/Scripts/thoth/helpers/Installed.khn",
            0,
            "---@class Installed\n---@field value string\n",
        )],
    );
    let text = "---@type Installed\nlocal value = nil\nreturn value.va\nreturn value.value\n";
    let overlays = overlay(&workspace, project_path, "Project", text);
    let completion = workspace.completion(
        project_path,
        position(text, "value.va", true),
        &overlays,
        false,
    );
    assert!(labels(&completion).contains(&"value"));

    let hover = workspace
        .language_hover(project_path, position(text, "value", false), &overlays)
        .expect("packaged field hover");
    assert!(hover.contains("value"));
    assert!(hover.to_ascii_lowercase().contains("installed"));
    assert!(
        workspace
            .definition_locations_at(project_path, position(text, "value", false), &overlays,)
            .is_empty()
    );
}

#[test]
fn unknown_types_are_silent() {
    let path = Path::new("/synthetic/Project/Unknown.khn");
    let workspace = workspace(vec![module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[],
    )]);
    let text = "---@type MissingType\nlocal value = nil\nreturn value.va\n";
    let overlays = overlay(&workspace, path, "Project", text);
    let position = position(text, "value.va", true);
    assert!(
        workspace
            .completion(path, position, &overlays, false)
            .items
            .is_empty()
    );
    assert!(
        workspace
            .language_hover(path, position, &overlays)
            .is_none()
    );
    assert!(
        workspace
            .definition_locations_at(path, position, &overlays)
            .is_empty()
    );
}
