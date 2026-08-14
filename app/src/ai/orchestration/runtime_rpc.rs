//! L1 runtime RPC — metadata file + Unix-domain socket server for
//! headless orchestration.
//!
//! When the GUI (or `zap-oss serve`) starts, it writes a small JSON metadata
//! file (`zap-runtime.json`) into the state directory.  A background thread
//! listens on a Unix-domain socket; CLI invocations that detect a live GUI can
//! forward `check-status` / `check-messages` / `send-message` through the
//! socket instead of opening a second DB connection.
//!
//! **L1 scope**: only `status` and `echo` are real server-side methods; all
//! other commands fall through to a generic "fallback" response so the caller
//! can degrade gracefully.  A full dispatcher is L2.

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

const METADATA_FILE: &str = "zap-runtime.json";

/// Where the runtime metadata file lives.
pub fn runtime_metadata_path() -> PathBuf {
    warp_core::paths::secure_state_dir()
        .unwrap_or_else(warp_core::paths::state_dir)
        .join(METADATA_FILE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMetadata {
    /// Path to the Unix-domain socket this process is listening on.
    pub socket_path: String,
    /// PID of the process hosting the RPC server.
    pub pid: u32,
    /// Launch mode identifier (e.g. "app", "serve").
    pub mode: String,
}

/// Persist metadata to disk (best-effort: logs but does not propagate errors).
pub fn write_metadata(meta: &RuntimeMetadata) {
    let path = runtime_metadata_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string(meta) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("runtime_rpc: failed to write metadata {path:?}: {e}");
            } else {
                log::info!("runtime_rpc: metadata written to {path:?}");
            }
        }
        Err(e) => {
            log::warn!("runtime_rpc: failed to serialize metadata: {e}");
        }
    }
}

/// Remove the metadata file (best-effort).
pub fn clear_metadata() {
    let path = runtime_metadata_path();
    match std::fs::remove_file(&path) {
        Ok(()) => log::info!("runtime_rpc: metadata cleared"),
        Err(e) => log::warn!("runtime_rpc: failed to clear metadata: {e}"),
    }
}

/// Read and parse metadata.  Returns `None` when the file does not exist or
/// cannot be parsed.
pub fn read_metadata() -> Option<RuntimeMetadata> {
    let path = runtime_metadata_path();
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data)
        .map_err(|e| {
            log::warn!("runtime_rpc: corrupt metadata: {e}");
            e
        })
        .ok()
}
/// Whether a GUI runtime is currently alive per the metadata file. Used by
/// the CLI to refuse commands that would race the GUI's in-process router
/// (the single consumer of the "orchestrator" mailbox). **Serve mode does
/// not run a router**, so it must not trigger this guard (#5) — its pulls
/// are the only consumer.
pub fn runtime_alive() -> bool {
    match read_metadata() {
        Some(meta) => is_pid_alive(meta.pid) && meta.mode == "app",
        None => false,
    }
}

/// Check whether the PID recorded in metadata is still alive.
pub fn is_pid_alive(pid: u32) -> bool {
    // Sending signal 0 does not actually deliver a signal; it only checks
    // existence and permissions.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// ---------------------------------------------------------------------------
// Socket server
// ---------------------------------------------------------------------------

/// Handle to the spawned RPC server thread.  Dropping sets the stop flag,
/// wakes the listener, and joins the thread.
pub struct RpcServerHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    socket_path: Arc<Mutex<Option<String>>>,
}

