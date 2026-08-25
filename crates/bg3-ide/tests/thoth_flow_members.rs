//! Observable member-language integration for Thoth type flow.
//!
//! These tests intentionally exercise completion, hover, and definition
//! results.  The fixtures contain no installed game or mod data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, SourceLocation, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, Position, SchemaCatalog, SourceFile, SourceKind,
    parse_source,
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

fn overlays(workspace: &WorkspaceSnapshot, path: &Path, module: &str, text: &str) -> OverlaySet {
    let mut result = OverlaySet::default();
    insert_overlay(workspace, &mut result, path, module, text);
    result
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

fn labels(completion: &bg3_ide::CompletionList) -> Vec<&str> {
    completion
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect()
}

#[test]
fn follows_annotated_helper_result_into_an_unannotated_local_member() {
    let path = Path::new("/synthetic/Project/Helpers.khn");
    let text = "---@class Weapon\n---@field damage integer\n---@return Weapon\nfunction make_weapon() end\nlocal weapon = make_weapon()\nlocal partial = weapon.da\nlocal full = weapon.damage\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let open = overlays(&workspace, path, "Project", text);

    let completion = workspace.completion(path, position(text, "weapon.da", true), &open, false);
    assert!(labels(&completion).contains(&"damage"));

    let hover = workspace
        .language_hover(path, position(text, "damage", false), &open)
        .expect("inferred member hover");
    assert!(hover.contains("damage"));
    assert!(hover.contains("integer"));

    let declarations = bg3_index::parse_thoth_file(text).expect("facts");
    let field = &declarations.annotations.classes[0].fields[0];
    assert_eq!(
        workspace.definition_locations_at(path, position(text, "damage", false), &open),
        vec![SourceLocation {
            path: path.to_path_buf(),
            range: field.name_range,
        }]
    );
}

#[test]
fn inferred_helper_return_union_exposes_only_common_fields() {
    let path = Path::new("/synthetic/Project/Union.khn");
    let text = "---@class Weapon\n---@field common string\n---@field damage integer\n---@class Armor\n---@field common string\n---@field armor_only boolean\n---@type Weapon\nlocal weapon = nil\n---@type Armor\nlocal armor = nil\nfunction choose(flag)\n  if flag then\n    return weapon\n  end\n  return armor\nend\nlocal value = choose(true)\nlocal partial = value.co\nlocal common = value.common\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let open = overlays(&workspace, path, "Project", text);

    let completion = workspace.completion(path, position(text, "value.co", true), &open, false);
    let labels = labels(&completion);
    assert!(labels.contains(&"common"));
    assert!(!labels.contains(&"damage"));
    assert!(!labels.contains(&"armor_only"));

    let definitions =
        workspace.definition_locations_at(path, position(text, "common", false), &open);
    assert_eq!(definitions.len(), 2);
}

#[test]
fn condition_result_and_inferred_result_expose_boolean_result_without_definition() {
    let path = Path::new("/synthetic/Project/Condition.khn");
    let text = "function make_result()\n  return ConditionResult(false)\nend\nlocal direct = ConditionResult(true)\nlocal inferred = make_result()\nlocal direct_prefix = direct.Re\nlocal inferred_prefix = inferred.Re\nlocal direct_result = direct.Result\nlocal inferred_result = inferred.Result\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), text)],
    );
    let workspace = workspace(vec![project]);
    let open = overlays(&workspace, path, "Project", text);

    for needle in ["direct.Re", "inferred.Re"] {
        let completion = workspace.completion(path, position(text, needle, true), &open, false);
        assert!(
            labels(&completion).contains(&"Result"),
            "missing Result for {needle}"
        );
    }

    let hover = workspace
        .language_hover(path, position(text, "inferred.Result", false), &open)
        .expect("ConditionResult field hover");
    assert!(hover.contains("Result"));
    assert!(hover.contains("boolean"));
    let facts = bg3_index::parse_thoth_file(text).expect("facts");
    let expression_range = facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == "inferred.Result")
        .map(|fact| fact.range)
        .expect("inferred expression range");
    assert_eq!(hover.range, Some(expression_range));
    let result_range = facts
        .expression_facts
        .iter()
        .find_map(|fact| match &fact.kind {
            bg3_index::ThothExpressionKind::MemberAccess(segments) => segments
                .iter()
                .find(|segment| segment.text == "Result" && fact.text == "inferred.Result")
                .map(|segment| segment.range),
            _ => None,
        })
        .expect("Result member range");
    let member_hover = workspace
        .language_hover(path, position(text, "Result", false), &open)
        .expect("ConditionResult member hover");
    assert_eq!(member_hover.range, Some(result_range));

    assert!(
        workspace
            .definition_locations_at(path, position(text, "inferred.Result", false), &open)
            .is_empty()
    );
}

