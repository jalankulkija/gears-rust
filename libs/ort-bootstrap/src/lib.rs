//! Bounded, abandonable initialization for the gears that load ONNX Runtime.
//!
//! # The `ort` 2.0.0-rc.12 startup hang
//!
//! The workspace pins `ort` with its `load-dynamic` feature, so ONNX Runtime is
//! `dlopen`ed from `ORT_DYLIB_PATH` at process start rather than linked at build
//! time. When that library is missing, unset, the wrong architecture, or older
//! than the pinned `api-24` ABI, `ort` **hangs forever instead of erroring**.
//!
//! Two measured facts, both worth keeping in view before anyone simplifies this
//! away:
//!
//! - **No pre-flight validation exists.** `ort::init_from(path)` looks like one
//!   (takes a path, returns `Result`) but hangs on the same input. Every `ort`
//!   entry point funnels through the same lazy dylib init.
//! - **The internal mechanism is not settled.** Stack sampling shows the thread
//!   parked in `ort::api()`'s `OnceLock`, apparently re-entered by the very
//!   error path reporting the load failure — but the `init_from` reproduction
//!   above does not fit that explanation. Treat the hang as an observed
//!   property of a bad `ORT_DYLIB_PATH`, not as something understood well
//!   enough to narrow.
//!
//! Since the hang cannot be interrupted from inside, the blocked thread has to
//! be abandoned. That is what this crate encapsulates: a raw [`std::thread`]
//! (**not** `tokio::task::spawn_blocking`, because Tokio joins blocking threads
//! on shutdown, so a wedged one would hang the whole runtime's shutdown), a
//! `oneshot` back to the caller, a timeout, and a race against the gear's
//! cancellation token so shutdown never waits out the full budget.
//!
//! Abandoning a thread is only sound if the process then terminates.
//! [`BootstrapError::leaked_init_thread`] makes that obligation checkable in
//! code rather than documented in a comment.
//!
//! Provision a compatible runtime with `scripts/fetch-onnxruntime.sh`.
//!
//! This crate exists only while `ort` behaves this way. Check for a fix above
//! `2.0.0-rc.12` when bumping the workspace pin; once `ort` either errors
//! properly or offers a way to validate the dylib first, this crate and both
//! of its callers can go.
//!
//! # Example
//!
//! ```rust,ignore
//! let detector = Bootstrap::new("Magika content-type detector", Duration::from_secs(30))
//!     .run(
//!         move || MagikaDetector::with_config(extensions, threads),
//!         ctx.cancellation_token(),
//!     )
//!     .await?;
//! ```

use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Why a bounded initialization failed.
///
/// `E` is whatever the wrapped loader returns when it fails honestly — i.e.
/// when ONNX Runtime loaded far enough to produce a real error.
///
/// Two variants leave an abandoned OS thread behind, which is only sound if the
/// caller then terminates the process; see [`Self::leaked_init_thread`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BootstrapError<E> {
    /// No result within the timeout — almost always the `ort` startup hang
    /// described in the crate docs. The thread is abandoned, not killed.
    #[error(
        "{component} did not finish loading within {}s; the ONNX Runtime is likely missing, \
         the wrong architecture, or incompatible with the version `ort` was built against — \
         check ORT_DYLIB_PATH (the init thread is abandoned, not killed, and will leak until \
         the process exits)",
        timeout.as_secs()
    )]
    Timeout {
        component: &'static str,
        timeout: Duration,
    },

    /// The cancellation token fired before initialization finished. The thread
    /// may still be running, and may be wedged.
    #[error("{component} initialization cancelled by shutdown")]
    Cancelled { component: &'static str },

    /// The thread vanished without sending a result — it panicked, or the
    /// `oneshot` sender was dropped. The thread is gone, so nothing leaks.
    #[error("{component} init thread died unexpectedly: {source}")]
    InitThreadDied {
        component: &'static str,
        #[source]
        source: tokio::sync::oneshot::error::RecvError,
    },

    /// The loader ran to completion and returned a real error. Nothing leaks.
    #[error("{component} failed to load: {source}")]
    Load {
        component: &'static str,
        #[source]
        source: E,
    },
}

impl<E> BootstrapError<E> {
    /// Whether this failure left an OS thread that will never be joined.
    ///
    /// `true` means the caller must terminate the process for the leak to stay
    /// bounded. Note that exiting while that thread sits inside ONNX Runtime
    /// init can abort rather than exit cleanly (ORT's `cpuinfo` asserts if
    /// teardown races init), so the observable failure may be a signal rather
    /// than an exit code.
    #[must_use]
    pub const fn leaked_init_thread(&self) -> bool {
        match self {
            // `Timeout` is wedged by definition. `Cancelled` may be a thread
            // that is merely still loading, but the two are indistinguishable
            // from here, so assume the worse case.
            Self::Timeout { .. } | Self::Cancelled { .. } => true,
            // Both mean the thread ran to completion and is gone.
            Self::InitThreadDied { .. } | Self::Load { .. } => false,
        }
    }

