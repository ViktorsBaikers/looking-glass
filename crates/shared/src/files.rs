//! Served-file path confinement, shared by central's local range server and the
//! agent's data plane: a test file's admin-set `source_ref` is a path *relative
//! to* the files root, and any attempt to climb out is refused. Also the socket
//! option that lets the agent's data plane serve those files over IPv4 too.

use std::path::{Component, Path, PathBuf};

/// Resolve `source_ref` to a path confined to `root`, or `None` if it would
/// escape, names something other than a regular file, or `root` itself does not
/// resolve. Two independent guards: the relative path may contain only *normal*
/// components (rejecting `..`, an absolute root, or a Windows prefix before any
/// filesystem access), and — for a path that resolves — its canonical form must
/// still sit under the canonical root (defeating a symlink that points out).
pub fn resolve_within(root: &Path, source_ref: &str) -> Option<PathBuf> {
    let relative = Path::new(source_ref);
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }

    // An empty root joins onto the working directory, and a missing one has
    // nothing to confine to: fail closed for both.
    let canonical_root = root.canonicalize().ok()?;
    let candidate = root.join(relative);
    match candidate.canonicalize() {
        // Only a regular file is served: a directory would be answered 200
        // and then fail its body.
        Ok(resolved) => {
            (resolved.starts_with(canonical_root) && resolved.is_file()).then_some(candidate)
        }
        // A component-clean path to a missing file cannot climb out of root;
        // the file server will 404 it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(candidate),
        // Any other failure (e.g. a component over 255 bytes) is unservable:
        // refuse it here, as the file server would answer its open error 500.
        Err(_) => None,
    }
}

/// Let an IPv6 socket that will bind `[::]` accept IPv4 too (as IPv4-mapped
/// addresses), whatever the host's `net.ipv6.bindv6only` default. Call it
/// before `bind`.
#[cfg(unix)]
pub fn accept_ipv4_too(socket: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let off: libc::c_int = 0;
    // SAFETY: the fd is live for the call, and the option value is a c_int of
    // the size passed.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            (&off as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_within_rejects_parent_traversal() {
        let root = Path::new("/srv/files");
        assert!(resolve_within(root, "../../etc/passwd").is_none());
        assert!(resolve_within(root, "sub/../../escape").is_none());
    }

    #[test]
    fn resolve_within_rejects_an_absolute_path() {
        let root = Path::new("/srv/files");
        assert!(resolve_within(root, "/etc/passwd").is_none());
    }

    #[test]
    fn resolve_within_accepts_a_plain_relative_path() {
        let root = unique_temp_dir("plain");
        assert_eq!(
            resolve_within(&root, "downloads/1gb.bin"),
            Some(root.join("downloads/1gb.bin"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // F-281: an empty root is the working directory to the filesystem. Cargo
    // runs this from crates/shared, whose Cargo.toml stands in for the agent's
    // credential file there: it must not be served.
    #[test]
    fn resolve_within_refuses_an_empty_root() {
        assert!(Path::new("Cargo.toml").is_file());
        assert_eq!(resolve_within(Path::new(""), "Cargo.toml"), None);
    }

    // F-281: a root that does not resolve serves nothing (fail closed).
    #[test]
    fn resolve_within_refuses_an_unresolvable_root() {
        let parent = unique_temp_dir("absent-root");
        assert_eq!(resolve_within(&parent.join("absent"), "probe.bin"), None);
        let _ = std::fs::remove_dir_all(&parent);
    }

    // F-283: a socket that is IPv6-only (as every new one is on a
    // `net.ipv6.bindv6only=1` host) takes IPv4 once this has run.
    #[cfg(unix)]
    #[tokio::test]
    async fn accept_ipv4_too_lets_an_ipv6_only_socket_take_ipv4() {
        use std::net::{Ipv4Addr, Ipv6Addr};
        use std::os::fd::AsRawFd;

        let socket = tokio::net::TcpSocket::new_v6().unwrap();
        let on: libc::c_int = 1;
        // SAFETY: a live socket and a c_int option value of the size passed.
        let set = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IPV6,
                libc::IPV6_V6ONLY,
                (&on as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        assert_eq!(set, 0, "cannot make the socket IPv6-only");

        accept_ipv4_too(&socket).unwrap();
        socket.bind((Ipv6Addr::UNSPECIFIED, 0).into()).unwrap();
        let listener = socket.listen(8).unwrap();
        let port = listener.local_addr().unwrap().port();
        let v4 = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await;
        assert!(v4.is_ok(), "IPv4 refused: {v4:?}");
    }

    /// A fresh, created temp directory unique per test (process id + counter +
    /// nanos).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lg-files-test-{}-{}-{}-{}",
            tag,
            std::process::id(),
            n,
            nanos
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    // A symlink placed under root but pointing at a file outside it must not
    // resolve — the canonicalize/starts_with guard is what stops a range read
    // from following the link out of the served directory.
    #[cfg(unix)]
    #[test]
    fn resolve_within_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("symlink-root");
        let outside = unique_temp_dir("symlink-outside");
        let target = outside.join("secret.bin");
        std::fs::write(&target, b"secret").expect("write outside target");
        symlink(&target, root.join("escape.bin")).expect("create escaping symlink");

        assert!(resolve_within(&root, "escape.bin").is_none());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    // F-293: only a regular file resolves. A directory under root, however it
    // is spelled, would be opened and answered 200 with a body that fails.
    #[test]
    fn resolve_within_refuses_a_directory() {
        let root = unique_temp_dir("directory");
        std::fs::create_dir_all(root.join("sub")).expect("create sub dir");

        for source_ref in ["sub", "sub/", "sub/.", ""] {
            assert_eq!(resolve_within(&root, source_ref), None, "{source_ref:?}");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    // A symlink under root that points at a regular file under root still
    // resolves (the served path is the link; its target stays confined), but
    // one pointing at a directory under root does not.
    #[cfg(unix)]
    #[test]
    fn resolve_within_follows_an_inner_symlink_only_to_a_file() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("inner-symlink");
        std::fs::create_dir_all(root.join("sub")).expect("create sub dir");
        std::fs::write(root.join("sub/real.bin"), b"data").expect("write target");
        symlink(root.join("sub/real.bin"), root.join("file-link.bin")).expect("file link");
        symlink(root.join("sub"), root.join("dir-link")).expect("dir link");

        assert_eq!(
            resolve_within(&root, "file-link.bin"),
            Some(root.join("file-link.bin"))
        );
        assert_eq!(resolve_within(&root, "dir-link"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    // A component-clean path that does not exist yet falls through to the
    // `Some(candidate)` arm; it must stay confined under root, not escape.
    #[test]
    fn resolve_within_confines_a_missing_file() {
        let root = unique_temp_dir("missing");

        let resolved = resolve_within(&root, "downloads/missing.bin")
            .expect("component-clean missing path stays confined");
        assert!(resolved.starts_with(&root));

        let _ = std::fs::remove_dir_all(&root);
    }

    // F-302: a name the filesystem rejects (a component over 255 bytes is
    // ENAMETOOLONG) is unservable, so it does not resolve; the callers 404 it
    // rather than hand the file server an open error it answers 500.
    #[test]
    fn resolve_within_refuses_a_name_the_filesystem_rejects() {
        let root = unique_temp_dir("name-too-long");

        assert_eq!(resolve_within(&root, &"a".repeat(256)), None);

        let _ = std::fs::remove_dir_all(&root);
    }
}
