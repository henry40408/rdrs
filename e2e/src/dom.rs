//! The Playwright vocabulary the steps were written in, rebuilt on `WebDriver`.
//!
//! Almost every step was some composition of `page.getByTestId(...)` with
//! `fill`, `click` or an auto-retrying `expect(...)`. `WebDriver` has no retry
//! layer, so each becomes an `ElementQuery` with an explicit wait here rather
//! than at 900 call sites.
//!
//! Two conventions carried over from the sibling port:
//!
//! * **Presence and visibility are different questions.** `toBeVisible()` waits
//!   for a *displayed* element, `toHaveCount(0)` for the absence of *any*.
//!   Conflating them turns a `display: none` regression into a pass.
//! * **Asking about an absence must not wait.** A query that expects nothing is
//!   answered by the page as it stands, so those helpers use `nowait` and the
//!   caller wraps them in [`crate::wait::eventually`] when the absence is
//!   something the page has to *become*.

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

    /// Replaces the contents of the field a CSS selector picks out — the
    /// scoped-form variant of [`Dom::fill`], for `/user-settings`, which renders
    /// two forms carrying the same test ids.
    async fn fill_css(&self, selector: &str, value: &str) -> Result<()>;

    /// Clicks the element a CSS selector picks out, once it is clickable.
    async fn click_css(&self, selector: &str) -> Result<()>;

    /// Clicks a control that navigates, and waits for the navigation to land.
    ///
    /// `WebDriver`'s Element Click returns as soon as the click is dispatched, so
    /// a form POST answered with a redirect is still in flight when the next line
    /// runs. Waiting for the *current document* to go stale is what detects it,
    /// and unlike watching the URL it still works for a POST that redirects back
    /// to the page it came from.
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
    /// Playwright's `getByText`. Matches the innermost element carrying the text,
    /// as `getByText` does — otherwise every ancestor up to `<body>` matches.
    async fn expect_text_somewhere(&self, text: &str) -> Result<()>;

    /// Evaluates a script and hands back the JSON it returned.
    async fn eval(&self, script: &str) -> Result<serde_json::Value>;

    /// Finds an element and reads its text in one go, reporting `None` when it is
    /// not there *or* went away mid-read.
    ///
    /// Doing it in two steps races the in-place swaps: the element is found, the
    /// list pane is replaced, and the read fails with a stale-reference error —
    /// which is the poll's answer ("not yet"), not a fault.
    async fn text_of_css(&self, selector: &str) -> Result<Option<String>>;

    /// [`Dom::text_of_css`] addressed by `data-testid`.
    async fn text_of_test_id(&self, id: &str) -> Result<Option<String>>;

    /// The rendered text of every element a CSS selector matches, in document
    /// order. `toHaveText([...])` compared exactly this, and pinned the count
    /// along the way — a row that should have been hidden fails on the comparison
    /// rather than passing unnoticed at the end of the list.
    async fn texts_of(&self, selector: &str) -> Result<Vec<String>>;

    /// Is the checkbox ticked? Playwright's `toBeChecked`.
    async fn is_checked(&self, id: &str) -> Result<bool>;

    /// Ticks a checkbox if it is not already, Playwright's `check`.
    async fn check(&self, id: &str) -> Result<()>;

    /// Does this element have keyboard focus? Playwright's `toBeFocused`.
    async fn is_focused(&self, id: &str) -> Result<bool>;

    /// One computed style property of the first element a selector matches.
    async fn computed_style(&self, selector: &str, property: &str) -> Result<String>;

    /// The first matching element's border box, as `(x, y, width, height)` in
    /// viewport coordinates — Playwright's `boundingBox`. Read through
    /// `getBoundingClientRect` rather than `WebElement::rect`, which reports
    /// document coordinates and disagrees once the page has scrolled.
    async fn bounding_box(&self, selector: &str) -> Result<(f64, f64, f64, f64)>;

    /// Clicks the page body, then presses a key — the suite's "I press the X key"
    /// step. The shortcuts are bound on `document`, so a key sent while a field
    /// holds focus is typed into the field instead of acting.
    async fn press(&self, key: &str) -> Result<()>;

    /// Presses a key without moving focus first.
    ///
    /// The help overlay puts focus on its own Esc button, and clicking the body
    /// to "focus nothing" would both blur it and trip its click-outside-to-close
    /// handler.
    async fn press_focused(&self, key: &str) -> Result<()>;
}

