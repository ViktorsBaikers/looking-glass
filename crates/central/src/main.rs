use std::future::Future;
use std::net::SocketAddr;

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    central::init_tracing();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let shutdown = shutdown_signal();
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "central listening");
    // On SIGTERM/SIGINT stop accepting and return: leaving `main` drops the
    // runtime and every connection task, so the store closes cleanly instead of
    // the process being killed mid-write (or, as a container's PID 1, ignoring
    // `docker stop` until it escalates to SIGKILL). In-flight runs end without a
    // terminal event; their streams are long-lived, so draining them would
    // outlast any stop timeout.
    tokio::select! {
        served = central::serve(listener, central::app()) => served,
        () = shutdown => {
            tracing::info!("central shutting down");
            Ok(())
        }
    }
}

/// Resolves on the first SIGTERM or SIGINT. The handlers are installed when this
/// is called, not when first polled, so a signal cannot slip in before them.
fn shutdown_signal() -> impl Future<Output = ()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install the SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("install the SIGINT handler");
        async move {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
    }
    #[cfg(not(unix))]
    async {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// The child half of the SIGTERM test: runs the real `main` in a re-exec'd
    /// copy of this test binary. Ignored so a normal run never starts a server.
    #[test]
    #[ignore = "child half of sigterm_ends_serve_gracefully_within_a_bound"]
    fn sigterm_child() {
        super::main().expect("central exits cleanly");
    }

    // SIGTERM (what `docker stop` sends) ends `serve()` with a clean
    // exit within a bound, instead of being killed by the signal or ignored
    // as PID 1 until the stop timeout escalates to SIGKILL.
    #[test]
    fn sigterm_ends_serve_gracefully_within_a_bound() {
        let dir = std::env::temp_dir().join(format!("lg-t014-sigterm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The child inherits none of this process's LG_* environment (F-375),
        // as lib.rs's test-only `child_command` does, which this binary cannot reach.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| !key.to_string_lossy().starts_with("LG_")))
            .args([
                "tests::sigterm_child",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("PORT", "0")
            .env("LG_DB_PATH", dir.join("lg.redb"))
            .env("LG_FILES_DIR", dir.join("files"))
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        // Wait until it is serving, so the signal lands on a running server.
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut lines = stdout.lines();
        assert!(
            lines.any(|line| line.is_ok_and(|l| l.contains("central listening"))),
            "central never reported that it was listening"
        );

        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = std::fs::remove_dir_all(&dir);

        let status = status.expect("central must stop within 5s of SIGTERM");
        assert!(
            status.success(),
            "SIGTERM must end serve() with a clean exit, got {status:?} (signal {:?})",
            status.signal()
        );
    }
}
