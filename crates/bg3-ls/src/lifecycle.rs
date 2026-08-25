//! Process lifecycle behavior which is not exposed by `tower-lsp-server`.

use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tower::Service as TowerService;
use tower_lsp_server::jsonrpc::{Request, Response};

const PARENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Tracks the LSP lifecycle state needed by the executable.
#[derive(Clone, Default)]
pub(crate) struct State {
    shutdown: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
}

impl State {
    /// Returns the process status required by the LSP `exit` notification.
    pub(crate) fn exit_code(&self) -> std::process::ExitCode {
        if self.shutdown.load(Ordering::Acquire) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::from(1)
        }
    }

    fn mark_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    fn mark_exit(&self) {
        self.exited.store(true, Ordering::Release);
    }

    fn should_stop_parent_monitor(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

/// A transport service wrapper that observes lifecycle requests.
pub(crate) struct Service<S> {
    inner: S,
    state: State,
}

impl<S> Service<S> {
    pub(crate) fn new(inner: S, state: State) -> Self {
        Self { inner, state }
    }
}

impl<S> TowerService<Request> for Service<S>
where
    S: TowerService<Request, Response = Option<Response>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Option<Response>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let method = request.method().to_owned();
        let parent_pid = (method == "initialize")
            .then(|| request.params().and_then(|params| params.get("processId")))
            .flatten()
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok());
        let state = self.state.clone();
        let inner = self.inner.call(request);

        Box::pin(async move {
            let response = inner.await?;
            if response.as_ref().is_some_and(Response::is_ok) {
                if method == "shutdown" {
                    state.mark_shutdown();
                } else if method == "initialize"
                    && let Some(pid) = parent_pid
                {
                    spawn_parent_monitor(pid, state.clone());
                }
            }
            if method == "exit" {
                state.mark_exit();
            }
            Ok(response)
        })
    }
}

fn spawn_parent_monitor(pid: u32, state: State) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(PARENT_POLL_INTERVAL).await;
            if state.should_stop_parent_monitor() {
                return;
            }
            let alive = tokio::task::spawn_blocking(move || process_is_alive(pid))
                .await
                .unwrap_or(false);
            if !alive {
                std::process::exit(0);
            }
        }
    });
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .is_ok_and(|output| {
            output.status.success() && !String::from_utf8_lossy(&output.stdout).contains("No tasks")
        })
}

#[cfg(not(any(unix, windows)))]
fn process_is_alive(_pid: u32) -> bool {
    true
}
