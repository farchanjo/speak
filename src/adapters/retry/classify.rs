//! Map a failed call's `anyhow::Error` to a pure retry [`ErrorKind`].
//!
//! Classification is the one place the transport-specific error shapes
//! (`reqwest::Error`, `async_openai::error::OpenAIError`, our [`HttpStatusError`])
//! are inspected; the decorator and policy stay framework-free. Anything not
//! recognised as a transient transport failure classifies as
//! [`ErrorKind::Other`], which the policy never retries.

use async_openai::error::OpenAIError;

use super::HttpStatusError;
use crate::domain::retry::ErrorKind;

/// HTTP 429 Too Many Requests.
const TOO_MANY: u16 = 429;
/// Lower bound (inclusive) of the HTTP 5xx server-error range.
const SERVER_ERR_LO: u16 = 500;
/// Upper bound (exclusive) of the HTTP 5xx server-error range.
const SERVER_ERR_HI: u16 = 600;

/// Classify a transport-level error by its shared `reqwest` surface
/// (`is_connect`/`is_timeout`/`status`). A macro — not a function — because the
/// crate links two `reqwest` majors (its own + the one inside `async-openai`),
/// whose `Error` types are distinct yet expose the same methods.
macro_rules! classify_transport {
    ($err:expr) => {{
        let e = $err;
        if e.is_connect() {
            ErrorKind::Connect
        } else if e.is_timeout() {
            ErrorKind::Timeout
        } else if let Some(status) = e.status() {
            classify_status(status.as_u16())
        } else {
            ErrorKind::Other
        }
    }};
}

/// Classify an error from any wrapped network port into a retry [`ErrorKind`].
#[must_use]
pub fn classify(err: &anyhow::Error) -> ErrorKind {
    if let Some(http) = err.downcast_ref::<HttpStatusError>() {
        return classify_status(http.status);
    }
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        return classify_transport!(re);
    }
    if let Some(oe) = err.downcast_ref::<OpenAIError>() {
        return classify_openai(oe);
    }
    ErrorKind::Other
}

/// Classify an `async-openai` error (transcribe/translate paths). Its `Reqwest`
/// arm carries `async-openai`'s own `reqwest` major, classified via the shared
/// transport macro.
fn classify_openai(err: &OpenAIError) -> ErrorKind {
    match err {
        OpenAIError::Reqwest(e) => classify_transport!(e),
        OpenAIError::ApiError(resp) => classify_status(resp.status_code.as_u16()),
        _ => ErrorKind::Other,
    }
}

/// Classify an HTTP status: 429 and any 5xx are retryable; the rest are not.
fn classify_status(status: u16) -> ErrorKind {
    if status == TOO_MANY {
        ErrorKind::TooMany429
    } else if (SERVER_ERR_LO..SERVER_ERR_HI).contains(&status) {
        ErrorKind::Server5xx
    } else {
        ErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn classifies_http_status_error_5xx_and_429() {
        let e500 = anyhow!(HttpStatusError::new(503, "busy".into()));
        let e429 = anyhow!(HttpStatusError::new(429, "slow down".into()));
        let e404 = anyhow!(HttpStatusError::new(404, "nope".into()));
        assert_eq!(classify(&e500), ErrorKind::Server5xx);
        assert_eq!(classify(&e429), ErrorKind::TooMany429);
        assert_eq!(classify(&e404), ErrorKind::Other);
    }

    #[test]
    fn unrecognised_error_is_other() {
        assert_eq!(classify(&anyhow!("some plain message")), ErrorKind::Other);
    }

    #[test]
    fn status_range_boundaries() {
        assert_eq!(classify_status(500), ErrorKind::Server5xx);
        assert_eq!(classify_status(599), ErrorKind::Server5xx);
        assert_eq!(classify_status(600), ErrorKind::Other);
        assert_eq!(classify_status(499), ErrorKind::Other);
        assert_eq!(classify_status(200), ErrorKind::Other);
    }
}
