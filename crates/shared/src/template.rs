//! Method command templates — the second half of the injection boundary.
//!
//! A [`Method`] maps to a fixed program plus a fixed argument list, with the validated
//! target appended as one discrete argument. Nothing here ever builds a shell string:
//! the output is an argv (program + `Vec<String>` args) meant to be spawned directly
//! (`Command::new(program).args(args)`), so a target can never be interpreted as a shell
//! command however it is spelled. This is the basis for AC10/AC13.
//!
//! BGP (`bgp`/`bgp6`) does not go through [`Method`] — it takes a grammar-validated
//! prefix, not a diagnostic target, and shells to a routing daemon's read-only CLI.
//! It has its own [`BgpDaemon`] command builder and a stubbable [`DaemonProbe`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::validate::{PrefixFamily, ValidatedPrefix, ValidatedTarget};

/// A diagnostic method offered by a location. Each variant is locked to an address
/// family so a v4 target cannot reach a v6 tool and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Ping,
    Ping6,
    Mtr,
    Mtr6,
    Traceroute,
    Traceroute6,
}

/// A ready-to-spawn command: a program name and its discrete arguments. The target is
/// always the final element of `args` — never concatenated into any other string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTemplate {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl Method {
    /// Build the command for this method against a validated target. The pinned public IP
    /// is appended as the sole target argument; `-n` keeps every tool numeric so it never
    /// performs its own reverse lookup on top of the address we already validated.
    pub fn command(self, target: &ValidatedTarget) -> CommandTemplate {
        let (program, flags): (&'static str, &[&str]) = match self {
            Method::Ping => ("ping", &["-4", "-n", "-c", "4", "-w", "10"]),
            Method::Ping6 => ("ping", &["-6", "-n", "-c", "4", "-w", "10"]),
            Method::Mtr => ("mtr", &["--report", "--report-cycles", "4", "-n", "-4"]),
            Method::Mtr6 => ("mtr", &["--report", "--report-cycles", "4", "-n", "-6"]),
            Method::Traceroute => ("traceroute", &["-4", "-n", "-q", "1"]),
            Method::Traceroute6 => ("traceroute", &["-6", "-n", "-q", "1"]),
        };
        let mut args: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
        args.push(target.arg());
        CommandTemplate { program, args }
    }
}

/// A supported routing daemon whose read-only control CLI answers BGP route
/// queries. BGP shells to exactly one of these with a fixed `show`-only verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgpDaemon {
    /// BIRD, queried through `birdc`.
    Bird,
    /// FRR, queried through `vtysh`.
    Frr,
}

impl BgpDaemon {
    /// The bare program name this daemon's read-only CLI is invoked as — `birdc`
    /// for BIRD, `vtysh` for FRR. This is the single source for both halves of the
    /// BGP access contract: [`PathDaemonProbe`] looks this name up on `PATH` and
    /// [`command`](Self::command) puts the same name in `program`, so the binary the
    /// agent probes and the binary it runs are provably one file. The operator's
    /// scoped, read-only wrapper is installed under this name ahead of the real
    /// client on the service `PATH` (see `README.md`); there is no broad
    /// daemon-group grant.
    pub fn program(self) -> &'static str {
        match self {
            BgpDaemon::Bird => "birdc",
            BgpDaemon::Frr => "vtysh",
        }
    }

    /// Build the fixed, read-only route-inspection command for `prefix`.
    ///
    /// The verb is hardcoded per daemon — `birdc show route for <prefix>` (BIRD),
    /// `vtysh -c "show ip bgp <prefix>"` / `vtysh -c "show bgp ipv6 <prefix>"`
    /// (FRR) — so no configuration or state-changing verb is ever reachable through
    /// this method (AC15/FR-036). The only variable is the grammar-validated,
    /// canonical prefix token: for `birdc` it is its own discrete final argument;
    /// for `vtysh` it is appended inside the single fixed `-c` command word (the
    /// whole `show …` string is one argv element, never a shell string, and the
    /// prefix carries no whitespace or metacharacter — [`crate::validate::bgp_arg`]).
    pub fn command(self, prefix: &ValidatedPrefix) -> CommandTemplate {
        match self {
            BgpDaemon::Bird => CommandTemplate {
                program: self.program(),
                args: vec![
                    "show".to_string(),
                    "route".to_string(),
                    "for".to_string(),
                    prefix.arg().to_string(),
                ],
            },
            BgpDaemon::Frr => {
                let show = match prefix.family() {
                    PrefixFamily::V4 => format!("show ip bgp {}", prefix.arg()),
                    PrefixFamily::V6 => format!("show bgp ipv6 {}", prefix.arg()),
                };
                CommandTemplate {
                    program: self.program(),
                    args: vec!["-c".to_string(), show],
                }
            }
        }
    }
}

