//! Raw JSON-RPC coverage for LSP position encoding negotiation and conversion.
//!
//! The Neovim integration tests cannot expose these bugs because the editor
//! converts positions before sending them to the server.  These tests send
//! UTF-16 positions directly over stdio and use an emoji to distinguish UTF-8
//! byte columns from UTF-16 code-unit columns.

mod support;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{LspClient, TestWorkspace};
use url::Url;

fn capabilities(position_encodings: Option<&[&str]>) -> Value {
    let mut capabilities = json!({
        "textDocument": {
            "completion": { "completionItem": { "snippetSupport": false } }
        }
    });
    if let Some(position_encodings) = position_encodings {
        capabilities["general"] = json!({ "positionEncodings": position_encodings });
    }
    capabilities
}

fn initialized_client(position_encodings: Option<&[&str]>) -> (TestWorkspace, LspClient, Value) {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    let initialize = client
        .initialize(capabilities(position_encodings))
        .expect("initialize");
    client.initialized().expect("initialized");
    (workspace, client, initialize)
}

fn initialized_hover_client(format: &str) -> (TestWorkspace, LspClient) {
    initialized_hover_client_with_formats(&[format], "utf-16")
}

fn initialized_hover_client_with_encoding(
    format: &str,
    position_encoding: &str,
) -> (TestWorkspace, LspClient) {
    initialized_hover_client_with_formats(&[format], position_encoding)
}

fn initialized_hover_client_with_formats(
    formats: &[&str],
    position_encoding: &str,
) -> (TestWorkspace, LspClient) {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    client
        .initialize(json!({
            "general": { "positionEncodings": [position_encoding] },
            "textDocument": { "hover": { "contentFormat": formats } }
        }))
        .expect("initialize");
    client.initialized().expect("initialized");
    (workspace, client)
}

fn initialized_hover_client_without_content_format() -> (TestWorkspace, LspClient) {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    client
        .initialize(json!({
            "general": { "positionEncodings": ["utf-16"] },
            "textDocument": {}
        }))
        .expect("initialize");
    client.initialized().expect("initialized");
    (workspace, client)
}

fn document_uri(workspace: &TestWorkspace) -> String {
    Url::from_file_path(
        workspace
            .document()
            .canonicalize()
            .expect("canonical document"),
    )
    .expect("document URI")
    .to_string()
}

fn osiris_uri(workspace: &TestWorkspace) -> (std::path::PathBuf, String) {
    let path = workspace
        .root()
        .join("Mods/MyMod/Story/RawFiles/Goals/Tracking.txt");
    fs::create_dir_all(path.parent().expect("Osiris parent")).expect("create Osiris parent");
    fs::write(&path, "Version 1\n").expect("create Osiris document");
    let uri = Url::from_file_path(path.canonicalize().expect("canonical Osiris document"))
        .expect("Osiris URI")
        .to_string();
    (path, uri)
}

fn utf16_column(line: &str, needle: &str) -> u32 {
    let offset = line.find(needle).expect("needle in synthetic line");
    u32::try_from(line[..offset].encode_utf16().count()).expect("UTF-16 column fits")
}

fn utf8_column(line: &str, needle: &str) -> u32 {
    u32::try_from(line.find(needle).expect("needle in synthetic line")).expect("UTF-8 column fits")
}

fn open(client: &mut LspClient, workspace: &TestWorkspace, version: i32, text: &str) {
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": document_uri(workspace),
                    "languageId": "bg3_stats",
                    "version": version,
                    "text": text
                }
            }),
        )
        .expect("didOpen");
}

fn change(client: &mut LspClient, workspace: &TestWorkspace, version: i32, text: &str) {
    client
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": document_uri(workspace),
                    "version": version
                },
                "contentChanges": [{ "text": text }]
            }),
        )
        .expect("didChange");
}

