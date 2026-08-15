//! The Playwright vocabulary the steps were written in, rebuilt on `WebDriver`.
//!
//! Almost every step in the JavaScript suite was some composition of
//! `page.getByTestId(...)` with `fill`, `click` or an auto-retrying
//! `expect(...)`. `WebDriver` has no retry layer, so each of those becomes an
//! `ElementQuery` with an explicit wait here rather than at 900 call sites.
//!
//! Two conventions carried over from the sibling port:
//!
//! * **Presence and visibility are different questions.** Playwright's
//!   `toBeVisible()` waits for a *displayed* element and `toHaveCount(0)` waits
//!   for the absence of *any*, displayed or not. [`Dom::expect_visible`] and
//!   [`Dom::expect_absent`] keep that distinction; conflating them turns a
//!   `display: none` regression into a pass.
//! * **Asking about an absence must not wait.** A query that expects nothing is
//!   answered by the page as it stands, so the "not there" helpers use
//!   `nowait` and the caller wraps them in [`crate::wait::eventually`] when the
//!   absence is something the page has to *become*.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use thirtyfour::components::SelectElement;
use thirtyfour::prelude::*;

use crate::browser::{WAIT_INTERVAL, WAIT_TIMEOUT};

/// Playwright's page vocabulary, as far as this suite used it.
#[allow(async_fn_in_trait)]
pub trait Dom {
    /// The element carrying `data-testid`, once it is displayed.
    async fn test_id(&self, id: &str) -> Result<WebElement>;

    /// The element carrying `data-testid`, present or not, without waiting.
    async fn test_id_opt(&self, id: &str) -> Result<Option<WebElement>>;

    /// Every element carrying `data-testid`, without waiting.
    async fn test_ids(&self, id: &str) -> Result<Vec<WebElement>>;

    /// The element matching a CSS selector, once it is displayed.
    async fn css(&self, selector: &str) -> Result<WebElement>;

    /// The element matching a CSS selector, present or not, without waiting.
    async fn css_opt(&self, selector: &str) -> Result<Option<WebElement>>;

    /// Every element matching a CSS selector, without waiting.
    async fn css_all(&self, selector: &str) -> Result<Vec<WebElement>>;

    /// Replaces a field's contents, Playwright's `fill`.
    async fn fill(&self, id: &str, value: &str) -> Result<()>;

    /// Clicks the element once it is clickable.
    async fn click(&self, id: &str) -> Result<()>;

    /// Waits for the element to be displayed.
    async fn expect_visible(&self, id: &str) -> Result<()>;

    /// Waits for the element to stop being displayed — it may remain in the
    /// DOM, which is what `toBeHidden()` allowed.
    async fn expect_hidden(&self, id: &str) -> Result<()>;

    /// Waits for no element with that id to exist at all, `toHaveCount(0)`.
    async fn expect_absent(&self, id: &str) -> Result<()>;

    /// Waits for the element's rendered text to contain `needle`.
    async fn expect_text(&self, id: &str, needle: &str) -> Result<()>;

    /// The element's rendered text, once it is displayed.
    async fn text_of(&self, id: &str) -> Result<String>;

    /// Is the element present and displayed, as the page stands right now?
    async fn is_visible(&self, id: &str) -> Result<bool>;

    /// The heading with exactly this text, if the page has one.
    ///
    /// Stands in for `getByRole("heading", { name, exact: true })`.
    async fn heading_opt(&self, name: &str) -> Result<Option<WebElement>>;

    /// Replaces the contents of the field a CSS selector picks out.
    ///
    /// The scoped-form variant of [`Dom::fill`]: `/user-settings` renders two
    /// forms carrying the same test ids, so those steps addressed the field
    /// through its owning `form[action=…]`.
    async fn fill_css(&self, selector: &str, value: &str) -> Result<()>;

    /// Clicks the element a CSS selector picks out, once it is clickable.
    async fn click_css(&self, selector: &str) -> Result<()>;

