//! Retrying assertions.
//!
//! Playwright's `expect(...)` polls until the assertion holds or a timeout
//! expires, which is what let the old steps write
//! `await expect(page).toHaveURL('/')` straight after a click. `WebDriver` has
//! no such layer: a `find` that runs before `app.js` has finished swapping a
//! class simply reports the old state.
//!
//! thirtyfour's `ElementQuery` filters cover the cases that are really "wait
//! for an element matching X", and the page objects use them. These helpers
//! cover the rest — a computed value that has to settle, like an unread count
//! or the URL after a form post.

use std::fmt::Debug;
use std::future::Future;
use std::time::Instant;

use anyhow::{Result, bail};
use thirtyfour::error::{WebDriverError, WebDriverErrorInner};

use crate::browser::{WAIT_INTERVAL, WAIT_TIMEOUT};

/// Is this the DOM having moved under the probe, rather than a real fault?
///
/// The app swaps regions of the page in place, so an element found on one poll
/// can be detached before the next line reads it. To a poll that is "not yet" —
/// the answer it is there to wait for — and treating it as an error turns every
/// assertion that spans a swap into a flake.
///
/// Only the stale-reference error is forgiven. A missing element, a bad
/// selector or a dead session still fail immediately, which is what keeps a
/// genuinely broken page from being waited on for the full timeout.
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

/// Polls `probe` until it reports the expected value.
///
/// On timeout the failure names the last value seen, not merely that a wait
/// expired — that is the difference between "the counter never reached 2" and a
/// message you have to reproduce by hand to understand.
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

/// [`eventually`] with a deadline of its own.
///
/// For the handful of waits that are not "the page is catching up" but "a
/// background worker is getting to it" — chiefly summarization, which drains
/// one job at a time and yields its database work to interactive requests, so
/// on a slow machine it legitimately outlasts the interaction timeout.
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

/// Polls `probe` until it reports the same value `samples` times running.
///
/// For state that is *settling* rather than heading for a known value — where
/// the test cannot say which answer is correct, only that the page has stopped
/// changing its mind. The reading pane is the case in point: its actions are
/// wired up asynchronously, so "disabled" means "not ready yet" on one entry
/// and "deliberately inert" on another, and only one of those ever changes.
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
/// The shape for "read something once it exists" — Playwright's
/// `expect(locator).toBeVisible()` followed by a read, in one step.
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

/// Runs `action` again while the page keeps moving out from under it.
///
/// The acting counterpart to the polls above. They forgive a stale reference
/// while waiting for the page to *report* something; this forgives one in a
/// step that is trying to *do* something. Finding an element and clicking it
/// are two round trips, and the app swaps whole regions in between — every
/// `rdrs:swap-complete` refetches the sidebar, and an SSE `sidebar` event does
/// it again unprompted — so a handle can be detached before the click reaches
/// it. That is the page being busy, exactly as it is for a read, and it is the
/// one error worth another attempt.
///
/// `action` has to find what it acts on *inside* the closure. Given a handle
/// captured beforehand it just replays the same dead reference until the
/// deadline.
///
/// # Errors
///
/// Fails with whatever `action` failed with: immediately when that is anything
/// but a stale reference, and once the page has refused to hold still for
/// [`WAIT_TIMEOUT`] when it is.
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
        // The counterpart the retry must not swallow: waiting out the full
        // timeout on a button that is simply absent would trade a clear failure
        // for a slow, unexplained one.
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