fn wait_for_index(client: &mut LspClient) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = client
            .request_result(
                "workspace/executeCommand",
                json!({ "command": "bg3.indexInfo", "arguments": [] }),
            )
            .expect("indexInfo request");
        if info["generation"].as_u64().is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the synthetic index did not become ready"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn notification(client: &mut LspClient, method: &str) -> Value {
    loop {
        let message = client.next_message().expect("server notification");
        if message["method"] == method {
            return message;
        }
    }
}

#[test]
fn hover_returns_the_source_range_and_negotiated_markdown() {
    let (workspace, mut client) = initialized_hover_client("markdown");
    let text = "new entry \"WIRE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n";
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": 8 }
            }),
        )
        .expect("hover request");
    assert_eq!(hover["contents"]["kind"], "markdown");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| { value.contains("**Stats property** `Enabled`") })
    );
    assert_eq!(
        hover["range"]["start"],
        json!({ "line": 2, "character": 6 })
    );
    assert_eq!(hover["range"]["end"], json!({ "line": 2, "character": 13 }));

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn hover_degrades_to_plaintext_when_markdown_is_not_supported() {
    let (workspace, mut client) = initialized_hover_client("plaintext");
    let text = "new entry \"WIRE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n";
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": 8 }
            }),
        )
        .expect("hover request");
    assert_eq!(hover["contents"]["kind"], "plaintext");
    let value = hover["contents"]["value"].as_str().expect("plain text");
    assert!(value.contains("Stats property Enabled"), "{value}");
    assert!(!value.contains("**"), "{value}");
    assert!(!value.contains('`'), "{value}");

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn hover_defaults_to_plaintext_when_content_format_is_omitted() {
    let (workspace, mut client) = initialized_hover_client_without_content_format();
    let text = "new entry \"WIRE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n";
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": 8 }
            }),
        )
        .expect("hover request");
    assert_eq!(hover["contents"]["kind"], "plaintext");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("Stats property Enabled"))
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn hover_uses_the_first_supported_format_in_client_preference_order() {
    let (workspace, mut plaintext_client) =
        initialized_hover_client_with_formats(&["plaintext", "markdown"], "utf-16");
    let text = "new entry \"WIRE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n";
    wait_for_index(&mut plaintext_client);
    open(&mut plaintext_client, &workspace, 1, text);
    let plaintext_hover = plaintext_client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": 8 }
            }),
        )
        .expect("plaintext-preferred hover request");
    assert_eq!(plaintext_hover["contents"]["kind"], "plaintext");
    plaintext_client.shutdown().expect("shutdown");
    assert_eq!(plaintext_client.exit().expect("exit"), Some(0));

    let (workspace, mut markdown_client) =
        initialized_hover_client_with_formats(&["markdown", "plaintext"], "utf-16");
    wait_for_index(&mut markdown_client);
    open(&mut markdown_client, &workspace, 1, text);
    let markdown_hover = markdown_client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": 8 }
            }),
        )
        .expect("markdown-preferred hover request");
    assert_eq!(markdown_hover["contents"]["kind"], "markdown");
    markdown_client.shutdown().expect("shutdown");
    assert_eq!(markdown_client.exit().expect("exit"), Some(0));
}

