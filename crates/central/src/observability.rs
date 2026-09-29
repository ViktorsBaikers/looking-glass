use axum::http::HeaderMap;

use crate::auth::random_id;

const CORRELATION_HEADER: &str = "x-request-id";

/// The client's `X-Request-Id`, or a fresh one. Call sites log it with `%`, so
/// a client value outside a plain token alphabet is returned quoted: spaces or
/// `=` in it must not forge extra `key=value` fields in the log line.
pub(crate) fn correlation_id(headers: &HeaderMap) -> String {
    headers
        .get(CORRELATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            {
                value.to_string()
            } else {
                format!("{value:?}")
            }
        })
        .unwrap_or_else(random_id)
}

pub(crate) fn new_correlation_id() -> String {
    random_id()
}

pub(crate) fn log_validation_rejected(correlation_id: &str, surface: &str, reason: &str) {
    tracing::warn!(
        event = "validation.rejected",
        correlation_id,
        surface,
        reason,
        "validation rejected"
    );
}