/// Clicks an element once it is actually clickable.
///
/// Playwright checks actionability before every click and waits for it;
/// `WebElement::click` does not, and dispatches into a disabled control happily.
/// The reading pane's Summarize toggle starts disabled and is enabled only once
/// the neighbors call resolves, so a click that does not wait fires nothing and
/// fails several steps later with no explanation.
///
/// [`Dom::click`] and [`Dom::click_css`] already wait; this is for callers that
/// have found the element themselves, usually by scoping to a row.
///
/// # Errors
///
/// Fails when the element never becomes clickable.
pub async fn click_when_ready(element: &WebElement) -> Result<()> {
    element
        .wait_until()
        .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
        .clickable()
        .await
        .context("the element never became clickable")?;
    element.click().await?;
    Ok(())
}

/// Clicks a control that navigates, and waits for the navigation to land.
///
/// The element-handle form of [`Dom::submit_css`], for the row-scoped controls
/// found by walking a table rather than by selector. Without the wait, the
/// assertion that follows reads the *old* page.
///
/// # Errors
///
/// Fails when the click does not replace the document.
pub async fn submit_element(driver: &WebDriver, element: &WebElement) -> Result<()> {
    let document = driver.find(By::Tag("html")).await?;
    click_when_ready(element).await?;
    document
        .wait_until()
        .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
        .stale()
        .await
        .context("the click did not navigate anywhere")?;
    Ok(())
}

/// The text an element contains, as Playwright compared it.
///
/// **Not** `WebElement::text`, which returns *rendered* text — put through
/// `text-transform`, so a heading styled `uppercase` reads `READING FEED` where
/// the markup says `Reading Feed`. Playwright's matchers all read `textContent`
/// instead, so every assertion ported from them has to as well, or it compares
/// against the stylesheet rather than the page's content.
#[allow(async_fn_in_trait)]
pub trait TextContent {
    /// The element's `textContent`, untouched by CSS.
    async fn content_text(&self) -> Result<String>;
}

impl TextContent for WebElement {
    async fn content_text(&self) -> Result<String> {
        Ok(self.prop("textContent").await?.unwrap_or_default())
    }
}

/// Finding elements inside another element, for the steps that scoped a query
/// to a row or a panel.
#[allow(async_fn_in_trait)]
pub trait Within {
    /// The descendant link with this accessible name, if there is one.
    ///
    /// Stands in for `getByRole("link", { name })`.
    async fn link_named(&self, name: &str) -> Result<Option<WebElement>>;

    /// The descendant button with this accessible name, if there is one.
    ///
    /// Stands in for `getByRole("button", { name })`, which matches either the
    /// rendered text or an `aria-label` — both are in use here.
    async fn button_named(&self, name: &str) -> Result<Option<WebElement>>;

    /// The descendant carrying `data-testid`, without waiting.
    async fn test_id_opt(&self, id: &str) -> Result<Option<WebElement>>;

    /// The descendant carrying `data-testid`, which must be there.
    async fn test_id(&self, id: &str) -> Result<WebElement>;
}

impl Within for WebElement {
    async fn link_named(&self, name: &str) -> Result<Option<WebElement>> {
        Ok(self
            .query(By::XPath(named_role_xpath("a", name)))
            .nowait()
            .first_opt()
            .await?)
    }

