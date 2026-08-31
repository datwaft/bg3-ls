use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleVersion</key><string>4.1.1.7398727</string></dict></plist>
"#;

const STORY_HEADER: &str = "event CastedSpell((GUIDSTRING)_Caster, (STRING)_Spell) (1,0,0,1)\nquery IntegerSum([in](INTEGER)_A, [out](INTEGER)_Sum) (2,0,0,1)\n";

fn run(directory: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .args(["catalog", "generate"])
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn validates_the_checked_in_description_catalog_offline() {
    let directory = tempfile::tempdir().unwrap();
    let checked = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .args(["catalog", "check-descriptions"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert_eq!(
        String::from_utf8(checked.stdout).unwrap(),
        "description catalog is valid: 9 records (bg3-ls-osiris-descriptions-v1)\n"
    );
    assert!(checked.stderr.is_empty());
}

#[test]
fn generates_and_checks_catalog_from_game_metadata() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("Contents")).unwrap();
    fs::write(directory.path().join("Contents/Info.plist"), INFO_PLIST).unwrap();
    fs::write(directory.path().join("story_header.div"), STORY_HEADER).unwrap();

    let output = directory.path().join("catalog.rs");
    let generated = run(
        &directory,
        &[
            "--input",
            "story_header.div",
            "--game-root",
            ".",
            "--output",
            "catalog.rs",
        ],
    );
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let catalog = fs::read_to_string(&output).unwrap();
    assert!(catalog.contains("4.1.1.7398727"));
    assert!(catalog.contains("CastedSpell"));

    let checked = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .args([
            "catalog",
            "check",
            "--input",
            "story_header.div",
            "--game-root",
            ".",
            "--output",
            "catalog.rs",
        ])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn check_rejects_stale_output_without_modifying_it() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("Contents")).unwrap();
    fs::write(directory.path().join("Contents/Info.plist"), INFO_PLIST).unwrap();
    fs::write(directory.path().join("story_header.div"), STORY_HEADER).unwrap();
    fs::write(directory.path().join("catalog.rs"), "stale catalog\n").unwrap();

    let checked = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .args([
            "catalog",
            "check",
            "--input",
            "story_header.div",
            "--game-root",
            ".",
            "--output",
            "catalog.rs",
        ])
        .output()
        .unwrap();
    assert_eq!(checked.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(directory.path().join("catalog.rs")).unwrap(),
        "stale catalog\n"
    );
}

#[test]
fn failed_generation_preserves_existing_output() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("Contents")).unwrap();
    fs::write(directory.path().join("Contents/Info.plist"), INFO_PLIST).unwrap();
    fs::write(
        directory.path().join("invalid.div"),
        "query Broken((INTEGER)_Value) (2,0,0,1)\n",
    )
    .unwrap();
    fs::write(directory.path().join("catalog.rs"), "known catalog\n").unwrap();

    let generated = run(
        &directory,
        &[
            "--input",
            "invalid.div",
            "--game-root",
            ".",
            "--output",
            "catalog.rs",
        ],
    );
    assert_eq!(generated.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(directory.path().join("catalog.rs")).unwrap(),
        "known catalog\n"
    );
}
