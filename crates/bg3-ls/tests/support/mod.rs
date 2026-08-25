//! Small raw JSON-RPC client used by the LSP protocol integration tests.
//!
//! The client deliberately speaks the stdio wire format directly.  This keeps
//! protocol tests independent of an editor client and makes every message
//! visible to the test which is exercising it.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// A temporary project containing only synthetic data and a valid LSP config.
pub struct TestWorkspace {
    root: TempDir,
    document: PathBuf,
}

impl TestWorkspace {
    /// Creates a project that uses the repository's synthetic game fixture.
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("create synthetic workspace");
        let document = root
            .path()
            .join("Public/MyMod/Stats/Generated/Data/Passive.txt");
        fs::create_dir_all(document.parent().expect("document parent")).expect("create source");
        fs::write(
            &document,
            "new entry \"WIRE\"\ntype \"PassiveData\"\ndata \"Enabled\" \"Yes\"\n",
        )
        .expect("write synthetic source");

        let game_data = fixture_root().join("game");
        fs::write(
            root.path().join("bg3-ls.json"),
            serde_json::to_vec_pretty(&json!({
                "game_data": game_data,
                "base_modules": ["Shared"],
                "project": {
                    "name": "MyMod",
                    "diagnostics": { "unresolved_references": "warning" }
                }
            }))
            .expect("serialize synthetic config"),
        )
        .expect("write synthetic config");
        Self { root, document }
    }

    /// Returns the temporary workspace root.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Returns the synthetic document used by document notifications.
    pub fn document(&self) -> &Path {
        &self.document
    }

    /// Returns the document's `file:` URI.
    pub fn document_uri(&self) -> String {
        url::Url::from_file_path(&self.document)
            .expect("document path is absolute")
            .to_string()
    }

    /// Returns the workspace root's `file:` URI.
    pub fn root_uri(&self) -> String {
        url::Url::from_directory_path(self.root.path())
            .expect("workspace path is absolute")
            .to_string()
    }
}

impl Default for TestWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures")
}

enum ReaderMessage {
    Json(Value),
    Error(String),
    Eof,
}

/// A synchronous, timeout-bounded JSON-RPC client for the server's stdio.
pub struct LspClient {
    child: Child,
    _cache: Option<TempDir>,
    stdin: ChildStdin,
    messages: Receiver<ReaderMessage>,
    pending: VecDeque<Value>,
    next_id: u64,
    workspace_uri: String,
}

impl LspClient {
    /// Starts the built `bg3-ls` binary with isolated cache and workspace state.
    pub fn spawn(workspace: &TestWorkspace) -> Self {
        let cache = tempfile::tempdir().expect("create LSP cache");
        let cache_path = cache.path().to_path_buf();
        Self::spawn_with_cache_path_inner(workspace, &cache_path, Some(cache))
    }

    /// Starts the server with a caller-provided cache path.
    ///
    /// This is used by protocol tests that need the indexer to fail after
    /// initialization, for example when the path is an existing file.
    pub fn spawn_with_cache_path(workspace: &TestWorkspace, cache_path: &Path) -> Self {
        Self::spawn_with_cache_path_inner(workspace, cache_path, None)
    }

    fn spawn_with_cache_path_inner(
        workspace: &TestWorkspace,
        cache_path: &Path,
        cache: Option<TempDir>,
    ) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bg3-ls"))
            .current_dir(workspace.root())
            .arg("--cache-dir")
            .arg(cache_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bg3-ls");
        let stdin = child.stdin.take().expect("server stdin");
        let stdout = child.stdout.take().expect("server stdout");
        let (sender, messages) = mpsc::channel();
        thread::Builder::new()
            .name("bg3-ls-json-reader".into())
            .spawn(move || read_messages(stdout, sender))
            .expect("spawn JSON reader");
        Self {
            child,
            _cache: cache,
            stdin,
            messages,
            pending: VecDeque::new(),
            next_id: 1,
            workspace_uri: workspace.root_uri(),
        }
    }

    /// Sends `initialize`, using a synthetic workspace and custom capabilities.
    pub fn initialize(&mut self, capabilities: Value) -> Result<Value, String> {
        self.initialize_with_process_id(capabilities, None)
    }

    /// Sends `initialize` with an explicit parent process ID.
    pub fn initialize_with_process_id(
        &mut self,
        capabilities: Value,
        process_id: Option<u32>,
    ) -> Result<Value, String> {
        let root = self.root_uri();
        let params = json!({
            "processId": process_id,
            "rootUri": root,
            "capabilities": capabilities,
            "workspaceFolders": [{ "uri": root, "name": "synthetic" }]
        });
        self.request_result("initialize", params)
    }

    /// Sends the post-initialization notification.
    pub fn initialized(&mut self) -> Result<(), String> {
        self.notify("initialized", json!({}))
    }

