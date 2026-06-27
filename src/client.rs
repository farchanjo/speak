//! Shared HTTP client builder for the OpenAI-compatible speech server.
//!
//! The single tuned, warm keep-alive [`reqwest::Client`] every network adapter
//! reuses. The `openai` adapter ([`crate::adapters::openai`]) builds its pool
//! here and the daemon's warm Facade rides that same pool; the request/response
//! shaping and retry now live in the adapters + `application` layers, not here.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;

/// Build one warm, pooled [`reqwest::Client`] from the `[server]` tuning knobs.
///
/// Shared so every network adapter reuses an identically-configured keep-alive
/// pool (ADR-0004): the `openai` adapter passes a clone to `async-openai` *and*
/// keeps one for the extended speech / `/tts` / voice-CRUD requests the typed API
/// cannot express, so a single warm connection pool backs every call it makes.
pub fn build_http_client(s: &crate::config::Server) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(s.user_agent.clone())
        .pool_max_idle_per_host(s.pool_max_idle)
        .pool_idle_timeout(Duration::from_secs(s.pool_idle_timeout))
        .tcp_keepalive(Duration::from_secs(s.tcp_keepalive))
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(s.connect_timeout))
        .timeout(Duration::from_secs(s.timeout));
    if s.http2 {
        builder = builder.http2_prior_knowledge();
    }
    builder.build().context("building HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Server;

    fn server() -> Server {
        Server {
            host: "http://localhost:8800".to_owned(),
            api_key: None,
            timeout: 30,
            connect_timeout: 5,
            pool_max_idle: 8,
            pool_idle_timeout: 90,
            tcp_keepalive: 60,
            http2: false,
            user_agent: "speak/test".to_owned(),
        }
    }

    #[test]
    fn builds_a_pool_from_server_knobs() {
        assert!(build_http_client(&server()).is_ok());
    }

    #[test]
    fn builds_with_http2_prior_knowledge() {
        let mut s = server();
        s.http2 = true;
        assert!(build_http_client(&s).is_ok());
    }
}
