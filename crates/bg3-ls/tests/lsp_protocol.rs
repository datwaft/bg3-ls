mod support;

use serde_json::json;

use support::{LspClient, TestWorkspace};

#[test]
fn raw_stdio_client_can_initialize_and_shutdown() {
    let workspace = TestWorkspace::new();
    let mut client = LspClient::spawn(&workspace);
    let initialize = client
        .initialize(json!({"general": {"positionEncodings": ["utf-8"]}}))
        .expect("initialize response");
    assert_eq!(initialize["serverInfo"]["name"], "bg3-ls");
    let sync = &initialize["capabilities"]["textDocumentSync"];
    assert_eq!(sync["openClose"], true);
    assert_eq!(sync["change"], 1);
    assert_eq!(sync["save"]["includeText"], true);

    client.initialized().expect("initialized notification");
    client.shutdown().expect("shutdown response");
    assert_eq!(client.exit().expect("wait for clean exit"), Some(0));
}
