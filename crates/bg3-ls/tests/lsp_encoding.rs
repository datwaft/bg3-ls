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