#[test]
fn plaintext_hover_preserves_stats_declaration_preview_contents() {
    let (workspace, mut client) = initialized_hover_client("plaintext");
    let text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"UnlockSpell(Target_Test)\"\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 0, "character": 13 }
            }),
        )
        .expect("declaration hover request");
    assert_eq!(hover["contents"]["kind"], "plaintext");
    let value = hover["contents"]["value"].as_str().expect("plain text");
    assert!(value.contains("new entry \"CONSUMER\""), "{value}");
    assert!(
        value.contains("data \"Boosts\" \"UnlockSpell(Target_Test)\""),
        "{value}"
    );
    assert!(!value.contains("```"), "{value}");
    assert!(!value.contains("**"), "{value}");
    assert!(!value.contains('`'), "{value}");

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn hover_returns_utf16_range_after_an_emoji() {
    let (workspace, mut client) = initialized_hover_client_with_encoding("markdown", "utf-16");
    let text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(Target_Test)\"\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let line = text.lines().nth(2).expect("hover line");
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": {
                    "line": 2,
                    "character": utf16_column(line, "Target_Test")
                }
            }),
        )
        .expect("hover request");
    let start = utf16_column(line, "Target_Test");
    let end = start + u32::try_from("Target_Test".encode_utf16().count()).expect("range fits");
    assert_eq!(
        hover["range"],
        json!({
            "start": { "line": 2, "character": start },
            "end": { "line": 2, "character": end }
        })
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn hover_returns_utf8_range_after_an_emoji() {
    let (workspace, mut client) = initialized_hover_client_with_encoding("markdown", "utf-8");
    let text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(Target_Test)\"\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let line = text.lines().nth(2).expect("hover line");
    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": {
                    "line": 2,
                    "character": utf8_column(line, "Target_Test")
                }
            }),
        )
        .expect("hover request");
    let start = utf8_column(line, "Target_Test");
    let end = start + u32::try_from("Target_Test".len()).expect("range fits");
    assert_eq!(
        hover["range"],
        json!({
            "start": { "line": 2, "character": start },
            "end": { "line": 2, "character": end }
        })
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn document_symbols_keep_full_ranges_and_encode_selection_ranges() {
    let text = concat!(
        "new entry \"😀WIRE\"\n",
        "type \"PassiveData\"\n",
        "data \"Enabled\" \"Yes\"\n",
    );
    let line = text.lines().next().expect("entry header");
    let selection_start_utf8 = utf8_column(line, "😀WIRE");
    let selection_end_utf8 =
        selection_start_utf8 + u32::try_from("😀WIRE".len()).expect("selection range fits");
    let selection_start_utf16 = utf16_column(line, "😀WIRE");
    let selection_end_utf16 = selection_start_utf16
        + u32::try_from("😀WIRE".encode_utf16().count()).expect("selection range fits");

    for (encoding, selection_start, selection_end) in [
        ("utf-8", selection_start_utf8, selection_end_utf8),
        ("utf-16", selection_start_utf16, selection_end_utf16),
    ] {
        let (workspace, mut client, _) = initialized_client(Some(&[encoding]));
        wait_for_index(&mut client);
        open(&mut client, &workspace, 1, text);
        let symbols = client
            .request_result(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": document_uri(&workspace) } }),
            )
            .expect("document symbol request");
        let symbol = symbols
            .as_array()
            .and_then(|symbols| symbols.iter().find(|symbol| symbol["name"] == "😀WIRE"))
            .expect("WIRE document symbol");
        assert_eq!(
            symbol["range"],
            json!({
                "start": { "line": 0, "character": 0 },
                "end": { "line": 3, "character": 0 }
            })
        );
        assert_eq!(
            symbol["selectionRange"],
            json!({
                "start": { "line": 0, "character": selection_start },
                "end": { "line": 0, "character": selection_end }
            })
        );

        client.shutdown().expect("shutdown");
        assert_eq!(client.exit().expect("exit"), Some(0));
    }
}

#[test]
fn selects_utf16_when_the_client_offers_only_utf16() {
    let (workspace, mut client, initialize) = initialized_client(Some(&["utf-16"]));
    assert_eq!(initialize["capabilities"]["positionEncoding"], "utf-16");
    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn defaults_to_utf16_when_the_client_omits_position_encodings() {
    let (workspace, mut client, initialize) = initialized_client(None);
    assert_eq!(initialize["capabilities"]["positionEncoding"], "utf-16");
    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn advertises_comma_as_a_signature_help_retrigger_character() {
    let (workspace, mut client, initialize) = initialized_client(Some(&["utf-16"]));
    assert_eq!(
        initialize["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
        json!(["(", ","])
    );
    assert_eq!(
        initialize["capabilities"]["signatureHelpProvider"]["retriggerCharacters"],
        json!([","])
    );
    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn converts_utf16_input_for_definition_after_an_emoji() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let definition_text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(Target_Test)\"\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, definition_text);
    let line = definition_text.lines().nth(2).expect("definition line");
    let definition = client
        .request_result(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": {
                    "line": 2,
                    "character": utf16_column(line, "Target_Test")
                }
            }),
        )
        .expect("definition request");
    assert!(
        definition.as_array().is_some_and(|locations| {
            locations.iter().any(|location| {
                location["uri"]
                    .as_str()
                    .is_some_and(|uri| uri.ends_with("/Target.stats"))
            })
        }),
        "emoji-prefixed definition did not resolve: {definition}"
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn converts_utf16_input_for_completion_after_an_emoji() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let initial_text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(Target_T\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, initial_text);
    let completion_text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(Target_T\n",
    );
    change(&mut client, &workspace, 2, completion_text);
    let line = completion_text.lines().nth(2).expect("completion line");
    let cursor = u32::try_from(line.encode_utf16().count()).expect("cursor fits");
    let completion = client
        .request_result(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": document_uri(&workspace) },
                "position": { "line": 2, "character": cursor }
            }),
        )
        .expect("completion request");
    let item = completion["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["label"] == "Target_Test"))
        .expect("Target_Test completion");
    assert_eq!(
        item["textEdit"]["range"]["start"]["character"],
        utf16_column(line, "Target_T")
    );
    assert_eq!(item["textEdit"]["range"]["end"]["character"], cursor);

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn converts_utf8_diagnostic_ranges_to_utf16_after_an_emoji() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let text = concat!(
        "new entry \"CONSUMER\"\n",
        "type \"PassiveData\"\n",
        "data \"Boosts\" \"'😀';UnlockSpell(MISSING_REF)\"\n",
    );
    wait_for_index(&mut client);
    open(&mut client, &workspace, 1, text);
    let message = notification(&mut client, "textDocument/publishDiagnostics");
    let diagnostic = message["params"]["diagnostics"]
        .as_array()
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic["code"] == "unresolved-reference")
        })
        .expect("unresolved-reference diagnostic");
    let line = text.lines().nth(2).expect("diagnostic line");
    let start = utf16_column(line, "MISSING_REF");
    assert_eq!(diagnostic["range"]["start"]["character"], start);
    assert_eq!(
        diagnostic["range"]["end"]["character"],
        start + u32::try_from("MISSING_REF".encode_utf16().count()).expect("range fits")
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn tracks_osiris_head_variables_through_lsp_navigation() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let (path, uri) = osiris_uri(&workspace);
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "Died(_Caster)\n",
        "AND\n",
        "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _BonusActionPoints)\n",
        "THEN\n",
        "DB_Use(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    wait_for_index(&mut client);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_osiris",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .expect("open Osiris document");

    let hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 33 }
            }),
        )
        .expect("Osiris variable hover");
    let hover_value = hover["contents"]["value"].as_str().expect("hover markdown");
    assert!(
        hover_value.contains("Osiris variable _Caster"),
        "{hover_value}"
    );
    assert!(hover_value.contains("Type: CHARACTER"), "{hover_value}");
    assert!(!hover_value.contains("Bound by:"), "{hover_value}");
    assert!(!hover_value.contains("Binding:"), "{hover_value}");
    assert!(!hover_value.contains("Evidence:"), "{hover_value}");
    assert_eq!(
        hover["range"],
        json!({
            "start": { "line": 7, "character": 31 },
            "end": { "line": 7, "character": 38 }
        })
    );

    let definition = client
        .request_result(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 33 }
            }),
        )
        .expect("Osiris variable definition");
    assert_eq!(
        definition,
        json!([{
            "uri": uri,
            "range": {
                "start": { "line": 5, "character": 5 },
                "end": { "line": 5, "character": 12 }
            }
        }])
    );

    let references = client
        .request_result(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 33 },
                "context": { "includeDeclaration": true }
            }),
        )
        .expect("Osiris variable references");
    assert_eq!(references.as_array().map(Vec::len), Some(3));
    for (line, start, end) in [(5, 5, 12), (7, 31, 38), (9, 7, 14)] {
        assert!(
            references
                .as_array()
                .is_some_and(|items| items.iter().any(|item| {
                    item["uri"] == uri
                        && item["range"]
                            == json!({
                                "start": { "line": line, "character": start },
                                "end": { "line": line, "character": end }
                            })
                })),
            "missing Osiris reference at line {line}: {references}"
        );
    }

    let references_without_declaration = client
        .request_result(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 33 },
                "context": { "includeDeclaration": false }
            }),
        )
        .expect("Osiris references without declaration");
    assert_eq!(
        references_without_declaration.as_array().map(Vec::len),
        Some(2)
    );

    let bonus_hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 70 }
            }),
        )
        .expect("Osiris query output variable hover");
    let bonus_value = bonus_hover["contents"]["value"]
        .as_str()
        .expect("Osiris query output hover markdown");
    assert!(
        bonus_value.contains("Osiris variable _BonusActionPoints"),
        "{bonus_value}"
    );
    assert!(bonus_value.contains("Type: REAL"), "{bonus_value}");
    assert!(!bonus_value.contains("Bound by:"), "{bonus_value}");
    assert!(!bonus_value.contains("Binding:"), "{bonus_value}");
    assert!(!bonus_value.contains("Evidence:"), "{bonus_value}");

    let callable_hover = client
        .request_result(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 10 }
            }),
        )
        .expect("Osiris engine callable hover");
    let callable_value = callable_hover["contents"]["value"]
        .as_str()
        .expect("Osiris engine callable hover markdown");
    assert!(
        callable_value.contains("Osiris engine query GetActionResourceValuePersonal/4"),
        "{callable_value}"
    );
    assert!(
        callable_value.contains(
            "GetActionResourceValuePersonal(\n    [in] CHARACTER _Player,\n    [in] STRING _ResourceName,\n    [in] INTEGER _ResourceLevel,\n    [out] REAL _Amount\n)"
        ),
        "{callable_value}"
    );

    let query_line = text.lines().nth(7).expect("Osiris query line");
    let first_argument_end = query_line.find(',').expect("Osiris query comma") + 1;
    let signature = client
        .request_result(
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": first_argument_end }
            }),
        )
        .expect("Osiris engine signature help");
    assert_eq!(
        signature["signatures"][0]["label"],
        json!(
            "GetActionResourceValuePersonal([in] CHARACTER _Player, [in] STRING _ResourceName, [in] INTEGER _ResourceLevel, [out] REAL _Amount)"
        )
    );
    assert_eq!(signature["activeParameter"], json!(1));
    assert!(
        signature["signatures"][0]["documentation"]["value"]
            .as_str()
            .is_some_and(|documentation| documentation.contains("Returns a character's value"))
    );
    let bonus_definition = client
        .request_result(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 7, "character": 70 }
            }),
        )
        .expect("Osiris query output variable definition");
    assert_eq!(
        bonus_definition,
        json!([{
            "uri": uri,
            "range": {
                "start": { "line": 7, "character": 63 },
                "end": { "line": 7, "character": 81 }
            }
        }])
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(path);
}