    /// Sends one JSON-RPC request and returns its complete response envelope.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;
        self.response_for(id)
    }

    /// Sends one request and returns its wire request ID without waiting.
    pub fn request_async(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;
        Ok(id)
    }

    /// Waits for one response while preserving unrelated server messages.
    pub fn response_for(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = self.receive_message()?;
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return Ok(message);
            }
            self.pending.push_back(message);
        }
    }

    /// Discards messages already received before a lifecycle boundary.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Sends one request and extracts `result`, retaining protocol errors.
    pub fn request_result(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let response = self.request(method, params)?;
        if let Some(error) = response.get("error") {
            return Err(format!("{method} failed: {error}"));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Sends a JSON-RPC notification.
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write_json(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Sends `shutdown` and returns its result.
    pub fn shutdown(&mut self) -> Result<Value, String> {
        self.request_result("shutdown", Value::Null)
    }

    /// Sends the `exit` notification and waits for the child process.
    pub fn exit(mut self) -> Result<Option<i32>, String> {
        self.notify("exit", Value::Null)?;
        self.child
            .wait()
            .map(|status| status.code())
            .map_err(|error| format!("wait for bg3-ls: {error}"))
    }

    /// Waits for the server to exit without sending an additional notification.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<i32>, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("poll bg3-ls: {error}"))?
            {
                return Ok(status.code());
            }
            if std::time::Instant::now() >= deadline {
                return Err("timed out waiting for bg3-ls to exit".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Returns one server message or `None` when no message arrives in time.
    pub fn next_message_timeout(&mut self, timeout: Duration) -> Result<Option<Value>, String> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(Some(message));
        }
        match self.messages.recv_timeout(timeout) {
            Ok(ReaderMessage::Json(message)) => {
                if message.get("method").is_some() && message.get("id").is_some() {
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    self.write_json(&json!({ "jsonrpc": "2.0", "id": id, "result": null }))?;
                }
                Ok(Some(message))
            }
            Ok(ReaderMessage::Error(error)) => Err(error),
            Ok(ReaderMessage::Eof) => Ok(None),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("JSON reader disconnected".into()),
        }
    }

    /// Returns the next unconsumed server message, including notifications.
    pub fn next_message(&mut self) -> Result<Value, String> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(message);
        }
        self.receive_message()
    }

    fn root_uri(&self) -> String {
        // The root is carried in the process command's current directory by
        // the test workspace.  A request-level URI is reconstructed from the
        // source's parent rather than stored separately in the client.
        // `workspace_uri` is supplied by the spawn wrapper below.
        self.workspace_uri.clone()
    }

    fn write_json(&mut self, value: &Value) -> Result<(), String> {
        let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", payload.len())
            .and_then(|_| self.stdin.write_all(&payload))
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("write JSON-RPC message: {error}"))
    }

    fn receive_message(&mut self) -> Result<Value, String> {
        match self.messages.recv_timeout(MESSAGE_TIMEOUT) {
            Ok(ReaderMessage::Json(message)) => {
                if message.get("method").is_some() && message.get("id").is_some() {
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    self.write_json(&json!({ "jsonrpc": "2.0", "id": id, "result": null }))?;
                }
                Ok(message)
            }
            Ok(ReaderMessage::Error(error)) => Err(error),
            Ok(ReaderMessage::Eof) => Err("bg3-ls closed stdout".into()),
            Err(RecvTimeoutError::Timeout) => Err("timed out waiting for bg3-ls".into()),
            Err(RecvTimeoutError::Disconnected) => Err("JSON reader disconnected".into()),
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn read_messages(stdout: impl Read, sender: Sender<ReaderMessage>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = sender.send(ReaderMessage::Eof);
                    return;
                }
                Ok(_) if line == "\r\n" || line == "\n" => break,
                Ok(_) => {
                    if let Some(value) = line.strip_prefix("Content-Length:") {
                        content_length = value.trim().parse::<usize>().ok();
                    }
                }
                Err(error) => {
                    let _ = sender.send(ReaderMessage::Error(format!(
                        "read JSON-RPC headers: {error}"
                    )));
                    return;
                }
            }
        }
        let Some(length) = content_length else {
            let _ = sender.send(ReaderMessage::Error(
                "JSON-RPC message has no Content-Length header".into(),
            ));
            return;
        };
        let mut body = vec![0; length];
        if let Err(error) = reader.read_exact(&mut body) {
            let _ = sender.send(ReaderMessage::Error(format!("read JSON-RPC body: {error}")));
            return;
        }
        match serde_json::from_slice(&body) {
            Ok(value) => {
                if sender.send(ReaderMessage::Json(value)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(ReaderMessage::Error(format!(
                    "decode JSON-RPC body: {error}"
                )));
                return;
            }
        }
    }
}