impl Drop for RpcServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ref path) = *self.socket_path.lock() {
            let _ = UnixStream::connect(path); // wake accept()
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn a background Unix-domain socket RPC server.
///
/// Returns the socket path that was bound (for embedding in metadata) and a
/// handle whose Drop shuts the server down.
///
/// L1 implementation: each connection reads one JSON line, dispatches the
/// `cmd` field, and writes one JSON line back.
pub fn spawn_rpc_server(mode: &str) -> Result<(PathBuf, RpcServerHandle)> {
    let dir = warp_core::paths::secure_state_dir()
        .unwrap_or_else(warp_core::paths::state_dir);
    let _ = std::fs::create_dir_all(&dir);

    let socket_path = dir.join(format!(
        "zap-runtime-{}.sock",
        std::process::id()
    ));

    // Clean up stale socket from a previous run.
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind RPC socket {socket_path:?}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let socket_path_str = socket_path.to_string_lossy().to_string();
    let socket_path_for_cleanup = socket_path_str.clone();

    let stop_clone = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("zap-rpc-server".into())
        .spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("set_nonblocking on UnixListener");
            log::info!("runtime_rpc: listening on {socket_path_for_cleanup}");
            loop {
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let _ = stream.set_nonblocking(false);
                        handle_connection(stream);
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                    Err(e) => {
                        log::warn!("runtime_rpc: accept error: {e}");
                        break;
                    }
                }
            }
            let _ = std::fs::remove_file(&socket_path_for_cleanup);
            log::info!("runtime_rpc: server shut down");
        })
        .context("failed to spawn RPC server thread")?;

    let rpc_handle = RpcServerHandle {
        stop,
        thread: Some(handle),
        socket_path: Arc::new(Mutex::new(Some(socket_path_str))),
    };

    let _ = mode; // used in L2 for mode-specific dispatch
    Ok((socket_path, rpc_handle))
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RpcRequest {
    cmd: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct RpcResponse {
    ok: bool,
    /// Whether the command was actually executed on the GPUI thread.
    /// `ok:false && executed:false` = refused before execution (safe to
    /// retry via direct DB); `ok:false && executed:true` = ran but
    /// returned an error (do NOT retry — side effects may exist).
    #[serde(default)]
    executed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn handle_connection(mut stream: UnixStream) {
    // A half-open/slow client must not block the single-threaded accept
    // loop forever: cap the request read at 5 s.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut reader = std::io::BufReader::new(&stream);
    let mut line = String::new();

    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => return,
        _ => {}
    }
    let line = line.trim();

    let req: RpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_response(&mut stream, &RpcResponse {
                ok: false,
                executed: false,
                result: None,
                error: Some(format!("parse error: {e}")),
            });
            return;
        }
    };

    let resp = dispatch_request(&req.cmd, &req.args);
    let _ = write_response(&mut stream, &resp);
}

fn write_response(stream: &mut UnixStream, resp: &RpcResponse) -> std::io::Result<()> {
    let mut json = serde_json::to_string(resp).unwrap_or_else(|_| "{}".into());
    json.push('\n');
    stream.write_all(json.as_bytes())?;
    stream.flush()
}

fn dispatch_request(cmd: &str, args: &serde_json::Value) -> RpcResponse {
    match cmd {
        "echo" => RpcResponse {
            ok: true,
            executed: true,
            result: Some(args.clone()),
            error: None,
        },
        "status" => RpcResponse {
            ok: true,
            executed: true,
            result: Some(serde_json::json!({
                "pid": std::process::id(),
                "mode": "runtime",
            })),
            error: None,
        },
        // L2: full-command forwarding.
        "orchestration" => dispatch_orchestration(args),
        "send-message" | "check-messages" | "check-status" => {
            RpcResponse {
                ok: true,
                executed: false,
                result: Some(serde_json::json!({
                    "fallback": true,
                    "reason": "L1 stub; use the orchestration command",
                })),
                error: None,
            }
        }
        other => RpcResponse {
            ok: false,
            executed: false,
            result: None,
            error: Some(format!("unknown command: {other}")),
        },
    }
}

// ---------------------------------------------------------------------------
// L2 full-command dispatcher
// ---------------------------------------------------------------------------

use crate::ai::agent_sdk::orchestration::execute_command;
use warp_cli::orchestration::OrchestrationCommand;

/// One forwarded orchestration command awaiting GPUI-thread execution.
pub struct DispatcherJob {
    pub command: OrchestrationCommand,
    pub respond: std::sync::mpsc::Sender<DispatcherResult>,
}

/// Result of executing a forwarded command on the GPUI thread.
pub struct DispatcherResult {
    pub ok: bool,
    /// Captured stdout of the execution (what a local CLI would print).
    pub output: String,
    pub error: Option<String>,
}

static DISPATCH_JOBS: OnceLock<async_channel::Sender<DispatcherJob>> = OnceLock::new();

/// Install the process-wide dispatcher job sender. Called once at GUI
/// startup when the `RpcDispatcher` GPUI model is registered; serve mode
/// never calls this, which keeps its socket responses on the L1 fallback
/// path (CLI degrades to direct DB).
pub fn set_dispatcher_sender(sender: async_channel::Sender<DispatcherJob>) {
    let _ = DISPATCH_JOBS.set(sender);
}

/// Execute a forwarded command on the GPUI thread, waiting up to
/// `timeout_ms` for completion.
///
/// Returns `None` when no dispatcher is installed (serve mode) or the
/// execution did not finish in time — the server answers with a fallback
/// response so the CLI degrades to direct DB access.
fn execute_on_gui_thread(
    command: OrchestrationCommand,
    timeout_ms: u64,
) -> Option<DispatcherResult> {
    let tx = DISPATCH_JOBS.get()?;
    // Try-send first: a full channel must not block the accept loop.
    let (resp_tx, resp_rx) = std::sync::mpsc::channel();
    tx.try_send(DispatcherJob {
        command,
        respond: resp_tx,
    })
    .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return None;
        }
        match resp_rx.recv_timeout(deadline - now) {
            Ok(result) => return Some(result),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Dispatcher died mid-execution (e.g. panicking command).
                return Some(DispatcherResult {
                    ok: false,
                    output: String::new(),
                    error: Some("dispatcher dropped the job".into()),
                });
            }
        }
    }
}