#[test]
fn document_symbols_use_identifier_selection_ranges_for_osiris_declarations() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let (path, uri) = osiris_uri(&workspace);
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "DB_Tracked((CHARACTER)CHARACTERGUID_11111111-1111-1111-1111-111111111111, 1);\n",
        "KBSECTION\n",
        "PROC\n",
        "DoWork((INTEGER)_Value)\n",
        "THEN\n",
        "DB_Tracked(_Value, 1);\n",
        "QRY\n",
        "ReadWork((INTEGER)_Value)\n",
        "AND\n",
        "DB_Tracked(_Value, 1)\n",
        "THEN\n",
        "DB_Tracked(_Value, 1);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    wait_for_index(&mut client);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_osiris",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .expect("open Osiris document");

    let symbols = client
        .request_result(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )
        .expect("Osiris document symbol request");
    let symbols = symbols.as_array().expect("nested document symbols");
    assert_eq!(
        symbols
            .iter()
            .map(|symbol| symbol["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Tracking", "DoWork", "ReadWork", "DB_Tracked"]
    );

    let goal = symbols
        .iter()
        .find(|symbol| symbol["name"] == "Tracking")
        .expect("goal symbol");
    assert_eq!(goal["range"]["start"], json!({ "line": 0, "character": 0 }));
    assert_eq!(
        goal["selectionRange"],
        json!({
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 9 }
        })
    );
    assert!(goal["range"]["end"]["line"].as_u64().unwrap() > 0);

    let database = symbols
        .iter()
        .find(|symbol| symbol["name"] == "DB_Tracked")
        .expect("database symbol");
    assert_eq!(
        database["range"]["start"],
        json!({ "line": 3, "character": 0 })
    );
    assert_eq!(
        database["selectionRange"],
        json!({
            "start": { "line": 3, "character": 0 },
            "end": { "line": 3, "character": 10 }
        })
    );
    assert!(database["range"]["end"]["character"].as_u64().unwrap() > 10);

    let procedure = symbols
        .iter()
        .find(|symbol| symbol["name"] == "DoWork")
        .expect("procedure symbol");
    assert_eq!(
        procedure["selectionRange"],
        json!({
            "start": { "line": 6, "character": 0 },
            "end": { "line": 6, "character": 6 }
        })
    );
    assert!(procedure["range"]["end"]["line"].as_u64().unwrap() > 6);

    let query = symbols
        .iter()
        .find(|symbol| symbol["name"] == "ReadWork")
        .expect("query symbol");
    assert_eq!(
        query["selectionRange"],
        json!({
            "start": { "line": 10, "character": 0 },
            "end": { "line": 10, "character": 8 }
        })
    );
    assert!(query["range"]["end"]["line"].as_u64().unwrap() > 10);

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(path);
}

