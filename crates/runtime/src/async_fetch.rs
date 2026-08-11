//! Requests that run while the app keeps drawing.
//!
//! A guest is single threaded and the host's HTTP client is synchronous, so
//! `http-client.fetch` blocks the app's whole event loop until the response is
//! complete. Measured against a server that stalls three seconds, a real app
//! froze for the full three: no frame, no click, no cancel button (K-101).
//!
//! This module is the other option. `begin` hands the request to an OS thread
//! and returns a handle immediately; the guest keeps its loop turning and asks
//! `poll` whether the answer has arrived. Nothing here makes the *request*
//! faster -- it makes the app stay alive while the request is slow, which is
//! the part a person notices.
//!
//! Three properties this is built to hold, because each one is a way the
//! design could have gone quietly wrong:
//!
//! - **The capability check stays on the calling thread, at `begin`.** A
//!   handle is only ever issued for a host the person granted, and the check
//!   happens before any thread is spawned. Deferring it to the worker would
//!   have moved a permission decision off the path the guard sits on.
//! - **The worker gets a plain request and returns a plain response.** It
//!   never touches host state, which is `Rc<RefCell<_>>` and not `Send`. The
//!   only thing crossing the thread boundary is data.
//! - **A dropped handle cannot leak a thread forever.** `cancel` retires the
//!   handle and the worker's result is discarded when it lands; the run ending
//!   drops the whole table.

use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use crate::uapi_dispatch::{AdapterError, HttpResponse};

/// What became of a request. Mirrors the `fetch-status` variant in the WIT.
#[derive(Debug)]
pub enum FetchStatus {
    /// Still working. The handle stays live.
    Pending,
    /// Finished, with the response. The handle is retired.
    Ready(HttpResponse),
    /// Finished badly. The handle is retired.
    Failed(AdapterError),
    /// Never issued, or already answered or cancelled.
    UnknownHandle,
}

/// One request in flight, waiting on its worker.
struct Pending {
    rx: Receiver<Result<HttpResponse, AdapterError>>,
}

/// The in-flight requests for one run.
///
/// Not `Send` and not shared: it lives beside the rest of the host state and
/// dies with the run, so a handle cannot outlive the app that made it.
#[derive(Default)]
pub struct AsyncFetches {
    next_handle: u64,
    live: BTreeMap<u64, Pending>,
}

