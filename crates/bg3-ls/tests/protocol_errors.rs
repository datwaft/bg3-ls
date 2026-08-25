mod support;

use std::fs;
use std::thread;
use std::time::Duration;

use serde_json::json;
use support::{LspClient, TestWorkspace};

#[test]
fn malformed_method_parameters_use_invalid_params() {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    client.initialize(json!({})).expect("initialize");

    let response = client
        .request("textDocument/hover", json!({}))
        .expect("hover response");
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "response: {response}"
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn failed_index_requests_use_internal_error() {
    let workspace = TestWorkspace::new();
    let cache_root = tempfile::tempdir().expect("temporary cache parent");
    let cache_path = cache_root.path().join("cache-file");
    fs::write(&cache_path, b"cache path is intentionally a file").expect("create cache file");
    let mut client = LspClient::spawn_with_cache_path(&workspace, &cache_path);
    client.initialize(json!({})).expect("initialize");
    client.initialized().expect("initialized");

    let response = client
        .request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": workspace.document_uri()},
                "position": {"line": 0, "character": 0}
            }),
        )
        .expect("hover response");
    assert_eq!(
        response["error"]["code"],
        json!(-32603),
        "response: {response}"
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}

#[test]
fn canceled_requests_return_request_cancelled() {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    client.initialize(json!({})).expect("initialize");
    let id = client
        .request_async(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": workspace.document_uri()},
                "position": {"line": 0, "character": 0}
            }),
        )
        .expect("send hover request");
    thread::sleep(Duration::from_millis(25));
    client
        .notify("$/cancelRequest", json!({"id": id}))
        .expect("cancel request");

    let response = client.response_for(id).expect("canceled response");
    assert_eq!(
        response["error"]["code"],
        json!(-32800),
        "response: {response}"
    );

    client.shutdown().expect("shutdown");
    assert_eq!(client.exit().expect("exit"), Some(0));
}