#[test]
fn provides_osiris_signature_help_for_later_arguments() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let (_path, uri) = osiris_uri(&workspace);
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell((CHARACTER)_Caster, \"Target_MainHandAttack\", -, -, _StoryActionID)\n",
        "AND\n",
        "GetActionResourceValuePersonal(_Caster, \"BonusActionPoint\", 0, _BonusActionPoints)\n",
        "THEN\n",
        "DB_Use(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    wait_for_index(&mut client);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_osiris",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .expect("open Osiris document");

    let using_spell_line = text.lines().nth(5).expect("UsingSpell line");
    let first_comma = using_spell_line.find(',').expect("first argument comma");
    let signature = client
        .request_result(
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": {
                    "line": 5,
                    "character": u32::try_from(first_comma + 2).expect("position fits")
                }
            }),
        )
        .expect("signature help after casted first argument");
    assert_eq!(signature["activeParameter"], json!(1));
    assert_eq!(
        signature["signatures"][0]["label"]
            .as_str()
            .and_then(|label| label.split_once('('))
            .map(|(name, _)| name),
        Some("UsingSpell")
    );

    for (line_number, function, expected_parameters) in [
        (5, "UsingSpell", &[1_u32, 2, 3, 4][..]),
        (7, "GetActionResourceValuePersonal", &[1_u32, 2, 3][..]),
    ] {
        let line = text.lines().nth(line_number).expect("signature line");
        let mut comma_count = 0;
        for (index, character) in line.char_indices() {
            if character != ',' {
                continue;
            }
            comma_count += 1;
            let argument_start = line[index + 1..]
                .char_indices()
                .find(|(_, character)| !character.is_whitespace())
                .map(|(offset, _)| index + 1 + offset)
                .expect("argument after comma");
            let argument_end = line[argument_start..]
                .bytes()
                .position(|byte| byte == b',' || byte == b')')
                .map_or(line.len(), |offset| argument_start + offset);
            // Put the cursor inside the argument when it has enough content;
            // short literals such as `-` and `0` use their end position.
            let character = (argument_start + 3).min(argument_end);
            assert!(character > argument_start, "cursor must be inside argument");
            let signature = client
                .request_result(
                    "textDocument/signatureHelp",
                    json!({
                        "textDocument": { "uri": uri },
                        "position": {
                            "line": line_number,
                            "character": u32::try_from(character).expect("position fits")
                        }
                    }),
                )
                .expect("Osiris signature help");
            assert_eq!(
                signature["activeParameter"],
                json!(expected_parameters[comma_count - 1])
            );
            assert_eq!(
                signature["signatures"][0]["label"]
                    .as_str()
                    .and_then(|label| label.split_once('('))
                    .map(|(name, _)| name),
                Some(function)
            );
        }
        assert_eq!(
            comma_count,
            expected_parameters.len(),
            "{function} should have the expected number of separators"
        );
    }

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}

