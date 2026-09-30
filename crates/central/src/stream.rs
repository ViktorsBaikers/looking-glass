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

/// Stream a relayed remote run using the same SSE shape as a local run. Its
/// `done` reports the time from relaying the run to its terminal.
pub fn sse_relay(events: mpsc::Receiver<RelayEvent>) -> impl IntoResponse {
    let started = tokio::time::Instant::now();
    let stream = ReceiverStream::new(events).flat_map(move |event| {
        let events = match event {
            RelayEvent::Line(line) => vec![Ok::<_, Infallible>(line_event(line))],
            RelayEvent::Terminal { error, status } => error
                .map(|message| Ok(Event::default().event("run-error").data(message)))
                .into_iter()
                .chain([Ok(done_event(status, started.elapsed().as_millis()))])
                .collect(),
        };
        tokio_stream::iter(events)
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_event(event: ExecEvent) -> Event {
    match event {
        ExecEvent::Line(line) => line_event(line),
        ExecEvent::Failed(message) => Event::default().event("run-error").data(message),
        ExecEvent::Done { status, elapsed_ms } => done_event(status, elapsed_ms),
    }
}

/// A `line` event whose data is the line as a JSON string, so an empty line
/// still carries a `data` field: EventSource never dispatches an event without
/// one, which silently dropped blank output lines (F-190).
fn line_event(line: String) -> Event {
    Event::default()
        .event("line")
        .json_data(line)
        .expect("a string always serializes")
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

#[cfg(test)]
mod tests {
    use super::*;

    // A relayed run's `done` reports how long the run took, as a local one does.
    #[tokio::test(start_paused = true)]
    async fn a_relayed_done_reports_the_elapsed_run_time() {
        let (tx, rx) = mpsc::channel(1);
        let response = sse_relay(rx).into_response();
        tokio::time::advance(std::time::Duration::from_millis(1500)).await;
        tx.send(RelayEvent::Terminal {
            error: None,
            status: ExecStatus::TimedOut,
        })
        .await
        .unwrap();
        drop(tx);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(r#""elapsed_ms":1500"#), "{body}");
    }

    async fn body_of(response: impl IntoResponse) -> String {
        let body = axum::body::to_bytes(response.into_response().into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    /// The `data` of every `line` event a browser's EventSource dispatches. Per the
    /// SSE spec an event with no `data` field is never dispatched.
    fn dispatched_lines(body: &str) -> Vec<String> {
        body.split("\n\n")
            .filter(|block| block.lines().any(|field| field == "event: line"))
            .filter_map(|block| {
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|field| field.strip_prefix("data:"))
                    .map(|value| value.strip_prefix(' ').unwrap_or(value))
                    .collect();
                (!data.is_empty()).then(|| data.join("\n"))
            })
            .collect()
    }

    fn decoded(data: &[String]) -> Vec<String> {
        data.iter()
            .map(|value| serde_json::from_str(value).expect("a line event carries a JSON string"))
            .collect()
    }

    const OUTPUT: [&str; 4] = ["PING 8.8.8.8", "64 bytes", "", "--- stats ---"];

    // F-190: a blank output line (ping's separator before its statistics) reaches
    // the browser as its own event, in order, on a local run.
    #[tokio::test]
    async fn a_local_run_delivers_blank_lines() {
        let engine = shared::exec::ExecEngine::new(shared::exec::ExecLimits::default());
        let handle = engine
            .try_start(
                shared::template::CommandTemplate {
                    program: "printf",
                    args: vec!["PING 8.8.8.8\\n64 bytes\\n\\n--- stats ---\\n".into()],
                },
                None,
            )
            .expect("start printf");
        let body = body_of(sse_run(handle, ())).await;
        let data = dispatched_lines(&body);
        assert_eq!(
            data.len(),
            OUTPUT.len(),
            "every line, blank ones too, is dispatched: {body:?}"
        );
        assert_eq!(decoded(&data), OUTPUT, "{body:?}");
    }

    // F-190: the same for a relayed run.
    #[tokio::test]
    async fn a_relayed_run_delivers_blank_lines() {
        let (tx, rx) = mpsc::channel(8);
        for line in OUTPUT {
            tx.send(RelayEvent::Line(line.into())).await.unwrap();
        }
        tx.send(RelayEvent::Terminal {
            error: None,
            status: ExecStatus::Completed { success: true },
        })
        .await
        .unwrap();
        drop(tx);
        let body = body_of(sse_relay(rx)).await;
        let data = dispatched_lines(&body);
        assert_eq!(
            data.len(),
            OUTPUT.len(),
            "every line, blank ones too, is dispatched: {body:?}"
        );
        assert_eq!(decoded(&data), OUTPUT, "{body:?}");
        let last = body.trim_end().rsplit("\n\n").next().unwrap();
        assert!(
            last.starts_with("event: done\ndata: {\"status\":\"completed\",\"success\":true,"),
            "{body:?}"
        );
    }
}
