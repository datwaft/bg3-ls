use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bg3_ide::{
    CompletionItem as Bg3CompletionItem, CompletionKind as Bg3CompletionKind,
    DiagnosticSeverity as Bg3DiagnosticSeverity, OverlayDocument, OverlaySet, SourceLocation,
    Symbol, WorkspaceSnapshot,
};
use bg3_index::{Position as Bg3Position, SourceFile, SourceKind, TextRange, parse_source};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};
use url::Url;

use crate::Error;
use crate::config::ResolvedConfig;
use crate::coordinator::{BuildState, Coordinator};

/// A cloneable protocol adapter around shared server state.
#[derive(Clone)]
pub struct Backend {
    client: Client,
    inner: Arc<Inner>,
}

struct Inner {
    config: RwLock<Option<Arc<ResolvedConfig>>>,
    overlays: RwLock<OverlaySet>,
    coordinator: Arc<Coordinator>,
    snippet_support: AtomicBool,
    diagnostic_tasks: Mutex<HashMap<PathBuf, JoinHandle<()>>>,
}

impl Backend {
    /// Creates a server backend before LSP initialization supplies project configuration.
    pub fn new(client: Client, cache_dir: Option<PathBuf>) -> Self {
        Self {
            client,
            inner: Arc::new(Inner {
                config: RwLock::new(None),
                overlays: RwLock::new(OverlaySet::default()),
                coordinator: Arc::new(Coordinator::new(cache_dir)),
                snippet_support: AtomicBool::new(false),
                diagnostic_tasks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Returns the validated project configuration after initialization.
    async fn config(&self) -> Result<Arc<ResolvedConfig>, Error> {
        self.inner
            .config
            .read()
            .await
            .clone()
            .ok_or_else(|| Error::Config("the server is not initialized".into()))
    }

    /// Waits for a complete index generation.
    async fn snapshot(&self) -> jsonrpc::Result<Arc<WorkspaceSnapshot>> {
        self.inner
            .coordinator
            .wait_snapshot()
            .await
            .map_err(rpc_error)
    }

    /// Parses and stores one full open-document overlay.
    async fn update_overlay(&self, uri: &Uri, text: String, version: i32) -> Result<(), Error> {
        let path = uri_to_path(uri)?;
        let snapshot = self.inner.coordinator.wait_snapshot().await?;
        let module = snapshot.module_for_path(&path).ok_or_else(|| {
            Error::Config(format!(
                "document is outside configured modules: {}",
                path.display()
            ))
        })?;
        let parsed = parse_source(
            SourceFile {
                path: path.clone(),
                kind: open_document_kind(&path)?,
            },
            &text,
            &snapshot.schema,
            &self.config().await?.language,
        )?;
        self.inner.overlays.write().await.insert(
            path,
            OverlayDocument {
                module: module.name.clone(),
                version,
                text,
                parsed: Arc::new(parsed),
            },
        );
        Ok(())
    }

    /// Executes an overlay update and reports notification failures to the client log.
    async fn update_overlay_logged(&self, uri: &Uri, text: String, version: i32) {
        if let Err(error) = self.update_overlay(uri, text, version).await {
            self.client
                .log_message(MessageType::ERROR, error.to_string())
                .await;
        }
    }

    /// Debounces diagnostics and rejects work for an obsolete document version.
    async fn schedule_diagnostics(&self, uri: Uri, version: i32) {
        let Ok(path) = uri_to_path(&uri) else {
            return;
        };
        let backend = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            backend.publish_diagnostics_now(uri, version).await;
        });
        if let Some(previous) = self.inner.diagnostic_tasks.lock().await.insert(path, task) {
            previous.abort();
        }
    }

    /// Publishes diagnostics when the requested overlay version is still current.
    async fn publish_diagnostics_now(&self, uri: Uri, version: i32) {
        let Ok(path) = uri_to_path(&uri) else {
            return;
        };
        let Ok(snapshot) = self.inner.coordinator.wait_snapshot().await else {
            return;
        };
        let overlays = self.inner.overlays.read().await;
        if overlays.get(&path).map(|document| document.version) != Some(version) {
            return;
        }
        let Ok(config) = self.config().await else {
            return;
        };
        let severity = config
            .unresolved_references
            .as_deref()
            .and_then(diagnostic_severity);
        let diagnostics = snapshot
            .diagnostics(&path, &overlays, severity)
            .into_iter()
            .map(|diagnostic| Diagnostic {
                range: to_lsp_range(diagnostic.range),
                severity: Some(match diagnostic.severity {
                    Bg3DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    Bg3DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                    Bg3DiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
                    Bg3DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                }),
                code: Some(NumberOrString::String(diagnostic.code)),
                code_description: None,
                source: Some("bg3".into()),
                message: diagnostic.message,
                related_information: None,
                tags: None,
                data: None,
            })
            .collect();
        drop(overlays);
        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    /// Republishes all open-document diagnostics after each successful index generation.
    async fn monitor_diagnostics(&self) {
        let mut states = self.inner.coordinator.subscribe();
        while states.changed().await.is_ok() {
            if !matches!(*states.borrow(), BuildState::Ready(_)) {
                continue;
            }
            let documents = self.inner.overlays.read().await.versions();
            for (path, version) in documents {
                if let Ok(uri) = path_to_uri(&path) {
                    self.schedule_diagnostics(uri, version).await;
                }
            }
        }
    }
}

/// Selects a supported source format for an attached loose document.
fn open_document_kind(path: &Path) -> Result<SourceKind, Error> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("txt") => Ok(SourceKind::PlainStats),
        Some("lsx") => Ok(SourceKind::Lsx),
        _ => Err(Error::Config(format!(
            "the server cannot attach to this source format: {}",
            path.display()
        ))),
    }
}

#[allow(deprecated)]
impl LanguageServer for Backend {
    /// Validates configuration and advertises native navigation capabilities.
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        let snippet_support = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text| text.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|item| item.snippet_support)
            .unwrap_or(false);
        self.inner
            .snippet_support
            .store(snippet_support, Ordering::Release);
        let root_uri = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first().map(|folder| &folder.uri))
            .or(params.root_uri.as_ref())
            .ok_or_else(|| jsonrpc::Error::invalid_params("a workspace root is required"))?;
        let config = Arc::new(
            ResolvedConfig::load(params.initialization_options, root_uri.as_str())
                .map_err(rpc_error)?,
        );
        *self.inner.config.write().await = Some(config);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(PositionEncodingKind::UTF8),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(
                        ["\"", ";", "(", ",", ":"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    ),
                    ..CompletionOptions::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(["(", ","].into_iter().map(str::to_owned).collect()),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["bg3.reload".into(), "bg3.indexInfo".into()],
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(true),
                    },
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "bg3-ls".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            offset_encoding: None,
        })
    }

    /// Starts indexing and the recursive source watcher after the LSP handshake.
    async fn initialized(&self, _: InitializedParams) {
        let Ok(config) = self.config().await else {
            return;
        };
        let monitor = self.clone();
        tokio::spawn(async move { monitor.monitor_diagnostics().await });
        let backend = self.clone();
        tokio::spawn(async move {
            backend
                .inner
                .coordinator
                .rebuild(Arc::clone(&config), &backend.client)
                .await;
            if matches!(backend.inner.coordinator.state(), BuildState::Ready(_))
                && let Err(error) = backend
                    .inner
                    .coordinator
                    .start_watcher(config, backend.client.clone())
                    .await
            {
                backend
                    .client
                    .show_message(MessageType::WARNING, error.to_string())
                    .await;
            }
        });
    }

    /// Accepts graceful LSP shutdown.
    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    /// Replaces the disk record with the complete opened buffer text.
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        self.update_overlay_logged(
            &params.text_document.uri,
            params.text_document.text,
            version,
        )
        .await;
        self.schedule_diagnostics(uri, version).await;
    }

    /// Applies full-document synchronization for unsaved changes.
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.last() {
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;
            self.update_overlay_logged(&params.text_document.uri, change.text.clone(), version)
                .await;
            self.schedule_diagnostics(uri, version).await;
        }
    }

    /// Refreshes an overlay when the client includes saved text.
    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let version = uri_to_path(&params.text_document.uri)
            .ok()
            .and_then(|path| {
                self.inner
                    .overlays
                    .try_read()
                    .ok()
                    .and_then(|overlays| overlays.get(&path).map(|overlay| overlay.version))
            })
            .unwrap_or(0);
        if let Some(text) = params.text {
            self.update_overlay_logged(&params.text_document.uri, text, version)
                .await;
        }
        self.schedule_diagnostics(params.text_document.uri, version)
            .await;
    }

    /// Restores the indexed disk record when a document closes.
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Ok(path) = uri_to_path(&params.text_document.uri) {
            self.inner.overlays.write().await.remove(&path);
            if let Some(task) = self.inner.diagnostic_tasks.lock().await.remove(&path) {
                task.abort();
            }
            self.client
                .publish_diagnostics(params.text_document.uri, Vec::new(), None)
                .await;
        }
    }

    /// Returns every visible override from highest to lowest precedence.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let snapshot = self.snapshot().await?;
        let path = uri_to_path(&params.text_document_position_params.text_document.uri)
            .map_err(rpc_error)?;
        let position = to_bg3_position(params.text_document_position_params.position);
        let overlays = self.inner.overlays.read().await;
        let locations = snapshot
            .definitions_at(&path, position, &overlays)
            .into_iter()
            .map(|definition| location(&definition.path, definition.definition.selection_range))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rpc_error)?;
        Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)))
    }

    /// Returns schema metadata, effective definitions, and complete override chains.
    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let snapshot = self.snapshot().await?;
        let path = uri_to_path(&params.text_document_position_params.text_document.uri)
            .map_err(rpc_error)?;
        let position = to_bg3_position(params.text_document_position_params.position);
        let overlays = self.inner.overlays.read().await;
        Ok(snapshot
            .hover(&path, position, &overlays)
            .or_else(|| snapshot.language_hover(&path, position, &overlays))
            .map(|value| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: None,
            }))
    }

    /// Returns schema-aware values and lexical recovery results for incomplete input.
    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let snapshot = self.snapshot().await?;
        let path =
            uri_to_path(&params.text_document_position.text_document.uri).map_err(rpc_error)?;
        let position = to_bg3_position(params.text_document_position.position);
        let overlays = self.inner.overlays.read().await;
        let completion = snapshot.completion(
            &path,
            position,
            &overlays,
            self.inner.snippet_support.load(Ordering::Acquire),
        );
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: completion.incomplete,
            items: completion.items.into_iter().map(completion_item).collect(),
        })))
    }

    /// Reports verified curated function parameters without inferring signatures.
    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> jsonrpc::Result<Option<tower_lsp_server::ls_types::SignatureHelp>> {
        let snapshot = self.snapshot().await?;
        let path = uri_to_path(&params.text_document_position_params.text_document.uri)
            .map_err(rpc_error)?;
        let position = to_bg3_position(params.text_document_position_params.position);
        let overlays = self.inner.overlays.read().await;
        Ok(snapshot
            .signature_help(&path, position, &overlays)
            .map(|help| tower_lsp_server::ls_types::SignatureHelp {
                signatures: vec![SignatureInformation {
                    label: help.label,
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: help.documentation,
                    })),
                    parameters: Some(
                        help.parameters
                            .into_iter()
                            .map(|label| ParameterInformation {
                                label: ParameterLabel::Simple(label),
                                documentation: None,
                            })
                            .collect(),
                    ),
                    active_parameter: None,
                }],
                active_signature: Some(0),
                active_parameter: Some(help.active_parameter as u32),
            }))
    }

    /// Searches semantic references across every configured visible module.
    async fn references(&self, params: ReferenceParams) -> jsonrpc::Result<Option<Vec<Location>>> {
        let snapshot = self.snapshot().await?;
        let path =
            uri_to_path(&params.text_document_position.text_document.uri).map_err(rpc_error)?;
        let position = to_bg3_position(params.text_document_position.position);
        let overlays = self.inner.overlays.read().await;
        let locations = snapshot
            .references_at(
                &path,
                position,
                params.context.include_declaration,
                &overlays,
            )
            .into_iter()
            .map(|item| location(&item.path, item.range))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rpc_error)?;
        Ok(Some(locations))
    }

    /// Lists supported top-level declarations in one document.
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let snapshot = self.snapshot().await?;
        let path = uri_to_path(&params.text_document.uri).map_err(rpc_error)?;
        let overlays = self.inner.overlays.read().await;
        let symbols = snapshot
            .document_symbols(&path, &overlays)
            .into_iter()
            .map(|symbol| DocumentSymbol {
                name: symbol.name,
                detail: Some(symbol.kind.clone()),
                kind: symbol_kind(&symbol.kind),
                tags: None,
                deprecated: None,
                range: to_lsp_range(symbol.location.range),
                selection_range: to_lsp_range(symbol.location.range),
                children: None,
            })
            .collect();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    /// Searches visible declarations while preserving shadowed module results.
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> jsonrpc::Result<Option<WorkspaceSymbolResponse>> {
        let snapshot = self.snapshot().await?;
        let overlays = self.inner.overlays.read().await;
        let symbols = snapshot
            .workspace_symbols(&params.query, &overlays)
            .into_iter()
            .map(symbol_information)
            .collect::<Result<Vec<_>, _>>()
            .map_err(rpc_error)?;
        Ok(Some(WorkspaceSymbolResponse::Flat(symbols)))
    }

    /// Executes explicit reload and index-information commands without a plugin.
    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<LSPAny>> {
        match params.command.as_str() {
            "bg3.reload" => {
                let config = self.config().await.map_err(rpc_error)?;
                let started = self.inner.coordinator.rebuild(config, &self.client).await;
                Ok(Some(serde_json::json!({ "started": started })))
            }
            "bg3.indexInfo" => match self.inner.coordinator.state() {
                BuildState::Ready(info) => Ok(Some(
                    serde_json::to_value(info).map_err(|_| jsonrpc::Error::internal_error())?,
                )),
                BuildState::Failed(message) => Ok(Some(serde_json::json!({
                    "state": "failed",
                    "message": message,
                }))),
                BuildState::Idle => Ok(Some(serde_json::json!({ "state": "idle" }))),
                BuildState::Building => Ok(Some(serde_json::json!({ "state": "building" }))),
            },
            _ => Err(jsonrpc::Error::method_not_found()),
        }
    }
}

