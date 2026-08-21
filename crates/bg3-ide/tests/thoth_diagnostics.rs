//! Synthetic diagnostics coverage for proven Thoth `ConditionResult` misuse.

use std::path::Path;
use std::sync::Arc;

use bg3_ide::{DiagnosticSeverity, OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, SchemaCatalog, SourceFile, SourceKind, parse_source,
};

fn workspace(path: &Path, text: &str) -> WorkspaceSnapshot {
    let schema = Arc::new(SchemaCatalog::default());
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
    WorkspaceSnapshot::new(
        schema,
        vec![Arc::new(ModuleIndex::new(
            ModuleSpec {
                name: "Project".into(),
                root: "/synthetic/Project".into(),
                role: ModuleRole::Project,
            },
            vec![parsed],
        ))],
        1,
        200,
        200,
    )
}

fn overlay(workspace: &WorkspaceSnapshot, path: &Path, text: &str) -> OverlaySet {
    let parsed = parse_source(
        SourceFile {
            path: path.to_path_buf(),
            kind: SourceKind::Thoth,
        },
        text,
        &workspace.schema,
        "English",
    )
    .expect("synthetic Thoth overlay");
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.to_path_buf(),
        OverlayDocument {
            module: "Project".into(),
            version: 1,
            text: text.into(),
            parsed: Arc::new(parsed),
        },
    );
    overlays
}

#[test]
fn diagnoses_proven_condition_result_boolean_misuse() {
    let path = Path::new("/synthetic/Project/Conditions.khn");
    let text = "local first = ConditionResult(false)\nlocal second = ConditionResult(true)\nlocal flag = true\n\nif first then\nend\nif flag then\nelseif first then\nend\nwhile first do\n  break\nend\n\nlocal negated = not first\nlocal combined_and = first and second\nlocal combined_or = first or second\nif (first and second) then\nend\nlocal pass_through_and = flag and first\nlocal pass_through_or = flag or first\nlocal valid_and = first & second\nlocal valid_or = first | second\nlocal valid_xor = first ~ second\nif first.Result then\nend\nlocal uncertain = flag and first\nif uncertain then\nend\nif MissingHelper() then\nend\n";
    let workspace = workspace(path, text);
    let diagnostics = workspace.diagnostics(path, &OverlaySet::default(), None);

    assert_eq!(diagnostics.len(), 7);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        vec![
            "thoth-condition-result-condition",
            "thoth-condition-result-condition",
            "thoth-condition-result-condition",
            "thoth-condition-result-boolean-operator",
            "thoth-condition-result-overloaded-operator",
            "thoth-condition-result-overloaded-operator",
            "thoth-condition-result-overloaded-operator",
        ]
    );
    assert!(diagnostics[4].message.contains("`&`"));
    assert!(diagnostics[5].message.contains("`|`"));
}

#[test]
fn condition_result_diagnostics_follow_overlays() {
    let path = Path::new("/synthetic/Project/Overlay.khn");
    let disk = "local result = ConditionResult(false)\nif result.Result then\nend\n";
    let invalid = "local result = ConditionResult(false)\nif result then\nend\n";
    let workspace = workspace(path, disk);

    assert!(
        workspace
            .diagnostics(path, &OverlaySet::default(), None)
            .is_empty()
    );
    assert_eq!(
        workspace.diagnostics(path, &overlay(&workspace, path, invalid), None)[0].code,
        "thoth-condition-result-condition"
    );
    assert!(
        workspace
            .diagnostics(path, &overlay(&workspace, path, disk), None)
            .is_empty()
    );
}
