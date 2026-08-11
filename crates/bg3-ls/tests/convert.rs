use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

const SYNTHETIC_LSX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<save>
    <version major="4" minor="0" revision="9" build="331" lslib_meta="v1,bswap_guids" lsf_version="7"/>
    <region id="root">
        <node id="root">
            <attribute id="Name" type="LSString" value="Synthetic value"/>
            <children>
                <node id="Child">
                    <attribute id="Count" type="int32" value="42"/>
                </node>
            </children>
        </node>
    </region>
</save>
"#;

fn run(directory: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
        .current_dir(directory.path())
        .arg("convert")
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn converts_synthetic_lsx_to_lsf_and_back() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("source.lsx"), SYNTHETIC_LSX).unwrap();

    let compile = run(&directory, &["source.lsx", "resource.lsf"]);
    assert_eq!(compile.status.code(), Some(0));
    assert_eq!(
        &fs::read(directory.path().join("resource.lsf")).unwrap()[..4],
        b"LSOF"
    );

    let decompile = run(&directory, &["resource.lsf", "roundtrip.lsx"]);
    assert_eq!(decompile.status.code(), Some(0));
    let roundtrip = fs::read_to_string(directory.path().join("roundtrip.lsx")).unwrap();
    assert!(roundtrip.contains(r#"<region id="root">"#));
    assert!(roundtrip.contains(r#"id="Name" type="LSString" value="Synthetic value""#));
    assert!(roundtrip.contains(r#"id="Count" type="int32" value="42""#));
}

#[test]
fn refuses_existing_and_unsupported_destinations() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("source.lsx"), SYNTHETIC_LSX).unwrap();
    fs::write(directory.path().join("existing.lsf"), b"sentinel").unwrap();

    let existing = run(&directory, &["source.lsx", "existing.lsf"]);
    assert_eq!(existing.status.code(), Some(2));
    assert!(
        String::from_utf8(existing.stderr)
            .unwrap()
            .contains("already exists")
    );
    assert_eq!(
        fs::read(directory.path().join("existing.lsf")).unwrap(),
        b"sentinel"
    );

    let unsupported = run(&directory, &["source.lsx", "output.xml"]);
    assert_eq!(unsupported.status.code(), Some(2));
    assert!(
        String::from_utf8(unsupported.stderr)
            .unwrap()
            .contains("unsupported conversion")
    );
    assert!(!directory.path().join("output.xml").exists());
}

#[test]
fn failed_forced_conversion_keeps_the_existing_destination() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("invalid.lsx"), "<not-lsx/>").unwrap();
    fs::write(directory.path().join("existing.lsf"), b"sentinel").unwrap();

    let output = run(&directory, &["invalid.lsx", "existing.lsf", "--force"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read(directory.path().join("existing.lsf")).unwrap(),
        b"sentinel"
    );
}

#[test]
fn validates_lsx_with_the_current_xml_parser_before_conversion() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("unsafe.lsx"),
        SYNTHETIC_LSX.replacen("major=\"4\"", "major=\"4\" major=\"4\"", 1),
    )
    .unwrap();

    let output = run(&directory, &["unsafe.lsx", "output.lsf"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot validate")
    );
    assert!(!directory.path().join("output.lsf").exists());
}

#[cfg(unix)]
#[test]
fn forced_conversion_preserves_destination_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.lsx");
    let destination = directory.path().join("existing.lsf");
    fs::write(&source, SYNTHETIC_LSX).unwrap();
    fs::write(&destination, b"sentinel").unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();

    let output = run(&directory, &["source.lsx", "existing.lsf", "--force"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        fs::metadata(destination).unwrap().permissions().mode() & 0o777,
        0o640
    );
}