    /// The component name this failure was reported against.
    #[must_use]
    pub const fn component(&self) -> &'static str {
        match self {
            Self::Timeout { component, .. }
            | Self::Cancelled { component }
            | Self::InitThreadDied { component, .. }
            | Self::Load { component, .. } => component,
        }
    }
}

/// A bounded initialization, ready to run.
#[derive(Debug, Clone, Copy)]
pub struct Bootstrap {
    component: &'static str,
    timeout: Duration,
}

impl Bootstrap {
    /// `component` names the thing being loaded, and appears in every error
    /// message — make it something an operator reading a startup failure would
    /// recognize. `timeout` bounds the wedged case; size it against a *healthy*
    /// load, since the hang never completes and a generous budget only delays a
    /// legible failure.
    #[must_use]
    pub const fn new(component: &'static str, timeout: Duration) -> Self {
        Self { component, timeout }
    }

    /// Runs `load` on an abandonable OS thread, bounded by the timeout and
    /// racing `cancellation_token`.
    ///
    /// `load` must be self-contained: on timeout it keeps running with no way
    /// to stop it, so it must not hold anything whose lifetime the caller
    /// expects to control.
    ///
    /// # Errors
    ///
    /// See [`BootstrapError`] — two variants oblige the caller to terminate the
    /// process.
    pub async fn run<T, E, F>(
        self,
        load: F,
        cancellation_token: &CancellationToken,
    ) -> Result<T, BootstrapError<E>>
    where
        F: FnOnce() -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
    {
        let component = self.component;

        // Checked before spawning, not just in the `select!` below. Abandoning
        // a thread midway through ONNX Runtime init can abort the process on
        // exit (ORT's `cpuinfo` asserts if teardown races init), so if shutdown
        // has already begun, don't start work we know we are about to discard.
        if cancellation_token.is_cancelled() {
            return Err(BootstrapError::Cancelled { component });
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            // The receiver may already be gone if we timed out and moved on; a
            // dropped receiver just means `send` fails harmlessly.
            drop(tx.send(load()));
        });

        // Race the bounded wait against shutdown, so a wedged load doesn't
        // block graceful shutdown for the full timeout.
        tokio::select! {
            result = tokio::time::timeout(self.timeout, rx) => {
                result
                    .map_err(|_elapsed| BootstrapError::Timeout { component, timeout: self.timeout })?
                    .map_err(|source| BootstrapError::InitThreadDied { component, source })?
                    .map_err(|source| BootstrapError::Load { component, source })
            }
            () = cancellation_token.cancelled() => Err(BootstrapError::Cancelled { component }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("simulated load failure")]
    struct LoadFailed;

    fn bootstrap() -> Bootstrap {
        Bootstrap::new("test component", Duration::from_millis(200))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_load_returns_the_value() {
        let out = bootstrap()
            .run(|| Ok::<_, LoadFailed>(42), &CancellationToken::new())
            .await;
        assert_eq!(out.unwrap(), 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn honest_failure_is_load_and_leaks_nothing() {
        let err = bootstrap()
            .run(|| Err::<(), _>(LoadFailed), &CancellationToken::new())
            .await
            .unwrap_err();

        assert!(matches!(err, BootstrapError::Load { .. }));
        assert!(!err.leaked_init_thread(), "the thread ran to completion");
        assert!(err.to_string().contains("test component"));
    }

    /// The reason this crate exists: a loader that never returns must surface
    /// as a bounded, legible error rather than hanging the caller forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wedged_load_times_out_and_reports_the_leak() {
        let err = bootstrap()
            .run(
                || {
                    // Stands in for the `ort` hang: never returns.
                    std::thread::sleep(Duration::from_secs(30));
                    Ok::<_, LoadFailed>(())
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, BootstrapError::Timeout { .. }));
        assert!(
            err.leaked_init_thread(),
            "a timed-out load abandons its thread"
        );
        assert!(
            err.to_string().contains("ORT_DYLIB_PATH"),
            "the message must point at the usual cause: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_beats_the_timeout() {
        let token = CancellationToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            canceller.cancel();
        });

        let started = std::time::Instant::now();
        let err = bootstrap()
            .run(
                || {
                    std::thread::sleep(Duration::from_secs(30));
                    Ok::<_, LoadFailed>(())
                },
                &token,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, BootstrapError::Cancelled { .. }));
        assert!(err.leaked_init_thread());
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "cancellation must not wait out the timeout"
        );
    }

    /// Shutdown that has already begun must not start work it will discard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn already_cancelled_never_spawns() {
        let token = CancellationToken::new();
        token.cancel();

        let err = bootstrap()
            .run(
                || {
                    panic!("loader must not run once shutdown has begun");
                    #[allow(unreachable_code)]
                    Ok::<_, LoadFailed>(())
                },
                &token,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, BootstrapError::Cancelled { .. }));
    }
}
