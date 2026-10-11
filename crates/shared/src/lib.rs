//! Types and logic reused by both `central` and `agent`: the versioned wire
//! protocol, target/BGP-argument validation, served-file path confinement,
//! method templates, and the audited process-execution engine.

pub mod exec;
pub mod files;
pub mod liveness;
pub mod protocol;
pub mod template;
pub mod validate;
