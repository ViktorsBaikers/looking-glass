//! The looking-glass remote agent library. Slice 7 populates the enrollment client;
//! the outbound TLS tunnel + node executor are later slices. Exposed as a library so
//! the enrollment logic is testable from `tests/` (a binary-only crate has no such
//! seam).

pub mod dataplane;
pub mod enroll;
pub mod tunnel;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Install the process log subscriber (structured logs at `info` by default,
/// `RUST_LOG`-overridable) — the agent's only observability surface for the
/// tunnel connect / relay path.
pub fn init_tracing() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();
}