/// Detects which supported routing daemon's read-only CLI is available on this
/// node. Injectable so a test can simulate a daemon present or absent WITHOUT a
/// live BIRD/FRR install; the production [`PathDaemonProbe`] looks for `birdc` /
/// `vtysh` on `PATH`. BGP is offered only where a daemon is present (FR-036) — an
/// absent daemon makes the method unavailable, node-side, at run time.
pub trait DaemonProbe: Send + Sync {
    fn detect(&self) -> Option<BgpDaemon>;
}

/// The environment variable naming the agent's scoped BGP wrapper directory — the
/// single directory [`ScopedDaemonProbe`] is allowed to resolve `birdc`/`vtysh` in.
/// The installer sets it in the service unit to the directory it prepends to the
/// service `PATH`.
pub const BGP_WRAPPER_DIR_ENV: &str = "LG_AGENT_BGP_WRAPPER_DIR";

/// A full-`PATH` probe for the daemon CLIs, for a node the operator runs directly
/// (central's local node). BIRD is preferred when both are present. A pure lookup —
/// it never runs the binary, so probing has no effect on the daemon.
///
/// This is deliberately NOT the agent's probe: on a real BIRD/FRR router the system
/// client sits on `PATH` (`/usr/sbin/birdc`, `/usr/bin/vtysh`), so a full-`PATH`
/// probe would happily reach the UNSCOPED daemon. The least-privilege agent uses
/// [`ScopedDaemonProbe`] instead, which fails closed unless a scoped wrapper is
/// installed.
pub struct PathDaemonProbe;

impl DaemonProbe for PathDaemonProbe {
    fn detect(&self) -> Option<BgpDaemon> {
        daemon_on_path(std::env::var_os("PATH").as_deref())
    }
}

/// The least-privilege agent probe: resolves `birdc`/`vtysh` ONLY inside a single
/// operator-controlled wrapper directory, never the full `PATH`. This is the agent's
/// BGP access contract (AC38). On a real router the unscoped system client is on
/// `PATH`; restricting resolution to the agent's wrapper directory means that with no
/// scoped read-only wrapper installed there, [`detect`](Self::detect) returns `None`
/// and BGP fails closed with the agent's existing clear refusal — it never falls
/// through to `/usr/sbin/birdc`. The directory is the one the installer controls and
/// prepends to the service `PATH`; [`from_env`](Self::from_env) reads it from
/// [`BGP_WRAPPER_DIR_ENV`], and an unset variable is itself fail-closed (`None`).
pub struct ScopedDaemonProbe {
    wrapper_dir: Option<PathBuf>,
}