impl AsyncFetches {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many requests are in flight. Test-only: it exists so the tests can
    /// prove a retired handle does not leak an entry.
    #[cfg(test)]
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// Spawn `work` on its own thread and return the handle to poll.
    ///
    /// `work` is whatever actually performs the request. It is a closure
    /// rather than the adapter itself because the adapter is not `Send`: the
    /// caller captures a plain request and a `Send` way to run it, and this
    /// module never learns what an adapter is.
    ///
    /// **The caller must have run the capability check already.** This is the
    /// one invariant that cannot be enforced from inside here, so it is stated
    /// at every call site and pinned by a test in `uapi_dispatch`.
    pub fn begin<F>(&mut self, work: F) -> u64
    where
        F: FnOnce() -> Result<HttpResponse, AdapterError> + Send + 'static,
    {
        // Handles start at 1 so that 0 is never valid: a guest that forgets to
        // store the handle gets `unknown-handle` rather than someone's result.
        self.next_handle += 1;
        let handle = self.next_handle;

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // A send failure means the handle was cancelled and the receiver
            // is gone. That is expected, not an error: drop the answer.
            let _ = tx.send(work());
        });

        self.live.insert(handle, Pending { rx });
        handle
    }

    /// Ask what happened. Returns immediately, always.
    ///
    /// A terminal answer retires the handle, so a second poll of a finished
    /// request reports `UnknownHandle` rather than blocking forever on a
    /// channel nobody will send to.
    pub fn poll(&mut self, handle: u64) -> FetchStatus {
        let Some(pending) = self.live.get(&handle) else {
            return FetchStatus::UnknownHandle;
        };

        match pending.rx.try_recv() {
            Ok(Ok(response)) => {
                self.live.remove(&handle);
                FetchStatus::Ready(response)
            }
            Ok(Err(err)) => {
                self.live.remove(&handle);
                FetchStatus::Failed(err)
            }
            Err(TryRecvError::Empty) => FetchStatus::Pending,
            // The worker thread died without sending -- a panic in the
            // adapter. Report it as a failure rather than leaving the guest
            // polling a handle that will never answer.
            Err(TryRecvError::Disconnected) => {
                self.live.remove(&handle);
                FetchStatus::Failed(AdapterError::Network(
                    "the request ended without a result".to_string(),
                ))
            }
        }
    }

    /// Retire a handle. Safe on one already retired.
    ///
    /// The worker thread is not killed -- there is no safe way to do that --
    /// but its result is dropped when it lands, and the thread ends on its own
    /// once the request finishes or times out. The request's own timeout is
    /// what bounds that, which is why a caller-supplied timeout matters.
    pub fn cancel(&mut self, handle: u64) {
        self.live.remove(&handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// The property the whole module exists for: `begin` does not wait.
    #[test]
    fn begin_returns_before_the_work_is_done() {
        let mut fetches = AsyncFetches::new();
        let started = std::time::Instant::now();
        let handle = fetches.begin(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            Ok(response(200))
        });
        let elapsed = started.elapsed();

        assert!(handle > 0, "handles start at 1 so 0 is never valid");
        assert!(
            elapsed < std::time::Duration::from_millis(150),
            "begin blocked for {elapsed:?} -- it must return immediately"
        );
    }

    #[test]
    fn a_slow_request_reads_as_pending_then_ready() {
        let mut fetches = AsyncFetches::new();
        let handle = fetches.begin(|| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            Ok(response(201))
        });

        assert!(
            matches!(fetches.poll(handle), FetchStatus::Pending),
            "a request that has not finished must read as pending"
        );

        // Poll until it lands, the way a guest's loop would.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match fetches.poll(handle) {
                FetchStatus::Ready(got) => {
                    assert_eq!(got.status, 201);
                    break;
                }
                FetchStatus::Pending => {
                    assert!(std::time::Instant::now() < deadline, "never became ready");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                other => panic!("unexpected status: {other:?}"),
            }
        }
    }

    /// A terminal answer retires the handle, so the guest cannot poll a
    /// finished request forever and cannot read someone else's result later.
    #[test]
    fn a_handle_is_retired_once_it_answers() {
        let mut fetches = AsyncFetches::new();
        let handle = fetches.begin(|| Ok(response(200)));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while matches!(fetches.poll(handle), FetchStatus::Pending) {
            assert!(std::time::Instant::now() < deadline, "never became ready");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(matches!(fetches.poll(handle), FetchStatus::UnknownHandle));
        assert_eq!(fetches.live_count(), 0, "a retired handle must not leak");
    }

    #[test]
    fn a_failed_request_is_reported_as_failed_not_pending() {
        let mut fetches = AsyncFetches::new();
        let handle = fetches.begin(|| Err(AdapterError::Network("boom".to_string())));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match fetches.poll(handle) {
                FetchStatus::Failed(err) => {
                    assert!(format!("{err:?}").contains("boom"));
                    break;
                }
                FetchStatus::Pending => {
                    assert!(std::time::Instant::now() < deadline, "never failed");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                other => panic!("unexpected status: {other:?}"),
            }
        }
    }

    /// A handle nobody was issued is not a network failure, and saying so
    /// keeps a guest bug from looking like a server problem.
    #[test]
    fn an_unissued_handle_is_unknown_rather_than_failed() {
        let mut fetches = AsyncFetches::new();
        assert!(matches!(fetches.poll(0), FetchStatus::UnknownHandle));
        assert!(matches!(fetches.poll(9999), FetchStatus::UnknownHandle));
    }

    /// What a cancel button does.
    #[test]
    fn cancel_retires_the_handle_and_is_safe_to_repeat() {
        let mut fetches = AsyncFetches::new();
        let handle = fetches.begin(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            Ok(response(200))
        });

        fetches.cancel(handle);
        assert_eq!(fetches.live_count(), 0);
        assert!(matches!(fetches.poll(handle), FetchStatus::UnknownHandle));

        // Cancelling twice must not panic -- a cancel button can be pressed
        // twice, and the second press is not an error.
        fetches.cancel(handle);
        assert!(matches!(fetches.poll(handle), FetchStatus::UnknownHandle));
    }

    /// Several requests at once is the normal case for a feed or a gallery,
    /// and each must keep its own answer.
    #[test]
    fn requests_do_not_get_each_others_answers() {
        let mut fetches = AsyncFetches::new();
        let slow = fetches.begin(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            Ok(response(200))
        });
        let quick = fetches.begin(|| Ok(response(404)));

        assert_ne!(slow, quick, "handles must be distinct");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match fetches.poll(quick) {
                FetchStatus::Ready(got) => {
                    assert_eq!(got.status, 404, "the quick request got the wrong answer");
                    break;
                }
                FetchStatus::Pending => {
                    assert!(std::time::Instant::now() < deadline, "never became ready");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                other => panic!("unexpected status: {other:?}"),
            }
        }

        // The slow one is untouched by the quick one finishing.
        assert!(matches!(fetches.poll(slow), FetchStatus::Pending));
    }
}