/// Run `f` with the process stdout redirected into an in-memory buffer.
///
/// Forwarded commands print their results via `println!` (shared execution
/// body with the CLI path); capturing lets the RPC response carry that
/// output back to the CLI caller.
///
/// **Deadlock prevention**: a background drain thread reads the pipe
/// continuously so its 64 KiB kernel buffer cannot fill up and block the
/// calling (GPUI main) thread mid-`println!`. The old single-threaded
/// approach — write via `f()`, then `read_to_end` — deadlocked whenever a
/// command produced more than 64 KiB of stdout.
fn with_captured_stdout(f: impl FnOnce() -> anyhow::Result<()>) -> DispatcherResult {
    use std::io::Read;

    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return DispatcherResult {
            ok: false,
            output: String::new(),
            error: Some("pipe() failed".into()),
        };
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let saved_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved_fd < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return DispatcherResult {
            ok: false,
            output: String::new(),
            error: Some("dup() failed".into()),
        };
    }

    // Drain thread: reads continuously so the pipe buffer never fills.
    // Takes ownership of read_fd (closed on drop / when read_to_end
    // returns after write_fd is closed below).
    let drain = std::thread::Builder::new()
        .name("rpc-stdout-drain".into())
        .spawn(move || {
            let mut buf = Vec::new();
            let mut reader =
                unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(read_fd) };
            let _ = reader.read_to_end(&mut buf);
            buf
        });

    // Flush Rust's stdout buffer before swapping the fd.
    let _ = std::io::stdout().flush();
    unsafe { libc::dup2(write_fd, libc::STDOUT_FILENO) };

    let result = f();

    // Restore stdout, then close write_fd — the drain thread sees EOF and
    // finishes its read_to_end.
    let _ = std::io::stdout().flush();
    unsafe {
        libc::dup2(saved_fd, libc::STDOUT_FILENO);
        libc::close(saved_fd);
        libc::close(write_fd);
    }

    let buf = match drain {
        Ok(handle) => handle.join().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let output = String::from_utf8_lossy(&buf).to_string();

    match result {
        Ok(()) => DispatcherResult {
            ok: true,
            output,
            error: None,
        },
        Err(e) => DispatcherResult {
            ok: false,
            output,
            error: Some(format!("{e:#}")),
        },
    }
}

