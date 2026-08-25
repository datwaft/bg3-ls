//! Wire-level regression tests for LSP progress and document synchronization.
//!
//! These tests intentionally exercise the standalone process over stdio. They
//! use only the synthetic fixtures in `test/fixtures` and do not depend on an
//! editor's LSP client implementation.

mod support;

use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

use support::{LspClient, TestWorkspace};

const INDEX_TIMEOUT: Duration = Duration::from_secs(5);

fn start_client() -> (TestWorkspace, LspClient) {
    start_client_with_capabilities(json!({}))
}

fn start_client_with_capabilities(capabilities: Value) -> (TestWorkspace, LspClient) {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    client
        .initialize(capabilities)
        .expect("initialize response");
    client.initialized().expect("initialized notification");
    (workspace, client)
}

fn wait_ready(client: &mut LspClient) {
    let deadline = std::time::Instant::now() + INDEX_TIMEOUT;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the index did not become ready"
        );
        let result = client
            .request_result(
                "workspace/executeCommand",
                json!({"command": "bg3.indexInfo", "arguments": []}),
            )
            .expect("indexInfo response");
        if result["generation"].is_number() {
            client.clear_pending();
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn drain_for(client: &mut LspClient, duration: Duration) -> Vec<Value> {
    let deadline = std::time::Instant::now() + duration;
    let mut messages = Vec::new();
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match client
            .next_message_timeout(remaining)
            .expect("read server message")
        {
            Some(message) => messages.push(message),
            None => break,
        }
    }
    messages
}

fn finish(mut client: LspClient) {
    client.shutdown().expect("shutdown response");
    client.exit().expect("wait for clean exit");
}

#[test]
fn does_not_create_or_report_progress_without_client_support() {
    let (_workspace, mut client) = start_client();
    let messages = drain_for(&mut client, INDEX_TIMEOUT);
    let progress_messages: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["method"] == "window/workDoneProgress/create"
                || message["method"] == "$/progress"
        })
        .collect();
    assert!(
        progress_messages.is_empty(),
        "server sent work-done progress without client support: {progress_messages:?}"
    );
    finish(client);
}

#[test]
fn uses_the_execute_command_work_done_token() {
    let (_workspace, mut client) = start_client_with_capabilities(json!({
        "window": {"workDoneProgress": true}
    }));
    wait_ready(&mut client);

    client
        .request_result(
            "workspace/executeCommand",
            json!({
                "command": "bg3.reload",
                "arguments": [],
                "workDoneToken": "caller-token"
            }),
        )
        .expect("reload response");
    let progress: Vec<_> = drain_for(&mut client, Duration::from_millis(200))
        .into_iter()
        .filter(|message| message["method"] == "$/progress")
        .collect();
    assert!(
        progress.iter().any(|message| {
            message["params"]["token"] == "caller-token"
                && message["params"]["value"]["kind"] == "begin"
        }),
        "executeCommand did not begin progress with the caller token: {progress:?}"
    );
    assert!(
        progress.iter().any(|message| {
            message["params"]["token"] == "caller-token"
                && message["params"]["value"]["kind"] == "end"
        }),
        "executeCommand did not finish progress with the caller token: {progress:?}"
    );
    finish(client);
}

#[test]
fn advertises_full_sync_with_save_support() {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    let initialize = client.initialize(json!({})).expect("initialize response");
    let sync = &initialize["capabilities"]["textDocumentSync"];
    assert!(
        sync.is_object(),
        "textDocumentSync must advertise options: {sync}"
    );
    assert_eq!(sync["openClose"], true);
    assert_eq!(sync["change"], 1);
    assert_eq!(sync["save"], true);
    finish(client);
}

#[test]
fn ignores_stale_document_versions() {
    let (workspace, mut client) = start_client();
    wait_ready(&mut client);
    let uri = document_uri(&workspace);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_stats",
                    "version": 7,
                    "text": invalid_source()
                }
            }),
        )
        .expect("didOpen notification");
    thread::sleep(Duration::from_millis(20));
    client
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": 6},
                "contentChanges": [{"text": valid_source()}]
            }),
        )
        .expect("didChange notification");

    let diagnostics: Vec<_> = drain_for(&mut client, INDEX_TIMEOUT)
        .into_iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .collect();
    assert!(
        diagnostics
            .iter()
            .any(|message| message["params"]["version"] == 7),
        "the current document version was not diagnosed: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|message| message["params"]["version"] != 6),
        "a stale document version was published: {diagnostics:?}"
    );
    finish(client);
}

#[test]
fn does_not_publish_synthetic_zero_version_for_unversioned_save() {
    let (workspace, mut client) = start_client();
    wait_ready(&mut client);
    let uri = document_uri(&workspace);
    client
        .notify(
            "textDocument/didSave",
            json!({
                "textDocument": {"uri": uri},
                "text": invalid_source()
            }),
        )
        .expect("didSave notification");

    let diagnostics: Vec<_> = drain_for(&mut client, INDEX_TIMEOUT)
        .into_iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .collect();
    assert!(
        diagnostics
            .iter()
            .all(|message| message["params"]["version"] != 0),
        "the server invented diagnostic version zero for an unversioned save: {diagnostics:?}"
    );
    finish(client);
}

#[test]
fn stops_pending_diagnostics_after_shutdown() {
    let (workspace, mut client) = start_client();
    wait_ready(&mut client);
    let uri = document_uri(&workspace);
    client
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "bg3_stats",
                    "version": 1,
                    "text": invalid_source()
                }
            }),
        )
        .expect("didOpen notification");
    thread::sleep(Duration::from_millis(20));
    client.shutdown().expect("shutdown response");
    let after_shutdown: Vec<_> = drain_for(&mut client, Duration::from_millis(500))
        .into_iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                || message["method"] == "$/progress"
        })
        .collect();
    assert!(
        after_shutdown.is_empty(),
        "server sent protocol activity after shutdown: {after_shutdown:?}"
    );
    client.exit().expect("wait for clean exit");
}

fn invalid_source() -> &'static str {
    "new entry \"WIRE_TEST\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Maybe\"\n"
}

fn valid_source() -> &'static str {
    "new entry \"WIRE_TEST\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n"
}

fn document_uri(workspace: &TestWorkspace) -> String {
    Url::from_file_path(
        std::fs::canonicalize(workspace.document()).expect("canonical synthetic document path"),
    )
    .expect("synthetic document URI")
    .to_string()
}
