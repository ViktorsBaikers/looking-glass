//! The audited process-execution engine (risk #2) — spawn-argv, process-group
//! kill, hard timeout, bounded output, incremental line-streaming, and the one
//! global concurrency cap. Reused verbatim by the built-in local node (`central`)
//! and the remote agent, so the orphan-reap / timeout logic is written and audited
//! once, not copied into two crates that drift.
//!
//! Safety properties this module owns:
//! - **No shell.** A [`CommandTemplate`] is spawned as `program` + discrete argv
//!   (`Command::new(program).args(..)`); user input never becomes a shell string.
//! - **No orphans.** Every child is spawned as its own process-group leader
//!   (`process_group(0)`); on timeout, cancel, or client disconnect the whole
//!   group is `SIGKILL`ed, so descendant processes cannot outlive the run (AC14).
//! - **Bounded.** A hard per-run timeout and a total-output byte cap bound both
//!   time and memory. Output is read in fixed chunks and no single line is
//!   buffered past [`MAX_LINE_BYTES`], so a process that emits gigabytes with no
//!   newline cannot balloon the heap before the cap is checked; the streaming
//!   channel is bounded too, so a slow reader applies backpressure.
//! - **One global cap.** A single semaphore bounds the total number of in-flight
//!   commands per node (AC40); over the cap a start is refused with no process
//!   spawned, and the permit is released on every exit path.

use std::io::ErrorKind;
use std::net::IpAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, Semaphore};

use crate::template::CommandTemplate;
use crate::validate::validate_ip;

/// Configured bounds every run is held to. `max_concurrent` is the global cap
/// (the semaphore size); the rest bound a single run.
#[derive(Clone, Copy, Debug)]
pub struct ExecLimits {
    /// Global cap on total concurrent runs per node (AC40 / FR-075).
    pub max_concurrent: usize,
    /// Hard wall-clock timeout for a single run.
    pub timeout: Duration,
    /// Total output bytes streamed before a run is truncated and stopped.
    pub max_output_bytes: usize,
    /// Bound on the in-flight event channel — backpressure, not buffering.
    pub channel_capacity: usize,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            timeout: Duration::from_secs(30),
            max_output_bytes: 256 * 1024,
            channel_capacity: 64,
        }
    }
}

/// How a run ended. Every terminal path produces exactly one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    /// The process exited on its own; `success` mirrors its exit status.
    Completed { success: bool },
    /// The hard timeout elapsed; the process group was killed.
    TimedOut,
    /// The total-output cap was hit; the process group was killed.
    OutputCapped,
    /// The consumer went away (client disconnect / cancel); the group was killed.
    Canceled,
    /// The process could not be started or reaped (e.g. missing tool).
    Failed,
}

/// One item in a run's event stream. The consumer renders `Line`s as they arrive
/// (incremental, AC16), shows `Failed` as a clear error line (AC41), and treats
/// `Done` as the terminal marker.
#[derive(Clone, Debug)]
pub enum ExecEvent {
    Line(String),
    Failed(String),
    Done {
        status: ExecStatus,
        elapsed_ms: u128,
    },
}

/// Why a run could not be started — refused *before* any process is spawned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    /// The global concurrency cap is saturated (AC40): no permit, no spawn.
    Busy,
    /// The pinned target IP failed re-validation at spawn time (defense in depth).
    Rejected(String),
}

/// A started run: the receiver drains [`ExecEvent`]s until `Done`. Dropping it
/// (client disconnect / cancel) is observed by the driver and kills the process
/// group — no explicit cancel call is required.
pub struct ExecHandle {
    pub events: mpsc::Receiver<ExecEvent>,
}

/// The execution engine: the bounds plus the one global permit pool. Cheap to
/// clone (the semaphore is shared) so it lives in shared application state.
#[derive(Clone)]
pub struct ExecEngine {
    limits: ExecLimits,
    permits: Arc<Semaphore>,
}

impl ExecEngine {
    pub fn new(limits: ExecLimits) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limits.max_concurrent)),
            limits,
        }
    }

    /// Permits currently free — used by tests to assert the cap and that a permit
    /// is released on every exit path.
    pub fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }

    /// Try to start a run. Acquires a global permit first: if the cap is
    /// saturated this returns [`StartError::Busy`] and spawns **nothing**. The
    /// pinned target IP (when the caller has one) is re-validated here as
    /// defense in depth, so a bug upstream cannot reach a non-public address.
    pub fn try_start(
        &self,
        command: CommandTemplate,
        pinned_ip: Option<IpAddr>,
    ) -> Result<ExecHandle, StartError> {
        if let Some(ip) = pinned_ip {
            validate_ip(ip).map_err(|reason| StartError::Rejected(reason.to_string()))?;
        }
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| StartError::Busy)?;

        let (tx, rx) = mpsc::channel(self.limits.channel_capacity);
        let limits = self.limits;
        tokio::spawn(async move {
            // The permit is moved into the driver and dropped when it returns —
            // released on success, timeout, cancel, disconnect, or spawn failure.
            let _permit = permit;
            drive(command, tx, limits).await;
        });
        Ok(ExecHandle { events: rx })
    }
}

