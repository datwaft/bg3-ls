//! Raw JSON-RPC lifecycle regression tests.

mod support;

use std::process::Command;
use std::time::Duration;

use serde_json::json;

use support::{LspClient, TestWorkspace};

const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

fn client() -> (TestWorkspace, LspClient) {
    let workspace = TestWorkspace::new();
    let client = LspClient::spawn(&workspace);
    (workspace, client)
}

fn capabilities() -> serde_json::Value {
    json!({
        "window": {"workDoneProgress": true},
        "general": {"positionEncodings": ["utf-16", "utf-8"]}
    })
}

#[test]
fn exits_successfully_after_shutdown() {
    let (_workspace, mut server) = client();
    server
        .initialize(capabilities())
        .expect("initialize response");
    server.initialized().expect("initialized notification");
    server.shutdown().expect("shutdown response");
    server
        .notify("exit", serde_json::Value::Null)
        .expect("exit notification");

    assert_eq!(
        server.wait_for_exit(EXIT_TIMEOUT).expect("clean exit"),
        Some(0)
    );
}

#[test]
fn exits_with_failure_when_exit_precedes_shutdown() {
    let (_workspace, mut server) = client();
    server
        .notify("exit", serde_json::Value::Null)
        .expect("exit notification");

    assert_eq!(
        server.wait_for_exit(EXIT_TIMEOUT).expect("abnormal exit"),
        Some(1)
    );
}

#[cfg(unix)]
#[test]
fn exits_when_initialize_process_id_is_no_longer_alive() {
    let (_workspace, mut server) = client();
    let mut parent = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("start liveness sentinel");
    server
        .initialize_with_process_id(capabilities(), Some(parent.id()))
        .expect("initialize response");
    parent.kill().expect("kill liveness sentinel");
    parent.wait().expect("wait for liveness sentinel");

    assert_eq!(
        server
            .wait_for_exit(EXIT_TIMEOUT)
            .expect("parent liveness exit"),
        Some(0)
    );
}

#[test]
fn sends_no_protocol_activity_after_shutdown_response() {
    let (_workspace, mut server) = client();
    server
        .initialize(capabilities())
        .expect("initialize response");
    server.initialized().expect("initialized notification");

    // Keep an indexing task active while shutdown is handled. A conforming
    // server must stop the task before it can emit a later progress or log
    // notification.
    let _reload = server
        .request_async(
            "workspace/executeCommand",
            json!({"command": "bg3.reload", "arguments": []}),
        )
        .expect("reload request");
    let shutdown = server
        .request_async("shutdown", serde_json::Value::Null)
        .expect("shutdown request");
    let response = server.response_for(shutdown).expect("shutdown response");
    assert!(
        response.get("result").is_some(),
        "shutdown failed: {response}"
    );
    server.clear_pending();

    assert_eq!(
        server
            .next_message_timeout(Duration::from_millis(500))
            .expect("read post-shutdown messages"),
        None,
        "server emitted protocol activity after shutdown"
    );
    server
        .notify("exit", serde_json::Value::Null)
        .expect("exit notification");
    assert_eq!(
        server.wait_for_exit(EXIT_TIMEOUT).expect("clean exit"),
        Some(0)
    );
}