    async fn button_named(&self, name: &str) -> Result<Option<WebElement>> {
        Ok(self
            .query(By::XPath(named_role_xpath("button", name)))
            .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
            .first_opt()
            .await?)
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
                let text = element.content_text().await.unwrap_or_default();
                if text.contains(needle) {
                    return Ok(());
                }
                last = Some(text);
            }
            if Instant::now() >= deadline {
                let seen =
                    last.map_or_else(|| "no such element".to_owned(), |text| format!("{text:?}"));
                bail!(
                    "testid `{id}`: expected text containing {needle:?}, \
                     last saw {seen} after {WAIT_TIMEOUT:?}"
                );
            }
            tokio::time::sleep(WAIT_INTERVAL).await;
        }
    }

    async fn text_of(&self, id: &str) -> Result<String> {
        self.test_id(id).await?.content_text().await
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
            // A stale handle means the document was replaced between finding
            // the element and reading it — which is exactly what these
            // assertions are waiting through, since most of them follow a form
            // post. Treat it as "not yet", not as a failure.
            match element.attr(attr).await {
                Ok(value) => Ok(value.as_deref() == expected),
                Err(_) => Ok(false),
            }
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
        let xpath = format!(
            "//*[contains(normalize-space(.), {literal})][not(.//*[contains(normalize-space(.), {literal})])]"
        );
        displayed(self, By::XPath(xpath), &format!("text {text:?}"))
            .await
            .map(|_| ())
    }

    async fn eval(&self, script: &str) -> Result<serde_json::Value> {
        Ok(self.execute(script, vec![]).await?.json().clone())
    }

    async fn text_of_css(&self, selector: &str) -> Result<Option<String>> {
        let Some(element) = self.css_opt(selector).await? else {
            return Ok(None);
        };
        Ok(element.content_text().await.ok())
    }

    async fn text_of_test_id(&self, id: &str) -> Result<Option<String>> {
        let Some(element) = self.test_id_opt(id).await? else {
            return Ok(None);
        };
        Ok(element.content_text().await.ok())
    }

    async fn texts_of(&self, selector: &str) -> Result<Vec<String>> {
        let mut texts = Vec::new();
        for element in self.css_all(selector).await? {
            texts.push(element.content_text().await?);
        }
        Ok(texts)
    }

    async fn is_checked(&self, id: &str) -> Result<bool> {
        Ok(self.test_id(id).await?.is_selected().await?)
    }

    async fn check(&self, id: &str) -> Result<()> {
        if !self.is_checked(id).await? {
            self.click(id).await?;
        }
        Ok(())
    }

    async fn is_focused(&self, id: &str) -> Result<bool> {
        let element = self.test_id(id).await?;
        Ok(self.active_element().await? == element)
    }

    async fn computed_style(&self, selector: &str, property: &str) -> Result<String> {
        // Deliberately not `css`, which waits for a *displayed* element: the
        // scenarios that read a computed style are usually asking whether
        // something is `display: none`, and requiring it to be visible first
        // makes that question unanswerable.
        let element = self
            .css_opt(selector)
            .await?
            .with_context(|| format!("no element matches `{selector}`"))?;
        let value = self
            .execute(
                "return getComputedStyle(arguments[0]).getPropertyValue(arguments[1]);",
                vec![element.to_json()?, serde_json::json!(property)],
            )
            .await?;
        Ok(value.json().as_str().unwrap_or_default().to_owned())
    }

    async fn bounding_box(&self, selector: &str) -> Result<(f64, f64, f64, f64)> {
        let rect = self
            .execute(
                "const r = arguments[0].getBoundingClientRect();\
                 return [r.x, r.y, r.width, r.height];",
                vec![self.css(selector).await?.to_json()?],
            )
            .await?;
        let values = rect
            .json()
            .as_array()
            .context("the rect probe did not return an array")?
            .iter()
            .map(|value| value.as_f64().unwrap_or_default())
            .collect::<Vec<_>>();
        let [x, y, width, height] = values[..] else {
            bail!(
                "the rect probe returned {} values, expected 4",
                values.len()
            );
        };
        Ok((x, y, width, height))
    }

    async fn press(&self, key: &str) -> Result<()> {
        self.find(By::Tag("body")).await?.click().await?;
        self.press_focused(key).await
    }

    async fn press_focused(&self, key: &str) -> Result<()> {
        // Only `Enter` and `Escape` are named keys in this suite; everything
        // else is a literal character, including the ones needing shift (`A`,
        // `}`, `?`), which WebDriver applies for us.
        let keys = match key {
            "Enter" => char::from(Key::Enter).to_string(),
            "Escape" => char::from(Key::Escape).to_string(),
            other => other.to_owned(),
        };
        self.action_chain().send_keys(keys).perform().await?;
        Ok(())
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

/// An `XPath` for "the `tag` whose accessible name matches `name`", the way
/// `getByRole(role, { name })` matches.
///
/// Playwright's default is **substring, case-insensitive** — not the exact
/// comparison the name suggests. Matching exactly instead makes a button
/// labelled "Load more entries" invisible to a step asking for "Load more".
///
/// The name comes from the rendered text or an `aria-label`, both in use here.
/// `XPath` 1.0 has no case-insensitive compare, so both sides are folded with
/// `translate`.
fn named_role_xpath(tag: &str, name: &str) -> String {
    const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
    let needle = xpath_literal(&name.to_lowercase());
    let fold = |expression: &str| format!("translate({expression}, '{UPPER}', '{LOWER}')");
    format!(
        ".//{tag}[contains({}, {needle}) or contains({}, {needle})]",
        fold("normalize-space(.)"),
        fold("@aria-label"),
    )
}

/// Quotes a string for `XPath`, which has no escape syntax of its own.
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