    /// Clicks a control that navigates, and waits for the navigation to land.
    ///
    /// `WebDriver`'s Element Click returns as soon as the click is dispatched,
    /// so a form POST answered with a redirect is still in flight when the next
    /// line runs — which is what Playwright's
    /// `waitForLoadState("domcontentloaded")` covered. Waiting for the *current
    /// document* to go stale is what detects it, and unlike watching the URL it
    /// still works for a POST that redirects back to the page it came from.
    async fn submit_css(&self, selector: &str) -> Result<()>;

    /// [`Dom::submit_css`] addressed by `data-testid`.
    async fn submit(&self, id: &str) -> Result<()>;

    /// Chooses an `<option>` by value, Playwright's `selectOption`.
    async fn select_option(&self, id: &str, value: &str) -> Result<()>;

    /// A form control's current value, Playwright's `inputValue`.
    async fn value_of(&self, id: &str) -> Result<String>;

    /// Waits for an attribute on the element a CSS selector picks out to equal
    /// `expected`, or to be absent when `expected` is `None`.
    async fn expect_attr(&self, selector: &str, attr: &str, expected: Option<&str>) -> Result<()>;

    /// The table row containing `text`, Playwright's
    /// `locator("tr", { hasText })`.
    async fn row_with_text(&self, text: &str) -> Result<WebElement>;

    /// Waits for some element rendering exactly `text` to be displayed,
    /// Playwright's `getByText`.
    ///
    /// Matches the innermost element carrying the text, as `getByText` does —
    /// otherwise every ancestor up to `<body>` would match too.
    async fn expect_text_somewhere(&self, text: &str) -> Result<()>;

    /// Evaluates a script and hands back the JSON it returned.
    async fn eval(&self, script: &str) -> Result<serde_json::Value>;
}

/// Finding elements inside another element, for the steps that scoped a query
/// to a row or a panel.
#[allow(async_fn_in_trait)]
pub trait Within {
    /// The descendant link with this accessible name, if there is one.
    ///
    /// Stands in for `getByRole("link", { name })`.
    async fn link_named(&self, name: &str) -> Result<Option<WebElement>>;

    /// The descendant carrying `data-testid`, without waiting.
    async fn test_id_opt(&self, id: &str) -> Result<Option<WebElement>>;

    /// The descendant carrying `data-testid`, which must be there.
    async fn test_id(&self, id: &str) -> Result<WebElement>;
}

impl Within for WebElement {
    async fn link_named(&self, name: &str) -> Result<Option<WebElement>> {
        let xpath = format!(".//a[normalize-space(.)={}]", xpath_literal(name));
        Ok(self.query(By::XPath(xpath)).nowait().first_opt().await?)
    }

    async fn test_id_opt(&self, id: &str) -> Result<Option<WebElement>> {
        Ok(self
            .query(By::Testid(id.to_owned()))
            .nowait()
            .first_opt()
            .await?)
    }

    async fn test_id(&self, id: &str) -> Result<WebElement> {
        self.query(By::Testid(id.to_owned()))
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .first()
            .await
            .with_context(|| format!("no descendant with testid `{id}`"))
    }
}

impl Dom for WebDriver {
    async fn test_id(&self, id: &str) -> Result<WebElement> {
        displayed(self, By::Testid(id.to_owned()), &format!("testid `{id}`")).await
    }

    async fn test_id_opt(&self, id: &str) -> Result<Option<WebElement>> {
        Ok(self
            .query(By::Testid(id.to_owned()))
            .nowait()
            .first_opt()
            .await?)
    }

    async fn test_ids(&self, id: &str) -> Result<Vec<WebElement>> {
        all(self, By::Testid(id.to_owned())).await
    }

    async fn css(&self, selector: &str) -> Result<WebElement> {
        displayed(
            self,
            By::Css(selector.to_owned()),
            &format!("selector `{selector}`"),
        )
        .await
    }

