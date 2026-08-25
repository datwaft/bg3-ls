//! Raw JSON-RPC coverage for LSP position encoding negotiation and conversion.
//!
//! The Neovim integration tests cannot expose these bugs because the editor
//! converts positions before sending them to the server.  These tests send
//! UTF-16 positions directly over stdio and use an emoji to distinguish UTF-8
//! byte columns from UTF-16 code-unit columns.

mod support;

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
