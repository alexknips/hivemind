//! A stalled request body is cut off with 408 before any handler has run (hivemind-g3vd0).
//!
//! The `Json` and `Bytes` extractors read the whole body before the handler, and so before auth,
//! while holding one of the server's concurrent-request slots. A client that sends its headers
//! and then goes quiet kept that slot until it disconnected; enough of them leave no slot for
//! anyone else. [`request_body_timeout_middleware`] gives the body read an idle deadline: a body
//! that delivers nothing for the timeout is abandoned, the slot is released, and the answer is 408.
//! Because the extractor fails, the handler never starts and nothing is recorded.
//!
//! The deadline is idle time between frames, not total time. A large body from a slow link, such
//! as a ledger replay batch, keeps going as long as it keeps arriving.
//!
//! Not covered: the header phase. A client that stalls before its headers are complete has not
//! reached the router, so it holds a connection but no request slot; `axum::serve` sets no hyper
//! timer, so hyper's own header-read timeout does not run, and bounding it needs a hand-written
//! accept loop in place of `axum::serve`.

use std::future::Future as _;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::BoxError;
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::time::Sleep;

/// Hands the handler a request body that gives up after `idle` without a frame, and answers 408
/// when it did.
pub(super) async fn request_body_timeout_middleware(
    State(idle): State<Duration>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let timed_out = Arc::new(AtomicBool::new(false));
    let body = Body::new(IdleTimeoutBody {
        inner: body,
        idle,
        sleep: None,
        timed_out: Arc::clone(&timed_out),
    });
    let response = next.run(Request::from_parts(parts, body)).await;
    // The extractor that was reading the body rejected it (as a 400, which says nothing about
    // why). Whatever it answered, the cause was the stalled body.
    if timed_out.load(Ordering::Acquire) {
        return (StatusCode::REQUEST_TIMEOUT, "request body timed out").into_response();
    }
    response
}

#[derive(Debug)]
struct BodyIdleTimeout;

impl std::fmt::Display for BodyIdleTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the request body stalled")
    }
}

impl std::error::Error for BodyIdleTimeout {}

/// A request body that errors once `idle` passes without a frame arriving. The timer starts when
/// the body is first found empty-handed and restarts with every frame, so a body nobody is
/// reading yet (a request queued behind the concurrency limit) is not timing out.
struct IdleTimeoutBody {
    inner: Body,
    idle: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
    timed_out: Arc<AtomicBool>,
}

impl HttpBody for IdleTimeoutBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        if let Poll::Ready(frame) = Pin::new(&mut this.inner).poll_frame(cx) {
            this.sleep = None;
            return Poll::Ready(frame.map(|frame| frame.map_err(Into::into)));
        }
        let idle = this.idle;
        let sleep = this
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
        if sleep.as_mut().poll(cx).is_ready() {
            this.timed_out.store(true, Ordering::Release);
            return Poll::Ready(Some(Err(Box::new(BodyIdleTimeout))));
        }
        Poll::Pending
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
