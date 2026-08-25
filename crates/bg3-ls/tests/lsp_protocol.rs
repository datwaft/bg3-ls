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
    assert_eq!(initialize["capabilities"]["textDocumentSync"], 1);

    client.initialized().expect("initialized notification");
    client.shutdown().expect("shutdown response");
    assert_eq!(client.exit().expect("wait for clean exit"), Some(0));
}