    async fn css_opt(&self, selector: &str) -> Result<Option<WebElement>> {
        Ok(self
            .query(By::Css(selector.to_owned()))
            .nowait()
            .first_opt()
            .await?)
    }

    async fn css_all(&self, selector: &str) -> Result<Vec<WebElement>> {
        all(self, By::Css(selector.to_owned())).await
    }

    async fn fill(&self, id: &str, value: &str) -> Result<()> {
        let field = self.test_id(id).await?;
        // `clear` then `send_keys`, because WebDriver's Element Send Keys
        // appends. Playwright's `fill` sets the value outright.
        field.clear().await?;
        field.send_keys(value).await?;
        Ok(())
    }

    async fn click(&self, id: &str) -> Result<()> {
        self.query(By::Testid(id.to_owned()))
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .and_clickable()
            .first()
            .await
            .with_context(|| format!("no clickable element with testid `{id}`"))?
            .click()
            .await?;
        Ok(())
    }

    async fn expect_visible(&self, id: &str) -> Result<()> {
        self.test_id(id).await.map(|_| ())
    }

    async fn expect_hidden(&self, id: &str) -> Result<()> {
        crate::wait::eventually(&format!("testid `{id}` is hidden"), || async {
            match self.test_id_opt(id).await? {
                None => Ok(true),
                Some(element) => Ok(!element.is_displayed().await.unwrap_or(false)),
            }
        })
        .await
    }

    async fn expect_absent(&self, id: &str) -> Result<()> {
        crate::wait::eventually(&format!("testid `{id}` is gone"), || async {
            Ok(self.test_ids(id).await?.is_empty())
        })
        .await
    }

    async fn expect_text(&self, id: &str, needle: &str) -> Result<()> {
        // Written as a loop rather than through `wait::eventually` so the
        // failure can name the text that *was* there — the difference between
        // "the banner never said Password set" and a bare timeout.
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut last = None;
        loop {
            if let Some(element) = self.test_id_opt(id).await? {
                let text = element.text().await.unwrap_or_default();
                if text.contains(needle) {
                    return Ok(());
                }
                last = Some(text);
            }
            if Instant::now() >= deadline {
                let seen = last.map_or_else(
                    || "no such element".to_owned(),
                    |text| format!("{text:?}"),
                );
                bail!(
                    "testid `{id}`: expected text containing {needle:?}, \
                     last saw {seen} after {WAIT_TIMEOUT:?}"
                );
            }
            tokio::time::sleep(WAIT_INTERVAL).await;
        }
    }

    async fn text_of(&self, id: &str) -> Result<String> {
        Ok(self.test_id(id).await?.text().await?)
    }

    async fn is_visible(&self, id: &str) -> Result<bool> {
        match self.test_id_opt(id).await? {
            None => Ok(false),
            Some(element) => Ok(element.is_displayed().await.unwrap_or(false)),
        }
    }

    async fn heading_opt(&self, name: &str) -> Result<Option<WebElement>> {
        // `normalize-space` matches Playwright's `exact: true`, which compares
        // whitespace-normalised text rather than the raw node value.
        let xpath = format!(
            "//*[self::h1 or self::h2 or self::h3 or self::h4 or self::h5 or self::h6]\
             [normalize-space(.)={}]",
            xpath_literal(name)
        );
        Ok(self.query(By::XPath(xpath)).nowait().first_opt().await?)
    }

    async fn fill_css(&self, selector: &str, value: &str) -> Result<()> {
        let field = self.css(selector).await?;
        field.clear().await?;
        field.send_keys(value).await?;
        Ok(())
    }

    async fn click_css(&self, selector: &str) -> Result<()> {
        self.query(By::Css(selector.to_owned()))
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .and_clickable()
            .first()
            .await
            .with_context(|| format!("nothing clickable matches `{selector}`"))?
            .click()
            .await?;
        Ok(())
    }

