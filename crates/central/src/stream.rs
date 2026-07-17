//! SSE transport for a run: it renders the engine's [`ExecEvent`] stream as
//! Server-Sent Events (`line` / `error` / `done`) the browser's `EventSource`
//! consumes. It owns no execution policy — the process engine and the global
//! concurrency cap live in `shared::exec`; this file is purely the wire shape.

use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures_util::StreamExt;
use serde::Serialize;
use shared::exec::{ExecEvent, ExecHandle, ExecStatus};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::RelayEvent;

/// Stream a live run: each output line is a `line` event, a mid-run failure an
/// `error` event, and the terminal marker a `done` event. When the client closes
/// the `EventSource` this stream is dropped, which the engine observes and uses
/// to kill the process group (AC14/AC18) — no separate cancel call is needed.
pub fn sse_run<T: Send + 'static>(handle: ExecHandle, admission: T) -> impl IntoResponse {
    let stream = ReceiverStream::new(handle.events).map(move |event| {
        let _ = &admission;
        Ok::<_, Infallible>(to_event(event))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Deliver a refusal in-band: one clear `error` line then a terminal `done`, so
/// an `EventSource` client — which cannot read the body of a non-200 response —
/// still shows the reason (node busy, invalid target, rate limited).
pub fn sse_refusal(message: String) -> impl IntoResponse {
    let events = vec![
        // Named "run-error" (not "error") so a browser's EventSource dispatches it
        // to a dedicated listener rather than colliding with its native `error`
        // event, which fires on connection problems.
        Ok::<_, Infallible>(Event::default().event("run-error").data(message)),
        Ok(done_event(ExecStatus::Failed, 0)),
    ];
    Sse::new(tokio_stream::iter(events))
}

/// Stream a relayed remote run using the same SSE shape as a local run.
pub fn sse_relay(events: mpsc::Receiver<RelayEvent>) -> impl IntoResponse {
    let stream = ReceiverStream::new(events).flat_map(|event| {
        let events = match event {
            RelayEvent::Line(line) => vec![Ok::<_, Infallible>(
                Event::default().event("line").data(line),
            )],
            RelayEvent::Terminal {
                error: Some(message),
            } => vec![
                Ok(Event::default().event("run-error").data(message)),
                Ok(done_event(ExecStatus::Failed, 0)),
            ],
            RelayEvent::Terminal { error: None } => {
                vec![Ok(done_event(ExecStatus::Completed { success: true }, 0))]
            }
        };
        tokio_stream::iter(events)
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_event(event: ExecEvent) -> Event {
    match event {
        ExecEvent::Line(line) => Event::default().event("line").data(line),
        ExecEvent::Failed(message) => Event::default().event("run-error").data(message),
        ExecEvent::Done { status, elapsed_ms } => done_event(status, elapsed_ms),
    }
}

#[derive(Serialize)]
struct DonePayload {
    status: &'static str,
    success: bool,
    elapsed_ms: u128,
}

fn done_event(status: ExecStatus, elapsed_ms: u128) -> Event {
    let (label, success) = match status {
        ExecStatus::Completed { success } => ("completed", success),
        ExecStatus::TimedOut => ("timeout", false),
        ExecStatus::OutputCapped => ("truncated", false),
        ExecStatus::Canceled => ("canceled", false),
        ExecStatus::Failed => ("failed", false),
    };
    let payload = DonePayload {
        status: label,
        success,
        elapsed_ms,
    };
    Event::default()
        .event("done")
        .json_data(payload)
        .unwrap_or_else(|_| Event::default().event("done").data(label))
}