#[test]
fn unknown_and_ambiguous_helpers_do_not_produce_member_evidence() {
    let path = Path::new("/synthetic/Project/Query.khn");
    let unknown = "local value = MissingHelper()\nlocal partial = value.da\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[(path.to_str().unwrap(), unknown)],
    );
    let unknown_workspace = workspace(vec![project]);
    let open = overlays(&unknown_workspace, path, "Project", unknown);
    assert!(
        unknown_workspace
            .completion(path, position(unknown, "value.da", true), &open, false)
            .items
            .is_empty()
    );
    assert!(
        unknown_workspace
            .language_hover(path, position(unknown, "value.da", false), &open)
            .is_none()
    );

    let ambiguous = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[
            (
                "/synthetic/Project/A.khn",
                "---@return ConditionResult\nfunction Shared() end\n",
            ),
            (
                "/synthetic/Project/B.khn",
                "---@return ConditionResult\nfunction Shared() end\n",
            ),
            (
                path.to_str().unwrap(),
                "local value = Shared()\nlocal partial = value.Re\n",
            ),
        ],
    );
    let ambiguous_workspace = workspace(vec![ambiguous]);
    let open = overlays(
        &ambiguous_workspace,
        path,
        "Project",
        "local value = Shared()\nlocal partial = value.Re\n",
    );
    assert!(
        ambiguous_workspace
            .completion(
                path,
                position(
                    "local value = Shared()\nlocal partial = value.Re\n",
                    "value.Re",
                    true
                ),
                &open,
                false
            )
            .items
            .is_empty()
    );
}

#[test]
fn overlay_replaces_helper_and_member_type_evidence() {
    let types_path = Path::new("/synthetic/Project/Types.khn");
    let query_path = Path::new("/synthetic/Project/Query.khn");
    let disk_types =
        "---@class Weapon\n---@field damage integer\n---@return Weapon\nfunction make() end\n";
    let query = "local value = make()\nlocal partial = value.da\n";
    let project = module(
        "Project",
        "/synthetic/Project",
        ModuleRole::Project,
        &[
            (types_path.to_str().unwrap(), disk_types),
            (query_path.to_str().unwrap(), query),
        ],
    );
    let workspace = workspace(vec![project]);
    let mut open = overlays(&workspace, query_path, "Project", query);
    let overlay_types =
        "---@class Armor\n---@field armor_only boolean\n---@return Armor\nfunction make() end\n";
    insert_overlay(&workspace, &mut open, types_path, "Project", overlay_types);

    let completion =
        workspace.completion(query_path, position(query, "value.da", true), &open, false);
    let initial_labels = labels(&completion);
    assert!(!initial_labels.contains(&"damage"));

    let armor_query = "local value = make()\nlocal partial = value.ar\n";
    insert_overlay(&workspace, &mut open, query_path, "Project", armor_query);
    let completion = workspace.completion(
        query_path,
        position(armor_query, "value.ar", true),
        &open,
        false,
    );
    assert!(labels(&completion).contains(&"armor_only"));
}