    async fn submit_css(&self, selector: &str) -> Result<()> {
        let document = self.find(By::Tag("html")).await?;
        self.click_css(selector).await?;
        document
            .wait_until()
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .stale()
            .await
            .with_context(|| format!("`{selector}` did not navigate anywhere"))?;
        Ok(())
    }

    async fn submit(&self, id: &str) -> Result<()> {
        let document = self.find(By::Tag("html")).await?;
        self.click(id).await?;
        document
            .wait_until()
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .stale()
            .await
            .with_context(|| format!("`{id}` did not navigate anywhere"))?;
        Ok(())
    }

    async fn select_option(&self, id: &str, value: &str) -> Result<()> {
        let select = SelectElement::new(&self.test_id(id).await?).await?;
        select
            .select_by_value(value)
            .await
            .with_context(|| format!("`{id}` has no option with value `{value}`"))?;
        Ok(())
    }

    async fn value_of(&self, id: &str) -> Result<String> {
        // `prop`, not `attr`: the attribute is the *initial* value, and these
        // assertions run after the server has re-rendered the form.
        Ok(self
            .test_id(id)
            .await?
            .prop("value")
            .await?
            .unwrap_or_default())
    }

    async fn expect_attr(&self, selector: &str, attr: &str, expected: Option<&str>) -> Result<()> {
        let what = match expected {
            Some(value) => format!("`{selector}` has {attr}={value:?}"),
            None => format!("`{selector}` has no {attr}"),
        };
        crate::wait::eventually(&what, || async {
            let Some(element) = self.css_opt(selector).await? else {
                return Ok(false);
            };
            Ok(element.attr(attr).await?.as_deref() == expected)
        })
        .await
    }

    async fn row_with_text(&self, text: &str) -> Result<WebElement> {
        let xpath = format!("//tr[contains(., {})]", xpath_literal(text));
        self.query(By::XPath(xpath))
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .first()
            .await
            .with_context(|| format!("no table row contains {text:?}"))
    }

    async fn expect_text_somewhere(&self, text: &str) -> Result<()> {
        // `not(.//*[…])` keeps this to the innermost match, which is what
        // `getByText` resolves to; without it every ancestor up to `<body>`
        // matches and the first hit is a container that may well be off-screen.
        let literal = xpath_literal(text);
        let xpath =
            format!("//*[contains(normalize-space(.), {literal})][not(.//*[contains(normalize-space(.), {literal})])]");
        displayed(self, By::XPath(xpath), &format!("text {text:?}"))
            .await
            .map(|_| ())
    }

    async fn eval(&self, script: &str) -> Result<serde_json::Value> {
        Ok(self.execute(script, vec![]).await?.json().clone())
    }
}

/// Waits for a displayed element, naming what was being looked for on failure.
async fn displayed(driver: &WebDriver, by: By, what: &str) -> Result<WebElement> {
    driver
        .query(by)
        .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
        .and_displayed()
        .first()
        .await
        .with_context(|| format!("no displayed element for {what}"))
}

/// Every match as the page stands, with "none" as an empty `Vec` rather than an
/// error.
async fn all(driver: &WebDriver, by: By) -> Result<Vec<WebElement>> {
    Ok(driver.query(by).nowait().all_from_selector().await?)
}

/// Quotes a string for XPath, which has no escape syntax of its own.
///
/// A value containing both quote characters has to be assembled with
/// `concat()`; anything simpler picks whichever quote it does not contain.
fn xpath_literal(value: &str) -> String {
    if !value.contains('\'') {
        return format!("'{value}'");
    }
    if !value.contains('"') {
        return format!("\"{value}\"");
    }
    let parts: Vec<String> = value.split('\'').map(|part| format!("'{part}'")).collect();
    format!("concat({})", parts.join(", \"'\", "))
}