impl ScopedDaemonProbe {
    /// Read the scoped wrapper directory from [`BGP_WRAPPER_DIR_ENV`]. An unset or
    /// empty value yields a probe that detects nothing — BGP fails closed until a
    /// scoped wrapper directory is configured.
    pub fn from_env() -> Self {
        let wrapper_dir = std::env::var_os(BGP_WRAPPER_DIR_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self { wrapper_dir }
    }

    /// Construct a probe scoped to an explicit directory — the deterministic core,
    /// testable without mutating the process environment.
    pub fn for_dir(wrapper_dir: Option<PathBuf>) -> Self {
        Self { wrapper_dir }
    }
}

impl DaemonProbe for ScopedDaemonProbe {
    fn detect(&self) -> Option<BgpDaemon> {
        let dir = self.wrapper_dir.as_deref()?;
        daemon_in_dir(dir)
    }
}

/// True when `path` is a regular file the current process can execute — the same
/// property `execvp` requires, so a probe/grant target this accepts is one the
/// service (running as itself) can actually run. On a non-unix host, executability
/// isn't modelled, so a regular file suffices.
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Resolve a bare program name to the absolute binary that will actually run: the
/// FIRST *executable* file of that name in `PATH` order — the same first-match rule
/// `execvp` (and therefore [`crate::exec`], which spawns `program` by bare name)
/// uses. Executability is part of the match, not just existence: a non-executable
/// file earlier on `PATH` is one `execvp` skips, so skipping it here keeps this
/// resolution and the spawned binary the same file. This is the grant-path seam
/// behind AC38 — the installer grants the raw-socket capability to the binary this
/// resolves to under the service `PATH`. Pure over an explicit `PATH` so it is
/// deterministically testable without mutating the process environment.
pub fn resolve_on_path(path: Option<&OsStr>, name: &str) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

/// Resolve the available daemon from an explicit `PATH` value — the pure core of
/// [`PathDaemonProbe`] (central's local node). BIRD is preferred when both are
/// present; each daemon is looked up by [`BgpDaemon::program`], the same name
/// [`BgpDaemon::command`] runs, so the probed and executed binaries are one file.
fn daemon_on_path(path: Option<&OsStr>) -> Option<BgpDaemon> {
    [BgpDaemon::Bird, BgpDaemon::Frr]
        .into_iter()
        .find(|daemon| resolve_on_path(path, daemon.program()).is_some())
}

/// Resolve the available daemon inside a single scoped directory — the pure core of
/// [`ScopedDaemonProbe`]. A daemon counts as present only when an *executable*
/// wrapper of its [`program`](BgpDaemon::program) name lives directly in `dir`;
/// there is no `PATH` fall-through, so an absent wrapper fails closed.
fn daemon_in_dir(dir: &Path) -> Option<BgpDaemon> {
    [BgpDaemon::Bird, BgpDaemon::Frr]
        .into_iter()
        .find(|daemon| is_executable_file(&dir.join(daemon.program())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{bgp_arg, validate_target};

    struct StubResolver;
    impl crate::validate::HostResolver for StubResolver {
        async fn resolve(
            &self,
            _host: &str,
        ) -> Result<Vec<std::net::IpAddr>, crate::validate::ResolveError> {
            Ok(vec![])
        }
    }

    async fn target(ip: &str) -> ValidatedTarget {
        validate_target(ip, &StubResolver).await.unwrap()
    }

    #[tokio::test]
    async fn target_is_a_discrete_final_argument() {
        let t = target("8.8.8.8").await;
        let cmd = Method::Ping.command(&t);
        // The target is its own element, exactly the pinned IP — not merged with a flag.
        assert_eq!(cmd.args.last().map(String::as_str), Some("8.8.8.8"));
        assert!(cmd.args.iter().filter(|a| a.contains("8.8.8.8")).count() == 1);
    }

    #[tokio::test]
    async fn program_is_a_bare_binary_never_a_shell() {
        let t = target("8.8.8.8").await;
        for method in [Method::Ping, Method::Mtr, Method::Traceroute] {
            let cmd = method.command(&t);
            assert!(!cmd.program.contains('/'));
            assert!(!["sh", "bash", "zsh", "cmd", "powershell"].contains(&cmd.program));
        }
    }

    #[tokio::test]
    async fn no_argument_is_a_concatenated_shell_string() {
        let t = target("1.1.1.1").await;
        for method in [
            Method::Ping,
            Method::Ping6,
            Method::Mtr,
            Method::Mtr6,
            Method::Traceroute,
            Method::Traceroute6,
        ] {
            let cmd = method.command(&t);
            // No single argument smuggles the target next to another token or a
            // shell metacharacter — every arg is one clean token.
            for arg in &cmd.args {
                assert!(!arg.contains(' '), "arg {arg:?} contains a space");
                assert!(!arg.contains(';') && !arg.contains('&') && !arg.contains('|'));
            }
            assert_eq!(cmd.args.last().map(String::as_str), Some("1.1.1.1"));
        }
    }

    #[tokio::test]
    async fn each_method_maps_to_expected_program_and_family() {
        let t = target("8.8.8.8").await;
        assert_eq!(Method::Ping.command(&t).program, "ping");
        assert!(Method::Ping.command(&t).args.contains(&"-4".to_string()));
        assert!(Method::Ping6.command(&t).args.contains(&"-6".to_string()));
        assert_eq!(Method::Mtr.command(&t).program, "mtr");
        assert!(Method::Mtr6.command(&t).args.contains(&"-6".to_string()));
        assert_eq!(Method::Traceroute.command(&t).program, "traceroute");
        assert!(Method::Traceroute6
            .command(&t)
            .args
            .contains(&"-6".to_string()));
    }

    // ---- BGP command templates (Slice 11 / T4) -------------------------------

    #[test]
    fn bird_command_is_a_fixed_show_route_query() {
        let prefix = bgp_arg("203.0.113.0/24", PrefixFamily::V4).unwrap();
        let cmd = BgpDaemon::Bird.command(&prefix);
        assert_eq!(cmd.program, "birdc");
        assert_eq!(cmd.args, vec!["show", "route", "for", "203.0.113.0/24"]);
        // The prefix is its own discrete final token — not merged with a verb.
        assert_eq!(cmd.args.last().map(String::as_str), Some("203.0.113.0/24"));
    }

    #[test]
    fn frr_v4_command_is_a_fixed_show_ip_bgp_query() {
        let prefix = bgp_arg("8.8.8.8", PrefixFamily::V4).unwrap();
        let cmd = BgpDaemon::Frr.command(&prefix);
        assert_eq!(cmd.program, "vtysh");
        // vtysh takes the whole read-only command as one -c argument.
        assert_eq!(cmd.args, vec!["-c", "show ip bgp 8.8.8.8"]);
    }

    #[test]
    fn frr_v6_command_is_a_fixed_show_bgp_ipv6_query() {
        let prefix = bgp_arg("2001:db8::/32", PrefixFamily::V6).unwrap();
        let cmd = BgpDaemon::Frr.command(&prefix);
        assert_eq!(cmd.program, "vtysh");
        assert_eq!(cmd.args, vec!["-c", "show bgp ipv6 2001:db8::/32"]);
    }

    // Read-only by construction: the generated command begins with a `show` verb
    // and contains no configuration/state-changing verb for either daemon (AC15).
    #[test]
    fn bgp_command_is_read_only_show_only_for_every_daemon() {
        let v4 = bgp_arg("10.0.0.0/8", PrefixFamily::V4).unwrap();
        let v6 = bgp_arg("2001:db8::/48", PrefixFamily::V6).unwrap();
        let commands = [
            BgpDaemon::Bird.command(&v4),
            BgpDaemon::Frr.command(&v4),
            BgpDaemon::Frr.command(&v6),
        ];
        for cmd in commands {
            let joined = format!("{} {}", cmd.program, cmd.args.join(" "));
            assert!(
                joined.contains("show"),
                "a BGP command must be a show query: {joined:?}"
            );
            for forbidden in [
                "configure",
                "conf t",
                "add",
                "delete",
                "disable",
                "enable",
                "set ",
                "clear",
                "write",
                "reload",
                "no ",
            ] {
                assert!(
                    !joined.contains(forbidden),
                    "a read-only BGP command must never carry {forbidden:?}: {joined:?}"
                );
            }
        }
    }

    /// A collision-free temp directory for a PATH fixture — unique across parallel
    /// tests via pid + high-resolution clock + a process-wide sequence counter.
    fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "{tag}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write an *executable* fake binary — the property `execvp` and the resolvers
    /// require, so a fixture models a runnable tool rather than a bare file.
    fn write_exec(path: &std::path::Path, body: &[u8]) {
        std::fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    }

    #[test]
    fn daemon_on_path_detects_birdc_then_vtysh_then_none() {
        use std::fs;

        let base = unique_tmp_dir("lg-bgp-probe");
        let bird_dir = base.join("bird");
        let frr_dir = base.join("frr");
        let empty_dir = base.join("empty");
        for dir in [&bird_dir, &frr_dir, &empty_dir] {
            fs::create_dir_all(dir).unwrap();
        }
        write_exec(&bird_dir.join("birdc"), b"");
        write_exec(&frr_dir.join("vtysh"), b"");

        let bird_path = std::env::join_paths([&empty_dir, &bird_dir]).unwrap();
        let frr_path = std::env::join_paths([&empty_dir, &frr_dir]).unwrap();
        let empty_path = std::env::join_paths([&empty_dir]).unwrap();

        assert_eq!(
            daemon_on_path(Some(bird_path.as_os_str())),
            Some(BgpDaemon::Bird)
        );
        assert_eq!(
            daemon_on_path(Some(frr_path.as_os_str())),
            Some(BgpDaemon::Frr)
        );
        assert_eq!(daemon_on_path(Some(empty_path.as_os_str())), None);
        assert_eq!(daemon_on_path(None), None);

        let _ = fs::remove_dir_all(&base);
    }

    // ---- Least-privilege grant path (Slice 12c / AC38) -----------------------

    // The grant-path seam resolves a bare program name to the FIRST executable match
    // in PATH order — the exact binary execvp runs — so the installer's raw-socket
    // grant targets the same `ping` the service executes and the two cannot diverge.
    #[test]
    fn resolve_on_path_returns_the_first_match_in_path_order() {
        use std::fs;
        let base = unique_tmp_dir("lg-grant-path");
        let first = base.join("first");
        let second = base.join("second");
        for dir in [&first, &second] {
            fs::create_dir_all(dir).unwrap();
        }
        // Two distinct executable `ping` binaries, one on each PATH entry.
        write_exec(&first.join("ping"), b"#first");
        write_exec(&second.join("ping"), b"#second");

        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(
            resolve_on_path(Some(path.as_os_str()), "ping"),
            Some(first.join("ping")),
            "the grant must target the FIRST ping on PATH — the one execvp runs"
        );
        // Flip the PATH order and the resolved binary flips with it: proof it is
        // genuinely first-match, so grant and run track one PATH, never a fixed guess.
        let flipped = std::env::join_paths([&second, &first]).unwrap();
        assert_eq!(
            resolve_on_path(Some(flipped.as_os_str()), "ping"),
            Some(second.join("ping")),
        );
        assert_eq!(resolve_on_path(Some(path.as_os_str()), "mtr"), None);
        assert_eq!(resolve_on_path(None, "ping"), None);

        let _ = fs::remove_dir_all(&base);
    }

    // Executability is part of the match: a NON-executable `ping` earlier on PATH is
    // one execvp skips, so the grant seam must skip it too and resolve the later
    // executable one. Otherwise the grant lands on a file the service can never run.
    #[test]
    fn resolve_on_path_skips_a_non_executable_earlier_match() {
        use std::fs;
        let base = unique_tmp_dir("lg-grant-nonexec");
        let first = base.join("first");
        let second = base.join("second");
        for dir in [&first, &second] {
            fs::create_dir_all(dir).unwrap();
        }
        // First `ping` is present but not executable; second is executable.
        fs::write(first.join("ping"), b"#not-exec").unwrap();
        write_exec(&second.join("ping"), b"#exec");

        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(
            resolve_on_path(Some(path.as_os_str()), "ping"),
            Some(second.join("ping")),
            "a non-executable earlier ping must be skipped — execvp would skip it"
        );

        let _ = fs::remove_dir_all(&base);
    }

    // CRITICAL fail-closed: the dangerous common case. An unscoped system `birdc` is
    // on PATH (a real BIRD router), but no scoped wrapper is in the agent's wrapper
    // directory. The scoped probe resolves ONLY its wrapper dir, so it detects
    // nothing and BGP fails closed with the agent's existing clear refusal — it never
    // falls through to the unscoped system client.
    #[test]
    fn scoped_probe_fails_closed_when_only_a_system_client_is_on_path() {
        use std::fs;
        let base = unique_tmp_dir("lg-bgp-scoped-failclosed");
        let system_dir = base.join("usr-sbin");
        let wrapper_dir = base.join("agent-wrapper"); // exists, but empty of wrappers
        for dir in [&system_dir, &wrapper_dir] {
            fs::create_dir_all(dir).unwrap();
        }
        // The real router's system client, executable, on PATH — but NOT scoped.
        write_exec(&system_dir.join("birdc"), b"#system-birdc");

        // A full-PATH probe WOULD reach the unscoped client (this is exactly why the
        // agent must not use it) — pin that so the regression is unambiguous.
        let full_path = std::env::join_paths([&system_dir]).unwrap();
        assert_eq!(
            daemon_on_path(Some(full_path.as_os_str())),
            Some(BgpDaemon::Bird),
            "a full-PATH probe reaches the unscoped system client — the danger"
        );

        // The agent's scoped probe, pointed at its own (wrapper-less) directory,
        // fails closed regardless of what sits on the wider PATH.
        let probe = ScopedDaemonProbe::for_dir(Some(wrapper_dir.clone()));
        assert_eq!(
            probe.detect(),
            None,
            "no scoped wrapper installed must fail closed, never reach system birdc"
        );

        let _ = fs::remove_dir_all(&base);
    }

    // The scoped wrapper contract: an EXECUTABLE wrapper under the exact probe name in
    // the scoped directory is what the probe detects (and what exec runs — same name,
    // same dir). Proven by resolving the executable, not by comparing literals.
    #[test]
    fn scoped_probe_detects_only_an_executable_wrapper_in_scope() {
        use std::fs;
        let base = unique_tmp_dir("lg-bgp-scoped-detect");
        let wrapper_dir = base.join("agent-wrapper");
        fs::create_dir_all(&wrapper_dir).unwrap();

        // A present-but-non-executable wrapper is not a runnable tool: fail closed.
        fs::write(wrapper_dir.join("birdc"), b"#not-exec").unwrap();
        assert_eq!(
            ScopedDaemonProbe::for_dir(Some(wrapper_dir.clone())).detect(),
            None,
            "a non-executable wrapper is not runnable — must not be detected"
        );

        // Make it executable and it is detected as the scoped BIRD wrapper; the same
        // absolute file resolves for the program name exec will spawn.
        write_exec(&wrapper_dir.join("birdc"), b"#!/bin/sh\n");
        assert_eq!(
            ScopedDaemonProbe::for_dir(Some(wrapper_dir.clone())).detect(),
            Some(BgpDaemon::Bird)
        );
        assert_eq!(
            resolve_on_path(
                std::env::join_paths([&wrapper_dir]).ok().as_deref(),
                BgpDaemon::Bird.program()
            ),
            Some(wrapper_dir.join("birdc")),
            "the detected wrapper is the exact file exec resolves for `birdc`"
        );

        let _ = fs::remove_dir_all(&base);
    }

    // An unconfigured scoped directory (unset env / no dir) is itself fail-closed.
    #[test]
    fn scoped_probe_is_fail_closed_when_unconfigured() {
        assert_eq!(ScopedDaemonProbe::for_dir(None).detect(), None);
    }
}
