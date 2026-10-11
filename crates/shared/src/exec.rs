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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

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
/// clone (the semaphore and bounds are shared) so it lives in shared application
/// state, and a bound set on one clone applies to runs started from any clone.
#[derive(Clone)]
pub struct ExecEngine {
    limits: Arc<Mutex<ExecLimits>>,
    permits: Arc<Semaphore>,
}

impl ExecEngine {
    pub fn new(limits: ExecLimits) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limits.max_concurrent)),
            limits: Arc::new(Mutex::new(limits)),
        }
    }

    /// Change the per-run timeout and output cap for runs started after this
    /// call; runs already in flight keep the bounds they started with. The
    /// global cap is the permit pool, sized once in [`Self::new`].
    pub fn set_run_limits(&self, timeout: Duration, max_output_bytes: usize) {
        let mut limits = self.limits.lock().expect("exec limits mutex");
        limits.timeout = timeout;
        limits.max_output_bytes = max_output_bytes;
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

        let limits = *self.limits.lock().expect("exec limits mutex");
        let (tx, rx) = mpsc::channel(limits.channel_capacity);
        // The permit is moved into the driver, which releases it once the process
        // group is gone — on success, timeout, cancel, disconnect, or spawn failure.
        tokio::spawn(drive(command, tx, limits, permit));
        Ok(ExecHandle { events: rx })
    }
}

/// Held while a run's group leader is spawned, and from the moment a finished
/// run reaps its leader until its group loop is done. Once the leader is reaped
/// and no member is left, its pid (the pgid the loop waits on) may be recycled;
/// only a run's spawn can make a new child of ours lead a group, so no spawn
/// may land inside that window.
static SPAWN_OR_REAP: AsyncMutex<()> = AsyncMutex::const_new(());

async fn drive(
    command: CommandTemplate,
    tx: mpsc::Sender<ExecEvent>,
    limits: ExecLimits,
    permit: OwnedSemaphorePermit,
) {
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
    // The agent's systemd unit grants an ambient CAP_NET_BIND_SERVICE to bind
    // :443; a diagnostic must not inherit it. File capabilities (ping, mtr)
    // still apply at exec.
    // SAFETY: the closure runs in the forked child before exec and only calls
    // prctl(2), which is async-signal-safe and touches no memory. A kernel
    // without ambient capabilities (< 4.3) returns EINVAL: nothing to clear.
    #[cfg(target_os = "linux")]
    unsafe {
        builder.pre_exec(|| {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            );
            Ok(())
        });
    }

    let spawned = {
        let _window = SPAWN_OR_REAP.lock().await;
        builder.spawn()
    };
    let mut child = match spawned {
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

    let deadline = tokio::time::Instant::now() + limits.timeout;
    let timeout = tokio::time::sleep_until(deadline);
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
                Some(line) => match emit(&tx, &mut sent_bytes, line, limits.max_output_bytes, deadline).await {
                    Emit::Ok => {}
                    Emit::Disconnected => break Ended::Canceled,
                    Emit::Capped => break Ended::OutputCapped,
                    Emit::TimedOut => break Ended::TimedOut,
                },
                None => stdout_done = true,
            },
            line = read_line(&mut stderr), if !stderr_done => match line {
                Some(line) => match emit(&tx, &mut sent_bytes, line, limits.max_output_bytes, deadline).await {
                    Emit::Ok => {}
                    Emit::Disconnected => break Ended::Canceled,
                    Emit::Capped => break Ended::OutputCapped,
                    Emit::TimedOut => break Ended::TimedOut,
                },
                None => stderr_done = true,
            },
        }
    };

    // Both pipes closed, but a leader may close them before it exits (GNU grep
    // and echo do): give it a bounded grace to exit so its own status, not our
    // SIGKILL, is reported. The wait does not reap, so the zombie keeps the pgid
    // pinned; the deadline and a disconnect still cut it short.
    if let (Ended::Completed, Some(pid)) = (&ended, pid) {
        let grace_end = deadline.min(tokio::time::Instant::now() + EXIT_GRACE);
        tokio::select! {
            _ = tokio::task::spawn_blocking(move || wait_exited(pid)) => {}
            _ = tokio::time::sleep_until(grace_end) => {}
            _ = tx.closed() => {}
        }
    }

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
    let window = SPAWN_OR_REAP.lock().await;
    let reaped = child.wait().await;
    // Then any other group member that became our child: when central runs as
    // PID 1, members orphaned by the kill are reparented to us, and nothing
    // else would ever reap them.
    if let Some(pid) = pid {
        let _ = tokio::task::spawn_blocking(move || reap_group(pid)).await;
    }
    drop(window);
    // The process group is gone; a consumer that stopped reading must not keep
    // the global permit while the terminal events below wait for it.
    drop(permit);

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
    TimedOut,
}