#[test]
fn provides_osiris_signature_help_across_lines_and_lexical_noise() {
    let (workspace, mut client, _) = initialized_client(Some(&["utf-16"]));
    let (_path, uri) = osiris_uri(&workspace);
    let text = concat!(
        "Version 1\n",
        "SubGoalCombiner SGC_AND\n",
        "INITSECTION\n",
        "KBSECTION\n",
        "IF\n",
        "UsingSpell( // FakeCall(1, 2)\n",
        "    /* FakeCall(\"ignored, comma\") */ (CHARACTER)_Caster,\n",
        "    \"Target, (not a call)\" /* 😄 comma, */,\n",
        "    -, -, _StoryActionID)\n",
        "THEN\n",
        "DB_Use(_Caster);\n",
        "EXITSECTION\n",
        "ENDEXITSECTION\n",
    );
    wait_for_index(&mut client);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_osiris",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .expect("open Osiris document");

    let opening_line = text.lines().nth(5).expect("UsingSpell line");
    let opening_parenthesis = opening_line.find('(').expect("opening parenthesis") + 1;
    let opening = client
        .request_result(
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": {
                    "line": 5,
                    "character": opening_parenthesis
                }
            }),
        )
        .expect("signature help after opening parenthesis");
    assert_eq!(opening["activeParameter"], json!(0));
    assert_eq!(
        opening["signatures"][0]["label"]
            .as_str()
            .and_then(|label| label.split_once('('))
            .map(|(name, _)| name),
        Some("UsingSpell")
    );

    let comment_cursor = opening_line.find("FakeCall").expect("comment call") + 4;
    let in_comment = client
        .request_result(
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": {
                    "line": 5,
                    "character": comment_cursor
                }
            }),
        )
        .expect("signature help in comment");
    assert!(
        in_comment.is_null(),
        "comments must not create signature context"
    );

    let second_argument_line = text.lines().nth(7).expect("second argument line");
    let separator = second_argument_line
        .rfind(',')
        .expect("separator after second argument");
    let utf16_cursor = second_argument_line[..separator + 1].encode_utf16().count();
    let later_argument = client
        .request_result(
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": {
                    "line": 7,
                    "character": u32::try_from(utf16_cursor).expect("position fits")
                }
            }),
        )
        .expect("signature help after multiline argument");
    assert_eq!(later_argument["activeParameter"], json!(2));
    assert_eq!(
        later_argument["signatures"][0]["label"]
            .as_str()
            .and_then(|label| label.split_once('('))
            .map(|(name, _)| name),
        Some("UsingSpell")
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
    drop(workspace);
}