/// Converts a file URI to an absolute local path.
fn uri_to_path(uri: &Uri) -> Result<PathBuf, Error> {
    Url::parse(uri.as_str())
        .map_err(|error| Error::Protocol(format!("invalid document URI: {error}")))?
        .to_file_path()
        .map_err(|()| Error::Protocol("document URI is not a file URI".into()))
}

/// Converts an absolute local path to an LSP file URI.
fn path_to_uri(path: &Path) -> Result<Uri, Error> {
    let url = Url::from_file_path(path)
        .map_err(|()| Error::Protocol(format!("path cannot become a URI: {}", path.display())))?;
    url.as_str()
        .parse()
        .map_err(|error| Error::Protocol(format!("generated URI is invalid: {error}")))
}

/// Converts one internal location to the LSP representation.
fn location(path: &Path, range: TextRange) -> Result<Location, Error> {
    Ok(Location {
        uri: path_to_uri(path)?,
        range: to_lsp_range(range),
    })
}

/// Converts an internal UTF-8 range to an LSP range.
fn to_lsp_range(range: TextRange) -> Range {
    Range {
        start: Position::new(range.start.line, range.start.character),
        end: Position::new(range.end.line, range.end.character),
    }
}

/// Converts an LSP position to the editor-neutral UTF-8 type.
fn to_bg3_position(position: Position) -> Bg3Position {
    Bg3Position {
        line: position.line,
        character: position.character,
    }
}

