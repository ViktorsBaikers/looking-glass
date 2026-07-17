use axum::http::HeaderMap;

use crate::auth::random_id;

const CORRELATION_HEADER: &str = "x-request-id";

pub(crate) fn correlation_id(headers: &HeaderMap) -> String {
    headers
        .get(CORRELATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
