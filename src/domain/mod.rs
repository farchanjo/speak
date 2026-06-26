//! Pure domain layer: value objects with zero IO (ADR-0003).
//!
//! Nothing here performs network, filesystem, or audio IO. The driving and
//! driven adapters depend inward on these types; the dependency never points
//! the other way.

pub mod gen_params;
pub mod retry;
pub mod voice_design;
