//! Types and logic reused by both `central` and `agent`: the versioned wire
//! protocol, target/BGP-argument validation, method templates, and the audited
//! process-execution engine. Populated by later slices; this is the skeleton seam.

pub mod exec;
pub mod liveness;
pub mod protocol;
pub mod template;
pub mod validate;