async fn emit(
    tx: &mpsc::Sender<ExecEvent>,
    sent_bytes: &mut usize,
    line: String,
    max_output_bytes: usize,
    deadline: tokio::time::Instant,
) -> Emit {
    *sent_bytes = sent_bytes.saturating_add(line.len() + 1);
    // A live consumer that stopped reading parks this send; the hard timeout
    // must still fire while it waits.
    match tokio::time::timeout_at(deadline, tx.send(ExecEvent::Line(line))).await {
        Err(_) => return Emit::TimedOut,
        Ok(Err(_)) => return Emit::Disconnected,
        Ok(Ok(())) => {}
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

/// How long a leader whose output is closed may take to exit on its own
/// before the group is killed anyway.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// Block until the child `pid` has exited, WITHOUT reaping it (`WNOWAIT`): it
/// stays a zombie, so its pid and pgid cannot be recycled before the kill.
#[cfg(unix)]
fn wait_exited(pid: u32) {
    // SAFETY: an all-zero `siginfo_t` is a valid value of this plain C struct.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `waitid(2)` writes only `info`, which we own; `pid` is our
        // unreaped child.
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if waited == 0 || std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            break;
        }
    }
}

#[cfg(not(unix))]
fn wait_exited(_pid: u32) {}

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

/// Reap every already-killed member of the group led by `pid` that is our
/// child, blocking until none is left (`ECHILD`). Outside PID 1 there are
/// none and this returns at once.
#[cfg(unix)]
fn reap_group(pid: u32) {
    loop {
        // SAFETY: `waitpid(2)` with a negative pid waits for any child in that
        // process group; a null status pointer is allowed, and it touches no
        // Rust memory. Only group members we just SIGKILLed can match.
        let reaped = unsafe { libc::waitpid(-(pid as i32), std::ptr::null_mut(), 0) };
        if reaped <= 0 && std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            break;
        }
    }
}