async fn drive(command: CommandTemplate, tx: mpsc::Sender<ExecEvent>, limits: ExecLimits) {
    let start = Instant::now();

    let mut builder = Command::new(command.program);
    builder
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    builder.process_group(0);

    let mut child = match builder.spawn() {
        Ok(child) => child,
        Err(error) => {
            // AC41: a missing tool (or any spawn failure) surfaces a clear,
            // non-technical message — never a raw OS error or a hang.
            let message = if error.kind() == ErrorKind::NotFound {
                "the diagnostic tool is not available on this node".to_string()
            } else {
                "the diagnostic could not be started".to_string()
            };
            let _ = tx.send(ExecEvent::Failed(message)).await;
            let _ = tx
                .send(ExecEvent::Done {
                    status: ExecStatus::Failed,
                    elapsed_ms: start.elapsed().as_millis(),
                })
                .await;
            return;
        }
    };

    let pid = child.id();
    let mut stdout = child.stdout.take().map(LineReader::new);
    let mut stderr = child.stderr.take().map(LineReader::new);
    let mut stdout_done = stdout.is_none();
    let mut stderr_done = stderr.is_none();
    let mut sent_bytes = 0usize;

    let timeout = tokio::time::sleep(limits.timeout);
    tokio::pin!(timeout);

    // `child.wait()` is deliberately NOT a select arm: reaping the leader here
    // would free its pid while a backgrounded descendant could still be alive,
    // reopening the pid-reuse window. Completion is instead detected by both
    // pipes reaching EOF, so the leader stays unreaped until the kill+reap tail —
    // its pgid cannot be recycled, and the kill is safe on every path.
    let ended = loop {
        if stdout_done && stderr_done {
            break Ended::Completed;
        }
        tokio::select! {
            // Consumer went away (client disconnect or Cancel closed the stream).
            _ = tx.closed() => break Ended::Canceled,
            // Single fixed deadline — created once so streaming output never
            // resets the timer.
            _ = &mut timeout => break Ended::TimedOut,
            line = read_line(&mut stdout), if !stdout_done => match line {
                Some(line) => match emit(&tx, &mut sent_bytes, line, limits.max_output_bytes).await {
                    Emit::Ok => {}
                    Emit::Disconnected => break Ended::Canceled,
                    Emit::Capped => break Ended::OutputCapped,
                },
                None => stdout_done = true,
            },
            line = read_line(&mut stderr), if !stderr_done => match line {
                Some(line) => match emit(&tx, &mut sent_bytes, line, limits.max_output_bytes).await {
                    Emit::Ok => {}
                    Emit::Disconnected => break Ended::Canceled,
                    Emit::Capped => break Ended::OutputCapped,
                },
                None => stderr_done = true,
            },
        }
    };

    // Kill the whole group on EVERY path, including clean completion: a command
    // can exit 0 while a process it backgrounded is still alive (`sh -c "sleep &
    // exit 0"`), and that descendant would otherwise be reparented to init and
    // orphaned. The leader is still unreaped here, so its pgid is pinned and
    // cannot be recycled; on a clean single-process exit the kill is a harmless
    // ESRCH no-op.
    if let Some(pid) = pid {
        kill_group(pid);
    }

    // Reap the leader (and release its pid) only after the group is signalled.
    let reaped = child.wait().await;

    let status = match ended {
        Ended::Completed => match reaped {
            Ok(exit) => ExecStatus::Completed {
                success: exit.success(),
            },
            Err(_) => ExecStatus::Failed,
        },
        Ended::Canceled => ExecStatus::Canceled,
        Ended::TimedOut => ExecStatus::TimedOut,
        Ended::OutputCapped => ExecStatus::OutputCapped,
    };

    if status == ExecStatus::OutputCapped {
        let _ = tx
            .send(ExecEvent::Failed(
                "output limit reached — the run was truncated".to_string(),
            ))
            .await;
    }

    // On Canceled the receiver is already gone; this send is a no-op.
    let _ = tx
        .send(ExecEvent::Done {
            status,
            elapsed_ms: start.elapsed().as_millis(),
        })
        .await;
}

