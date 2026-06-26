//! `ServerProbe` driven port (T022).
//!
//! The capability/health port behind `speak health` and `speak check` (FR-14):
//! `GET /health`, `GET /v1/models`, and the runtime `POST /v1/realtime/translate`
//! capability probe that selects the SSE path versus the chunked fallback
//! (ADR-0004). The openai adapter implements it; the retry decorator wraps it.

use anyhow::Result;

/// Driven port: probe server health and capabilities.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait ServerProbe {
    /// Whether `GET /health` reports the server healthy.
    async fn health(&self) -> Result<bool>;

    /// The model ids advertised by `GET /v1/models`.
    async fn models(&self) -> Result<Vec<String>>;

    /// Whether the realtime SSE endpoint is available (runtime capability probe).
    async fn supports_realtime(&self) -> Result<bool>;
}