#[cfg(not(unix))]
fn reap_group(_pid: u32) {}

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

    /// True while a process with `pid` still exists (signal 0 is an existence
    /// probe) and is not a zombie: a killed orphan stays one when nothing reaps
    /// it, as under a container whose PID 1 is cargo.
    fn process_alive(pid: i32) -> bool {
        let exists = unsafe { libc::kill(pid, 0) == 0 };
        exists && !is_zombie(pid)
    }

    #[cfg(target_os = "linux")]
    fn is_zombie(pid: i32) -> bool {
        // The state follows the parenthesised command name, which may itself
        // contain ") ".
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, rest)| rest.starts_with('Z'))
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn is_zombie(_pid: i32) -> bool {
        false
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

    /// The first line a tree-kill test's command prints: its backgrounded descendant's pid.
    async fn descendant_pid(events: &mut mpsc::Receiver<ExecEvent>) -> i32 {
        match events.recv().await.unwrap() {
            ExecEvent::Line(line) => line.trim().parse().expect("descendant pid"),
            other => panic!("expected the descendant pid line, got {other:?}"),
        }
    }

    /// Drain events until the run's terminal status (a closed channel reads as Canceled).
    async fn final_status(events: &mut mpsc::Receiver<ExecEvent>) -> ExecStatus {
        loop {
            match events.recv().await {
                Some(ExecEvent::Done { status, .. }) => break status,
                Some(_) => {}
                None => break ExecStatus::Canceled,
            }
        }
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

        let child_pid = descendant_pid(&mut events).await;
        assert!(process_alive(child_pid), "the descendant should be running");

        let status = final_status(&mut events).await;
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

        let child_pid = descendant_pid(&mut events).await;
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
    // clean completion), yet it remains in the process group. The leader holds
    // its exit until SIGUSR1, sent once the descendant is seen running, so the
    // engine's kill cannot race that precondition.
    #[tokio::test]
    async fn clean_exit_still_kills_a_backgrounded_descendant() {
        let engine = ExecEngine::new(fast());
        let handle = engine
            .try_start(
                sh("trap 'exit 0' USR1; sleep 300 >/dev/null 2>&1 & \
                    printf '%s\\n' \"$!\" \"$$\"; while :; do sleep 0.01; done"),
                None,
            )
            .unwrap();
        let mut events = handle.events;

        let child_pid = descendant_pid(&mut events).await;
        let leader = descendant_pid(&mut events).await;
        assert!(process_alive(child_pid), "the descendant should be running");
        // SAFETY: kill(2) takes no Rust memory; `leader` is the run's live shell.
        unsafe { libc::kill(leader, libc::SIGUSR1) };

        let status = final_status(&mut events).await;
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

    // F-175: a leader may close stdout and stderr before it exits (GNU grep and
    // echo do, via gnulib's close_stdout). Its own exit status must be reported,
    // not the SIGKILL of a group kill that raced its exit.
    #[tokio::test]
    async fn output_closed_before_exit_still_reports_the_exit_status() {
        let engine = ExecEngine::new(ExecLimits {
            timeout: Duration::from_secs(5),
            ..fast()
        });
        let mut wrong = Vec::new();
        for _ in 0..200 {
            let handle = engine
                .try_start(sh("exec >&- 2>&-; sleep 0.01; exit 0"), None)
                .unwrap();
            let (_, status) = collect(handle).await;
            if status != (ExecStatus::Completed { success: true }) {
                wrong.push(status);
            }
        }
        assert!(
            wrong.is_empty(),
            "{} of 200 runs whose leader exited 0 were reported {:?}",
            wrong.len(),
            wrong.first()
        );
    }

    // The grace is bounded: a leader that closed its output but keeps running
    // is killed shortly after, as before, not left until the hard timeout.
    #[tokio::test]
    async fn output_closed_leader_that_keeps_running_is_killed_after_the_grace() {
        let engine = ExecEngine::new(ExecLimits {
            timeout: Duration::from_secs(10),
            ..fast()
        });
        let mut events = engine
            .try_start(sh("exec >&- 2>&-; sleep 300"), None)
            .unwrap()
            .events;
        match events.recv().await {
            Some(ExecEvent::Done { status, elapsed_ms }) => {
                assert_eq!(status, ExecStatus::Completed { success: false });
                assert!(
                    elapsed_ms < 5_000,
                    "killed after {elapsed_ms} ms, not after the grace"
                );
            }
            other => panic!("expected the terminal event, got {other:?}"),
        }
    }

    // When central is PID 1, group members orphaned by the kill (mtr's
    // mtr-packet) are reparented to us. The engine must reap the whole group,
    // not only the leader, or each one stays a zombie. A process of ours placed
    // in the run's group stands in for that adopted orphan.
    #[tokio::test]
    async fn killed_group_leaves_no_zombie() {
        use std::os::unix::process::CommandExt;

        let engine = ExecEngine::new(fast()); // 400ms timeout
        let handle = engine
            .try_start(sh("printf '%s\\n' \"$$\"; sleep 300"), None)
            .unwrap();
        let mut events = handle.events;
        let pgid = descendant_pid(&mut events).await;

        let mut member = std::process::Command::new("sleep")
            .arg("300")
            .process_group(pgid)
            .spawn()
            .expect("join the run's process group");

        assert_eq!(final_status(&mut events).await, ExecStatus::TimedOut);
        let left = member.try_wait();
        assert!(
            left.is_err(),
            "the killed group member must already be reaped, not left a zombie: {left:?}"
        );
    }

    /// Put CAP_NET_BIND_SERVICE into this thread's inheritable and ambient sets,
    /// as the agent's systemd `AmbientCapabilities=` does. Needs the capability
    /// in the permitted set (root); returns whether the raise succeeded.
    #[cfg(target_os = "linux")]
    fn raise_ambient_net_bind_service() -> bool {
        const CAP_NET_BIND_SERVICE: u32 = 10;
        #[repr(C)]
        struct Header {
            version: u32,
            pid: libc::c_int,
        }
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        // _LINUX_CAPABILITY_VERSION_3; pid 0 is the calling thread.
        let mut header = Header {
            version: 0x2008_0522,
            pid: 0,
        };
        let mut data = [Data::default(); 2];
        // SAFETY: capget/capset read and write exactly these repr(C) structs, and
        // prctl touches no memory. Capabilities are per thread, so only this test
        // thread (and what it spawns) changes.
        unsafe {
            if libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) != 0 {
                return false;
            }
            data[0].inheritable |= 1 << CAP_NET_BIND_SERVICE;
            if libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) != 0 {
                return false;
            }
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong,
                CAP_NET_BIND_SERVICE as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            ) == 0
        }
    }

    // C-098: the agent runs with an ambient CAP_NET_BIND_SERVICE to bind :443;
    // a diagnostic child must start with an empty ambient set. The raise shows
    // the inheritance where the test can raise one (root); the run is spawned
    // from this thread (current-thread runtime), so the child would inherit it.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn child_starts_with_an_empty_ambient_capability_set() {
        let raised = raise_ambient_net_bind_service();
        eprintln!("ambient CAP_NET_BIND_SERVICE raised on the test thread: {raised}");
        let engine = ExecEngine::new(fast());
        // Shell builtins only: the child that reads its own status keeps its
        // output open until it exits.
        let command = sh("while IFS= read -r l; do case $l in CapAmb:*) printf '%s\\n' \"$l\";; esac; done </proc/self/status");
        let (lines, status) = collect(engine.try_start(command, None).unwrap()).await;
        assert_eq!(status, ExecStatus::Completed { success: true });
        assert_eq!(
            lines,
            vec!["CapAmb:\t0000000000000000"],
            "a spawned child must not inherit the agent's ambient capabilities"
        );
        if raised {
            let own = std::fs::read_to_string("/proc/thread-self/status").unwrap();
            assert!(
                own.lines().any(|l| l == "CapAmb:\t0000000000000400"),
                "the clear must happen in the child only; the spawning thread keeps its ambient set"
            );
        }
    }

    // A live consumer that stops reading must not hold the run past the
    // hard timeout — the process is killed and its global permit released.
    #[tokio::test]
    async fn stalled_consumer_cannot_outlive_the_timeout() {
        let engine = ExecEngine::new(ExecLimits {
            max_concurrent: 1,
            timeout: Duration::from_millis(300),
            ..fast() // channel of 16
        });
        let handle = engine
            .try_start(sh("printf '%s\\n' \"$$\"; seq 1000; sleep 300"), None)
            .unwrap();
        let mut events = handle.events;
        let leader = descendant_pid(&mut events).await;

        // Stop reading: the channel fills and the driver parks on send.
        let mut released = false;
        for _ in 0..150 {
            if engine.available_permits() == 1 && !process_alive(leader) {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            released,
            "3s (10x the timeout) later the stalled run still holds its process or permit"
        );

        // Nothing is lost for a consumer that resumes: the run ends TimedOut.
        assert_eq!(final_status(&mut events).await, ExecStatus::TimedOut);
    }
}
