//! Retrying assertions. `WebDriver` has no auto-waiting layer, so a read that
//! runs before `app.js` finishes a swap reports the old state. `ElementQuery`
//! covers "wait for an element"; these cover computed values that must settle.

use std::fmt::Debug;
use std::future::Future;
use std::time::Instant;

use anyhow::{Result, bail};
use thirtyfour::error::{WebDriverError, WebDriverErrorInner};

use crate::browser::{WAIT_INTERVAL, WAIT_TIMEOUT};

/// A stale reference means the app swapped the region mid-poll: "not yet", not
/// a fault. Every other error still fails immediately.
fn is_stale(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<WebDriverError>().is_some_and(|error| {
            matches!(
                error.as_inner(),
                WebDriverErrorInner::StaleElementReference(_)
            )
        })
    })
}

/// Polls `probe` until it reports the expected value; a timeout names the last
/// value seen.
///
/// # Errors
///
/// Fails when `probe` errors, or when the value has still not matched by
/// [`WAIT_TIMEOUT`].
pub async fn eventually_eq<T, E, F, Fut>(what: &str, expected: E, mut probe: F) -> Result<()>
where
    T: Debug,
    E: Debug + PartialEq<T>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut last = None;
    loop {
        match probe().await {
            Ok(value) => {
                if expected == value {
                    return Ok(());
                }
                last = Some(value);
            }
            Err(error) if is_stale(&error) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            bail!("{what}: expected {expected:?}, last saw {last:?} after {WAIT_TIMEOUT:?}");
        }
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

/// Polls `probe` until it reports `true`.
///
/// # Errors
///
/// Fails when `probe` errors, or when it has still not held by
/// [`WAIT_TIMEOUT`].
pub async fn eventually<F, Fut>(what: &str, probe: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    eventually_within(WAIT_TIMEOUT, what, probe).await
}

/// [`eventually`] with its own deadline, for background work (e.g.
/// summarization) that can legitimately outlast [`WAIT_TIMEOUT`].
///
/// # Errors
///
/// Fails when `probe` errors, or when it has still not held by `timeout`.
pub async fn eventually_within<F, Fut>(
    timeout: std::time::Duration,
    what: &str,
    mut probe: F,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + timeout;
    loop {
        match probe().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) if is_stale(&error) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            bail!("{what}: still not true after {timeout:?}");
        }
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

/// Polls `probe` until it reports the same value `samples` times running, for
/// state with no known target (e.g. pane actions that may be "not ready" or
/// "deliberately inert").
///
/// # Errors
///
/// Fails when `probe` errors, or when the value never holds still by
/// [`WAIT_TIMEOUT`].
pub async fn settles<T, F, Fut>(what: &str, samples: usize, mut probe: F) -> Result<T>
where
    T: Debug + PartialEq,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut last: Option<T> = None;
    let mut runs = 0;
    loop {
        match probe().await {
            Ok(value) => {
                if last.as_ref() == Some(&value) {
                    runs += 1;
                    if runs >= samples {
                        return Ok(value);
                    }
                } else {
                    runs = 1;
                }
                last = Some(value);
            }
            Err(error) if is_stale(&error) => runs = 0,
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            bail!("{what}: never held still, last saw {last:?} after {WAIT_TIMEOUT:?}");
        }
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

/// Polls `probe` until it reports a value, handing it back.
///
/// # Errors
///
/// Fails when `probe` errors, or when it has still reported `None` by
/// [`WAIT_TIMEOUT`].
pub async fn eventually_some<T, F, Fut>(what: &str, mut probe: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>>>,
{
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        match probe().await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(error) if is_stale(&error) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            bail!("{what}: never appeared within {WAIT_TIMEOUT:?}");
        }
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

/// Retries `action` on stale references: sidebar refetches (swap-complete, SSE)
/// can detach a handle between find and click.
///
/// `action` must find its element *inside* the closure, or it replays the same
/// dead reference.
///
/// # Errors
///
/// Fails at once on any non-stale error, or after [`WAIT_TIMEOUT`] of staleness.
pub async fn despite_swaps<T, F, Fut>(what: &str, mut action: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        match action().await {
            Ok(value) => return Ok(value),
            Err(error) if is_stale(&error) => {
                if Instant::now() >= deadline {
                    return Err(error.context(format!(
                        "{what}: the page swapped it away every time for {WAIT_TIMEOUT:?}"
                    )));
                }
            }
            Err(error) => return Err(error),
        }
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use thirtyfour::error::WebDriverErrorInfo;

    use super::*;

    /// The error a driver reports for a handle whose node has been replaced.
    fn stale_reference() -> anyhow::Error {
        WebDriverError::from(WebDriverErrorInner::StaleElementReference(
            WebDriverErrorInfo::new("stale element reference".to_owned()),
        ))
        .into()
    }

    #[tokio::test]
    async fn despite_swaps_retries_until_the_page_holds_still() {
        let attempts = Cell::new(0);

        let result = despite_swaps("clicking through a swap", || async {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(stale_reference())
            } else {
                Ok(attempts.get())
            }
        })
        .await;

        assert_eq!(
            result.expect("a swap mid-action is the page being busy, not a fault"),
            3
        );
    }

    #[tokio::test]
    async fn despite_swaps_reports_anything_else_at_once() {
        // A missing element must fail fast, not wait out the timeout.
        let attempts = Cell::new(0);

        let error = despite_swaps("clicking a button that is not there", || async {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(anyhow::anyhow!("the page has no `Cancel` button"))
        })
        .await
        .expect_err("a missing button is not something to wait for");

        assert_eq!(attempts.get(), 1, "a real fault must not be retried");
        assert!(error.to_string().contains("no `Cancel` button"));
    }
}