/// Maps BG3 top-level declarations to standard LSP symbol kinds.
fn symbol_kind(kind: &str) -> SymbolKind {
    match kind {
        "Equipment" | "ItemGroup" | "NameGroup" | "SpellSet" | "TreasureTable" => SymbolKind::ARRAY,
        _ => SymbolKind::OBJECT,
    }
}

/// Converts an editor-neutral workspace symbol to the legacy LSP result shape.
#[allow(deprecated)]
fn symbol_information(symbol: Symbol) -> Result<SymbolInformation, Error> {
    Ok(SymbolInformation {
        name: symbol.name,
        kind: symbol_kind(&symbol.kind),
        tags: None,
        deprecated: None,
        location: location(&symbol.location.path, symbol.location.range)?,
        container_name: Some(symbol.module),
    })
}

/// Converts one internal completion edit to a standard LSP completion item.
fn completion_item(item: Bg3CompletionItem) -> CompletionItem {
    CompletionItem {
        label: item.label,
        kind: Some(match item.kind {
            Bg3CompletionKind::Class => CompletionItemKind::CLASS,
            Bg3CompletionKind::Field => CompletionItemKind::FIELD,
            Bg3CompletionKind::Value => CompletionItemKind::VALUE,
            Bg3CompletionKind::Function => CompletionItemKind::FUNCTION,
            Bg3CompletionKind::Reference => CompletionItemKind::REFERENCE,
        }),
        detail: item.detail,
        documentation: item.documentation.map(Documentation::String),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: to_lsp_range(item.range),
            new_text: item.new_text,
        })),
        insert_text_format: item.snippet.then_some(InsertTextFormat::SNIPPET),
        ..CompletionItem::default()
    }
}

/// Converts a validated configuration severity to the editor-neutral type.
fn diagnostic_severity(value: &str) -> Option<Bg3DiagnosticSeverity> {
    match value {
        "error" => Some(Bg3DiagnosticSeverity::Error),
        "warning" => Some(Bg3DiagnosticSeverity::Warning),
        "information" => Some(Bg3DiagnosticSeverity::Information),
        "hint" => Some(Bg3DiagnosticSeverity::Hint),
        _ => None,
    }
}

/// Converts typed server failures to JSON-RPC request errors.
fn rpc_error(error: Error) -> jsonrpc::Error {
    jsonrpc::Error::invalid_params(error.to_string())
}

#[allow(dead_code)]
fn _location_type_contract(_: SourceLocation) {}
