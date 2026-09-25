//! Served-file path confinement, shared by central's local range server and the
//! agent's data plane: a test file's admin-set `source_ref` is a path *relative
//! to* the files root, and any attempt to climb out is refused.

use std::path::{Component, Path, PathBuf};

/// Resolve `source_ref` to a path confined to `root`, or `None` if it would
/// escape. Two independent guards: the relative path may contain only *normal*
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

    let candidate = root.join(relative);
    match (candidate.canonicalize(), root.canonicalize()) {
        (Ok(resolved), Ok(canonical_root)) => {
            resolved.starts_with(canonical_root).then_some(candidate)
        }
        // A component-clean path that does not yet resolve (e.g. the file is
        // missing) cannot climb out of root; the file server will 404 it.
        _ => Some(candidate),
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
        let root = Path::new("/srv/files");
        assert_eq!(
            resolve_within(root, "downloads/1gb.bin"),
            Some(PathBuf::from("/srv/files/downloads/1gb.bin"))
        );
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
}
