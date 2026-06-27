//! Driven adapters (ADR-0003): the ONLY layer where framework crates appear.
//!
//! Each `adapters/*` type is an **Adapter** (GoF) that implements one or more
//! driven ports ([`crate::ports`]) over a concrete framework — `async-openai` /
//! `reqwest` for the server, libav for codecs, CoreAudio for I/O — translating
//! the pure [`crate::domain`] value objects to and from the wire. Dependencies
//! point inward only: adapters depend on ports + domain, never the reverse.

pub mod chatmt;
pub mod coreaudio;
pub mod headless;
pub mod libav;
pub mod openai;
pub mod presenter;
pub mod retry;
pub mod sse;
