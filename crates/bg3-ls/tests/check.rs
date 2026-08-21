use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    cache: PathBuf,
    error_file: PathBuf,
    warning_file: PathBuf,
    clean_file: PathBuf,
    osiris_error_file: PathBuf,
    thoth_error_file: PathBuf,
}

/// Creates one configured project with error, warning, and clean source files.
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures");
    let data = root.path().join("Public/MyMod/Stats/Generated/Data");
    fs::create_dir_all(&data).unwrap();
    let error_file = data.join("Passive.txt");
    let warning_file = data.join("Passive_Warning.txt");
    let clean_file = data.join("Passive_Clean.txt");
    fs::write(
        &error_file,
        "new entry \"ERROR\"\ntype \"PassiveData\"\ndata \"Bogus\" \"value\"\n",
    )
    .unwrap();
    fs::write(
        &warning_file,
        "new entry \"WARNING\"\ntype \"PassiveData\"\ndata \"Boosts\" \"ApplyStatus(MISSING_STATUS,100,1)\"\n",
    )
    .unwrap();
    fs::write(
        &clean_file,
        "new entry \"CLEAN\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n",
    )
    .unwrap();
    let osiris_data = root.path().join("Mods/MyMod/Story/RawFiles/Goals");
    fs::create_dir_all(&osiris_data).unwrap();
    let osiris_error_file = osiris_data.join("BrokenGoal.txt");
    fs::write(
        &osiris_error_file,
        "Version 1\nSubGoalCombiner SGC_AND\nINITSECTION\nKBSECTION\nIF\nBroken(\nEXITSECTION\nENDEXITSECTION\n",
    )
    .unwrap();
    let thoth_data = root.path().join("Mods/MyMod/Scripts/thoth/helpers");
    fs::create_dir_all(&thoth_data).unwrap();
    let thoth_error_file = thoth_data.join("Broken.khn");
    fs::write(&thoth_error_file, "function Broken(\n").unwrap();
    fs::write(
        root.path().join("bg3-ls.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "game_data": fixture_root.join("game"),
            "base_modules": ["Shared"],
            "project": {
                "name": "MyMod",
                "diagnostics": { "unresolved_references": "warning" }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let cache = root.path().join("cache");
    Fixture {
        root,
        cache,
        error_file,
        warning_file,
        clean_file,
        osiris_error_file,
        thoth_error_file,
    }
}

/// Runs the actual binary with an isolated cache and workspace directory.
fn check(fixture: &Fixture, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(fixture.root.path())
        .arg("--cache-dir")
        .arg(&fixture.cache)
        .arg("check")
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn emits_human_and_json_diagnostics_with_path_filters() {
    let fixture = fixture();
    let error_path = fixture.error_file.to_str().unwrap();
    let json = check(
        &fixture,
        &[error_path, "--format", "json", "--fail-on", "error"],
    );
    assert_eq!(json.status.code(), Some(1));
    let records: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["severity"], "error");
    assert_eq!(records[0]["code"], "unknown-field");
    assert_eq!(records[0]["range"]["start"]["line"], 2);

    let human = check(&fixture, &[error_path, "--fail-on", "never"]);
    assert_eq!(human.status.code(), Some(0));
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("Passive.txt:3:"));
    assert!(human.contains("error [unknown-field]"));

    let clean = check(
        &fixture,
        &[fixture.clean_file.to_str().unwrap(), "--format", "json"],
    );
    assert_eq!(clean.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&clean.stdout).unwrap(),
        serde_json::json!([])
    );
}

#[test]
fn applies_thresholds_and_default_project_scope() {
    let fixture = fixture();
    let warning_path = fixture.warning_file.to_str().unwrap();
    let below_threshold = check(
        &fixture,
        &[warning_path, "--format", "json", "--fail-on", "error"],
    );
    assert_eq!(below_threshold.status.code(), Some(0));
    let records: serde_json::Value = serde_json::from_slice(&below_threshold.stdout).unwrap();
    assert_eq!(records[0]["severity"], "warning");

    let at_threshold = check(&fixture, &[warning_path, "--fail-on", "warning"]);
    assert_eq!(at_threshold.status.code(), Some(1));

    let all_project_files = check(&fixture, &["--format", "json", "--fail-on", "never"]);
    assert_eq!(all_project_files.status.code(), Some(0));
    let records: serde_json::Value = serde_json::from_slice(&all_project_files.stdout).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 4);
    assert!(records.as_array().unwrap().iter().any(|record| {
        record["path"] == "Mods/MyMod/Story/RawFiles/Goals/BrokenGoal.txt"
            && record["code"] == "osiris-syntax-error"
    }));
    assert!(records.as_array().unwrap().iter().any(|record| {
        record["path"] == "Mods/MyMod/Scripts/thoth/helpers/Broken.khn"
            && record["code"] == "thoth-syntax-error"
    }));
}

#[test]
fn checks_an_explicit_osiris_goal() {
    let fixture = fixture();
    let output = check(
        &fixture,
        &[
            fixture.osiris_error_file.to_str().unwrap(),
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    let records: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["code"], "osiris-syntax-error");
}

#[test]
fn checks_an_explicit_thoth_file() {
    let fixture = fixture();
    let output = check(
        &fixture,
        &[
            fixture.thoth_error_file.to_str().unwrap(),
            "--format",
            "json",
        ],
    );

    assert_eq!(output.status.code(), Some(1));
    let records: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["code"], "thoth-syntax-error");
}

#[test]
fn reports_configuration_failure_with_exit_code_two() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .arg("check")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot find bg3-ls.json")
    );
}

#[test]
fn inventories_an_empty_data_directory_without_workspace_configuration() {
    let data = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .arg("inventory")
        .arg("--game-data")
        .arg(data.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "package_files": 0,
            "package_roots": 0,
            "package_parts": 0,
            "rejected_packages": 0,
            "package_rejections": {},
            "thoth_entries": 0,
            "parsed_sources": 0,
            "rejected_sources": 0,
            "source_rejections": {},
            "declared_source_bytes": 0,
            "functions": 0,
            "classes": 0,
            "aliases": 0,
            "fields": 0,
            "function_annotations": 0,
            "duplicate_functions": 0,
            "equal_priority_function_conflicts": 0,
            "modules": {},
        })
    );
}