/// Handle the L2 `orchestration` command: deserialize the full
/// `OrchestrationCommand`, execute it on the GPUI thread, and wrap the
/// captured output in an `RpcResponse`.
fn dispatch_orchestration(args: &serde_json::Value) -> RpcResponse {
    let command: OrchestrationCommand = match serde_json::from_value(args.clone()) {
        Ok(c) => c,
        Err(e) => {
            return RpcResponse {
                ok: false,
                executed: false,
                result: None,
                error: Some(format!("bad command payload: {e}")),
            }
        }
    };

    // Single-consumer guard.
    if let OrchestrationCommand::CheckMessages { ref handle, .. } = command {
        if handle == "orchestrator" {
            return RpcResponse {
                ok: false,
                executed: false,
                result: None,
                error: Some(
                    "refused: the orchestrator mailbox is consumed by the \
                     in-process message router"
                        .into(),
                ),
            };
        }
    }

    match execute_on_gui_thread(command, 4500) {
        Some(result) => RpcResponse {
            ok: result.ok,
            // Command was executed — even on failure, side effects may
            // exist, so the CLI must NOT retry via direct DB.
            executed: true,
            result: Some(serde_json::json!({
                "output": result.output,
            })),
            error: result.error,
        },
        // Timeout: the job was dispatched and may still be running on the
        // GPUI thread. Returning fallback would cause the CLI to re-execute
        // via direct DB → double side effects. Instead return executed:true
        // so the CLI treats it as "handled, don't retry".
        None => RpcResponse {
            ok: false,
            executed: true,
            result: None,
            error: Some(
                "execution timed out — the command may still be running on \
                 the GUI thread, do not retry"
                    .into(),
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Client-side fast path
// ---------------------------------------------------------------------------

/// Try to forward a CLI command through the runtime socket.
///
/// Returns `Ok(Some(response_json))` if the socket was reached and responded
/// with a real (non-fallback) result, `Ok(None)` if no metadata/socket was
/// found or the server returned a fallback flag (caller should fall back to
/// direct DB), or `Err` if the socket responded with an error.
pub fn try_socket_forward(cmd: &str, args: &serde_json::Value) -> Result<Option<String>> {
    let meta = match read_metadata() {
        Some(m) => m,
        None => return Ok(None),
    };

    if !is_pid_alive(meta.pid) {
        log::info!("runtime_rpc: stale metadata (pid {} dead), clearing", meta.pid);
        clear_metadata();
        return Ok(None);
    }

    let socket = std::path::Path::new(&meta.socket_path);
    let mut stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) => {
            log::info!("runtime_rpc: socket connect failed: {e}, falling back");
            return Ok(None);
        }
    };

    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));

    let req = serde_json::json!({
        "cmd": cmd,
        "args": args,
    });
    let mut line = serde_json::to_string(&req)?;
    line.push('\n');

    if let Err(e) = stream.write_all(line.as_bytes()) {
        log::info!("runtime_rpc: socket write failed: {e}, falling back");
        return Ok(None);
    }
    if let Err(e) = stream.flush() {
        log::info!("runtime_rpc: socket flush failed: {e}, falling back");
        return Ok(None);
    }

    let mut reader = std::io::BufReader::new(stream);
    let mut response = String::new();
    match reader.read_line(&mut response) {
        Ok(0) => {
            log::info!("runtime_rpc: empty response from socket");
            return Ok(None);
        }
        Err(e) => {
            log::info!("runtime_rpc: socket read failed: {e}, falling back");
            return Ok(None);
        }
        _ => {}
    }

    let resp: RpcResponse = serde_json::from_str(response.trim())?;
    // 1d: executed=true means the command ran on the GPUI thread — even if
    // it returned ok:false (error) or timed out, side effects may exist.
    // Return Ok(Some) so the CLI does NOT fall back to direct DB (which
    // would double-execute). Only !executed responses (refused, L1 stub,
    // fallback) degrade to direct DB.
    if resp.executed {
        if resp.ok {
            return Ok(Some(serde_json::to_string_pretty(&resp.result)?));
        }
        return Ok(Some(serde_json::json!({
            "error": resp.error.clone(),
            "executed": true,
        })
        .to_string()));
    }
    if resp.ok {
        if let Some(result) = &resp.result {
            if result.get("fallback").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(None);
            }
        }
        Ok(Some(serde_json::to_string_pretty(&resp.result)?))
    } else {
        Err(anyhow!(
            "RPC error: {}",
            resp.error.unwrap_or_else(|| "unknown".into())
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatch_refuses_orchestrator_pull() {
        let args = serde_json::json!({
            "CheckMessages": {
                "handle": "orchestrator",
                "wait": false,
                "timeout_ms": 120000,
                "message_type": [],
            }
        });
        let resp = dispatch_request("orchestration", &args);
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("refused"));
    }

    #[test]
    fn test_metadata_roundtrip() {
        let meta = RuntimeMetadata {
            socket_path: "/tmp/test.sock".into(),
            pid: 12345,
            mode: "test".into(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: RuntimeMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.socket_path, meta.socket_path);
        assert_eq!(parsed.pid, meta.pid);
        assert_eq!(parsed.mode, meta.mode);
    }

    #[test]
    fn test_dispatch_echo() {
        let args = serde_json::json!({"hello": "world"});
        let resp = dispatch_request("echo", &args);
        assert!(resp.ok);
        assert_eq!(resp.result.unwrap(), args);
    }

    #[test]
    fn test_dispatch_status() {
        let resp = dispatch_request("status", &serde_json::Value::Null);
        assert!(resp.ok);
        assert_eq!(resp.result.unwrap()["pid"], std::process::id() as i64);
    }

    #[test]
    fn test_dispatch_unknown() {
        let resp = dispatch_request("nope", &serde_json::Value::Null);
        assert!(!resp.ok);
    }
}

// ---------------------------------------------------------------------------
// RpcDispatcher — GPUI model executing forwarded commands (L2)
// ---------------------------------------------------------------------------

/// GPUI model that drains `DispatcherJob`s on the main thread and executes
/// them via the shared `execute_command` body.
///
/// Register once at GUI startup (after the orchestration store and view
/// bridges are up):
///
/// ```ignore
/// ctx.add_singleton_model(|ctx| {
///     crate::ai::orchestration::runtime_rpc::RpcDispatcher::new(ctx)
/// });
/// ```
pub struct RpcDispatcher;

impl RpcDispatcher {
    pub fn new(ctx: &mut warpui::ModelContext<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<DispatcherJob>(16);
        set_dispatcher_sender(tx);

        // NB: do NOT touch `connection::store()` here — the DB path is only
        // set later during app init; resolve it lazily per job instead.

        ctx.spawn_stream_local(
            rx,
            move |_me, job: DispatcherJob, ctx| {
                let DispatcherJob { command, respond } = job;
                let store = ::ai::agent::orchestration::connection::store();
                let result = with_captured_stdout(|| execute_command(&command, store, ctx));
                let _ = respond.send(result);
            },
            |_, _| {},
        );
        Self
    }
}

impl warpui::Entity for RpcDispatcher {
    type Event = ();
}

impl warpui::SingletonEntity for RpcDispatcher {}

