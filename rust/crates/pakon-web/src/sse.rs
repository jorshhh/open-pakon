//! Event streams: `data: <json>\n\n` chunks, as the page's `postSSE` reads
//! them, and the report of how a stream ended (`_reported` in web/app.py).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

use crate::telemetry::Telemetry;

pub type Tx = UnboundedSender<Value>;

/// A stream's sending end and the response that carries it. The stream ends
/// when every sender is dropped. A client that goes away does not stop the
/// work behind the stream: a scan must run to its teardown.
pub fn channel() -> (Tx, Response) {
    let (tx, rx) = unbounded_channel::<Value>();
    let stream = UnboundedReceiverStream::new(rx).map(|ev| Ok::<_, Infallible>(format!("data: {ev}\n\n")));
    let resp = Response::builder()
        .header(CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(stream))
        .expect("a valid response");
    (tx, resp)
}

/// A stream with one error event.
pub fn refuse(message: &str) -> Response {
    let (tx, resp) = channel();
    let _ = tx.send(json!({"type": "error", "message": message}));
    resp
}

struct Report {
    telemetry: Arc<Telemetry>,
    name: &'static str,
    page: &'static str,
    props: Map<String, Value>,
    t0: Instant,
    /// The last log lines: the scanner's own log is the best clue to a
    /// failure on someone else's unit.
    tail: VecDeque<String>,
}

/// Sends a stream's events, reporting how it ended: `<name>_done` (usage)
/// or a `<Name>Failed` error with the last log lines (crash reports).
pub struct Emitter {
    tx: Tx,
    report: Option<Report>,
}

impl Emitter {
    pub fn reported(tx: Tx, telemetry: Arc<Telemetry>, name: &'static str, page: &'static str, props: Value) -> Emitter {
        let props = props.as_object().cloned().unwrap_or_default();
        Emitter { tx, report: Some(Report { telemetry, name, page, props, t0: Instant::now(), tail: VecDeque::new() }) }
    }

    pub fn emit(&mut self, ev: Value) {
        if let Some(r) = &mut self.report {
            let message = ev["message"].as_str().unwrap_or("");
            match ev["type"].as_str() {
                Some("log" | "feed") => {
                    if r.tail.len() == 40 {
                        r.tail.pop_front();
                    }
                    r.tail.push_back(message.to_string());
                    if message.starts_with("Model: ") {
                        if let Some(model) = message.split_whitespace().nth(1) {
                            r.props.insert("model".into(), json!(model));
                        }
                    }
                }
                Some("done") => {
                    let mut props = r.props.clone();
                    props.insert("seconds".into(), json!(r.t0.elapsed().as_secs_f64().round() as u64));
                    if let Some(mb) = ev.get("mb") {
                        props.insert("mb".into(), mb.clone());
                    }
                    r.telemetry.event(&format!("{}_done", r.name), r.page, Value::Object(props));
                }
                Some("error") => {
                    let kind = format!("{}{}Failed", r.name[..1].to_uppercase(), &r.name[1..]);
                    let tail = r.tail.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
                    r.telemetry.error(&kind, message, &tail, r.page);
                }
                _ => {}
            }
        }
        let _ = self.tx.send(ev);
    }

    pub fn log(&mut self, message: impl AsRef<str>) {
        self.emit(json!({"type": "log", "message": message.as_ref()}));
    }

    pub fn error(&mut self, message: impl AsRef<str>) {
        self.emit(json!({"type": "error", "message": message.as_ref()}));
    }
}