/// How the run's read loop ended, before the uniform kill + reap tail maps it to
/// an [`ExecStatus`].
enum Ended {
    Completed,
    Canceled,
    TimedOut,
    OutputCapped,
}

enum Emit {
    Ok,
    Capped,
    Disconnected,
}

async fn emit(
    tx: &mpsc::Sender<ExecEvent>,
    sent_bytes: &mut usize,
    line: String,
    max_output_bytes: usize,
) -> Emit {
    *sent_bytes = sent_bytes.saturating_add(line.len() + 1);
    if tx.send(ExecEvent::Line(line)).await.is_err() {
        return Emit::Disconnected;
    }
    if *sent_bytes >= max_output_bytes {
        Emit::Capped
    } else {
        Emit::Ok
    }
}

/// Bytes read from the child per syscall.
const READ_CHUNK: usize = 8 * 1024;
/// The most a single line is allowed to buffer before it is force-flushed as a
/// line of its own. This is the memory bound: a process that emits an unbounded
/// run of bytes with no newline is chunked into pieces of at most this size and
/// each piece counts toward the total cap, so the heap never grows without
/// limit waiting for a newline that never comes.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Reads a child pipe into lines with a hard per-line memory bound. Unlike
/// `AsyncBufReadExt::lines`, which buffers up to the next `\n` (or EOF) with no
/// length limit, this force-flushes once the accumulator reaches
/// [`MAX_LINE_BYTES`], so total buffered memory is bounded regardless of what
/// the process writes. The accumulator lives in `self`, so the read future is
/// cancel-safe across `select!` iterations.
struct LineReader<R> {
    reader: R,
    buf: Vec<u8>,
    eof: bool,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(READ_CHUNK),
            eof: false,
        }
    }

    /// The next line (newline stripped), a force-flushed [`MAX_LINE_BYTES`] chunk
    /// of a runaway line, or `None` at EOF once the buffer is drained.
    async fn next(&mut self) -> Option<String> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
                line.pop(); // drop the '\n'
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            if self.buf.len() >= MAX_LINE_BYTES {
                let line = std::mem::take(&mut self.buf);
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            if self.eof {
                if self.buf.is_empty() {
                    return None;
                }
                let line = std::mem::take(&mut self.buf);
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            let mut chunk = [0u8; READ_CHUNK];
            match self.reader.read(&mut chunk).await {
                Ok(0) | Err(_) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }
}

async fn read_line<R>(reader: &mut Option<LineReader<R>>) -> Option<String>
where
    R: AsyncRead + Unpin,
{
    match reader {
        Some(reader) => reader.next().await,
        None => None,
    }
}

/// Signal the whole process group led by `pid` (spawned with `process_group(0)`),
/// killing the command and every descendant it started (AC14). A negative pid
/// targets the group; a leaked orphan is a real bug this closes.
#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: `kill(2)` with a negative pid signals the process group whose id is
    // `pid`. The child leads its own group, so this reaps it and its descendants;
    // it takes no Rust references and is sound to call on a possibly-exited group
    // (a missing group yields ESRCH, which we ignore).
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> CommandTemplate {
        CommandTemplate {
            program: "sh",
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    fn fast() -> ExecLimits {
        ExecLimits {
            max_concurrent: 8,
            timeout: Duration::from_millis(400),
            max_output_bytes: 64 * 1024,
            channel_capacity: 16,
        }
    }

    /// True while a process with `pid` still exists (signal 0 is an existence probe).
    fn process_alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    async fn wait_until_dead(pid: i32) -> bool {
        for _ in 0..50 {
            if !process_alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !process_alive(pid)
    }

    async fn collect(mut handle: ExecHandle) -> (Vec<String>, ExecStatus) {
        let mut lines = Vec::new();
        loop {
            match handle.events.recv().await {
                Some(ExecEvent::Line(line)) => lines.push(line),
                Some(ExecEvent::Failed(message)) => lines.push(format!("!{message}")),
                Some(ExecEvent::Done { status, .. }) => return (lines, status),
                None => return (lines, ExecStatus::Canceled),
            }
        }
    }

    // AC10 (exec half): a shell metacharacter in an argument is passed literally,
    // never interpreted — proof that argv, not a shell string, is executed.
    #[tokio::test]
    async fn argument_is_not_shell_interpreted() {
        let engine = ExecEngine::new(fast());
        let command = CommandTemplate {
            program: "echo",
            args: vec!["a; rm -rf b".to_string()],
        };
        let handle = engine.try_start(command, None).unwrap();
        let (lines, status) = collect(handle).await;
        assert_eq!(lines, vec!["a; rm -rf b".to_string()]);
        assert_eq!(status, ExecStatus::Completed { success: true });
    }

    // AC16: output arrives incrementally — a line is delivered before the run
    // completes, not only at the end.
    #[tokio::test]
    async fn output_streams_before_completion() {
        let engine = ExecEngine::new(ExecLimits {
            timeout: Duration::from_secs(5),
            ..fast()
        });
        let handle = engine
            .try_start(sh("printf 'first\\n'; sleep 0.3; printf 'second\\n'"), None)
            .unwrap();
        let mut events = handle.events;

        let first = tokio::time::timeout(Duration::from_millis(150), events.recv())
            .await
            .expect("first line must arrive well before the 0.3s sleep completes")
            .unwrap();
        assert!(matches!(first, ExecEvent::Line(ref l) if l == "first"));

        let mut saw_second = false;
        let mut done = None;
        while let Some(event) = events.recv().await {
            match event {
                ExecEvent::Line(l) if l == "second" => saw_second = true,
                ExecEvent::Done { status, .. } => {
                    done = Some(status);
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_second, "the later line must still arrive");
        assert_eq!(done, Some(ExecStatus::Completed { success: true }));
    }

    // AC14 (crux): on timeout the whole process TREE is killed — a descendant the
    // command forked must not outlive the run.
    #[tokio::test]
    async fn timeout_kills_the_process_tree() {
        let engine = ExecEngine::new(fast()); // 400ms timeout
        let handle = engine
            .try_start(sh("sleep 300 & printf '%s\\n' \"$!\"; wait"), None)
            .unwrap();
        let mut events = handle.events;

        let child_pid: i32 = match events.recv().await.unwrap() {
            ExecEvent::Line(line) => line.trim().parse().expect("descendant pid"),
            other => panic!("expected the descendant pid line, got {other:?}"),
        };
        assert!(process_alive(child_pid), "the descendant should be running");

        let status = loop {
            match events.recv().await {
                Some(ExecEvent::Done { status, .. }) => break status,
                Some(_) => {}
                None => break ExecStatus::Canceled,
            }
        };
        assert_eq!(status, ExecStatus::TimedOut);
        assert!(
            wait_until_dead(child_pid).await,
            "the forked descendant must be killed with the group — no orphan"
        );
    }

    // AC14 / AC18 (crux): dropping the consumer (client disconnect / Cancel)
    // aborts the run and kills the descendant — no orphan.
    #[tokio::test]
    async fn disconnect_kills_the_process_tree() {
        let engine = ExecEngine::new(ExecLimits {
            timeout: Duration::from_secs(30),
            ..fast()
        });
        let handle = engine
            .try_start(sh("sleep 300 & printf '%s\\n' \"$!\"; wait"), None)
            .unwrap();
        let mut events = handle.events;

        let child_pid: i32 = match events.recv().await.unwrap() {
            ExecEvent::Line(line) => line.trim().parse().expect("descendant pid"),
            other => panic!("expected the descendant pid line, got {other:?}"),
        };
        assert!(process_alive(child_pid));

        // Simulate the browser closing the EventSource / pressing Cancel.
        drop(events);

        assert!(
            wait_until_dead(child_pid).await,
            "closing the stream must kill the descendant — no orphan"
        );
    }

    // AC41: a missing tool surfaces a clear, non-technical error and terminates —
    // no hang, no raw OS error, no panic.
    #[tokio::test]
    async fn missing_tool_surfaces_a_clear_error() {
        let engine = ExecEngine::new(fast());
        let command = CommandTemplate {
            program: "lg-nonexistent-tool-xyz",
            args: vec![],
        };
        let handle = engine.try_start(command, None).unwrap();
        let (lines, status) = collect(handle).await;
        assert_eq!(status, ExecStatus::Failed);
        assert_eq!(
            lines,
            vec!["!the diagnostic tool is not available on this node"]
        );
    }

    // AC40 (crux): at the global cap the next start is refused with no process
    // spawned, and the permit is released on completion, cancel, and timeout —
    // no leak deadlocks the node.
    #[tokio::test]
    async fn global_cap_refuses_over_limit_and_never_leaks_a_permit() {
        let engine = ExecEngine::new(ExecLimits {
            max_concurrent: 2,
            timeout: Duration::from_millis(300),
            ..fast()
        });

        let a = engine.try_start(sh("sleep 5"), None).unwrap();
        let b = engine.try_start(sh("sleep 5"), None).unwrap();
        assert_eq!(engine.available_permits(), 0, "both permits taken");

        // Over the cap: refused, and no third process is spawned.
        assert_eq!(
            engine.try_start(sh("sleep 5"), None).err(),
            Some(StartError::Busy)
        );

        // Cancel one (drop) → its permit must come back.
        drop(a);
        for _ in 0..50 {
            if engine.available_permits() >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            engine.available_permits() >= 1,
            "the permit must release on cancel — no leak"
        );

        // A fresh short run now fits, completes, and returns its permit.
        let c = engine.try_start(sh("printf done\\n"), None).unwrap();
        let (_lines, status) = collect(c).await;
        assert_eq!(status, ExecStatus::Completed { success: true });

        // Let the remaining `sleep 5` hit the 300ms timeout; its permit releases too.
        let (_l, status_b) = collect(b).await;
        assert_eq!(status_b, ExecStatus::TimedOut);

        for _ in 0..50 {
            if engine.available_permits() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            engine.available_permits(),
            2,
            "every permit is back — no leak on any exit path"
        );
    }

    // Defense in depth: a pinned target IP that is not public is refused at spawn
    // time, and no process starts.
    #[tokio::test]
    async fn spawn_refuses_a_non_public_pinned_ip() {
        let engine = ExecEngine::new(fast());
        let before = engine.available_permits();
        let result = engine.try_start(sh("echo should-not-run"), Some("10.0.0.1".parse().unwrap()));
        assert!(matches!(result, Err(StartError::Rejected(_))));
        assert_eq!(
            engine.available_permits(),
            before,
            "a refused start must not consume a permit"
        );
    }

    // Memory bound (defect fix): a process that emits a large stream with NO
    // newline must be capped and terminated at the output cap, not buffered
    // whole into one line. `head -c` bounds the shell side as a safety net; a
    // working per-line bound stops far earlier, at the 256 KiB cap.
    #[tokio::test]
    async fn no_newline_flood_is_capped_not_buffered() {
        let engine = ExecEngine::new(ExecLimits {
            max_output_bytes: 256 * 1024,
            timeout: Duration::from_secs(10),
            ..fast()
        });
        let handle = engine
            .try_start(sh("yes | tr -d '\\n' | head -c 20000000"), None)
            .unwrap();
        let mut events = handle.events;

        let mut buffered = 0usize;
        let mut status = None;
        while let Some(event) = events.recv().await {
            match event {
                ExecEvent::Line(line) => buffered += line.len(),
                ExecEvent::Done { status: done, .. } => {
                    status = Some(done);
                    break;
                }
                ExecEvent::Failed(_) => {}
            }
        }

        assert_eq!(
            status,
            Some(ExecStatus::OutputCapped),
            "the flood must stop at the output cap"
        );
        assert!(
            buffered <= 256 * 1024 + MAX_LINE_BYTES + READ_CHUNK,
            "output must stay bounded near the cap, not buffer the whole stream: {buffered} bytes"
        );
    }

    // Orphan on clean exit (defect fix): a command that exits 0 while a
    // descendant it backgrounded is still alive — the descendant must be killed
    // with the group, not reparented to init and orphaned. Its fds are
    // redirected off our pipe so the leader's exit is seen as EOF (a genuine
    // clean completion), yet it remains in the process group.
    #[tokio::test]
    async fn clean_exit_still_kills_a_backgrounded_descendant() {
        let engine = ExecEngine::new(fast());
        let handle = engine
            .try_start(
                sh("sleep 300 >/dev/null 2>&1 & printf '%s\\n' \"$!\"; exit 0"),
                None,
            )
            .unwrap();
        let mut events = handle.events;

        let child_pid: i32 = match events.recv().await.unwrap() {
            ExecEvent::Line(line) => line.trim().parse().expect("descendant pid"),
            other => panic!("expected the descendant pid line, got {other:?}"),
        };
        assert!(process_alive(child_pid), "the descendant should be running");

        let status = loop {
            match events.recv().await {
                Some(ExecEvent::Done { status, .. }) => break status,
                Some(_) => {}
                None => break ExecStatus::Canceled,
            }
        };
        assert_eq!(
            status,
            ExecStatus::Completed { success: true },
            "the leader exited 0 — a genuine completion"
        );
        assert!(
            wait_until_dead(child_pid).await,
            "a descendant backgrounded before a clean exit must still be killed — no orphan"
        );
    }
}
