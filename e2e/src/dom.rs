//! Page helpers on `WebDriver`, with the explicit waits it lacks.
//!
//! * Presence and visibility differ; conflating them lets a `display: none`
//!   regression pass.
//! * Absence queries use `nowait`; wrap them in [`crate::wait::eventually`] when
//!   the page has to *become* empty.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use thirtyfour::components::SelectElement;
use thirtyfour::prelude::*;

use crate::browser::{WAIT_INTERVAL, WAIT_TIMEOUT};

/// Page-level queries and actions.
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

    /// Replaces a field's contents.
    async fn fill(&self, id: &str, value: &str) -> Result<()>;

    /// Clicks the element once it is clickable.
    async fn click(&self, id: &str) -> Result<()>;

    /// Waits for the element to be displayed.
    async fn expect_visible(&self, id: &str) -> Result<()>;

    /// Waits for the element to stop being displayed (it may stay in the DOM).
    async fn expect_hidden(&self, id: &str) -> Result<()>;

    /// Waits for no element with that id to exist at all.
    async fn expect_absent(&self, id: &str) -> Result<()>;

    /// Waits for the element's rendered text to contain `needle`.
    async fn expect_text(&self, id: &str, needle: &str) -> Result<()>;

    /// The element's rendered text, once it is displayed.
    async fn text_of(&self, id: &str) -> Result<String>;

    /// Is the element present and displayed, as the page stands right now?
    async fn is_visible(&self, id: &str) -> Result<bool>;

    /// The heading with exactly this (whitespace-normalized) text, if any.
    async fn heading_opt(&self, name: &str) -> Result<Option<WebElement>>;

    /// [`Dom::fill`] by CSS selector, for pages with duplicate test ids.
    async fn fill_css(&self, selector: &str, value: &str) -> Result<()>;

    /// Clicks the element a CSS selector picks out, once it is clickable.
    async fn click_css(&self, selector: &str) -> Result<()>;

    /// Clicks a navigating control and waits for the old document to go stale,
    /// which (unlike watching the URL) catches a redirect back to the same page.
    async fn submit_css(&self, selector: &str) -> Result<()>;

    /// [`Dom::submit_css`] addressed by `data-testid`.
    async fn submit(&self, id: &str) -> Result<()>;

    /// Chooses an `<option>` by value.
    async fn select_option(&self, id: &str, value: &str) -> Result<()>;

    /// A form control's current value.
    async fn value_of(&self, id: &str) -> Result<String>;

    /// Waits for an attribute on the element a CSS selector picks out to equal
    /// `expected`, or to be absent when `expected` is `None`.
    async fn expect_attr(&self, selector: &str, attr: &str, expected: Option<&str>) -> Result<()>;

    /// The table row containing `text`.
    async fn row_with_text(&self, text: &str) -> Result<WebElement>;

    /// Waits for the innermost element containing `text` to be displayed.
    async fn expect_text_somewhere(&self, text: &str) -> Result<()>;

    /// Evaluates a script and hands back the JSON it returned.
    async fn eval(&self, script: &str) -> Result<serde_json::Value>;

    /// Finds and reads text in one go; `None` if absent *or* swapped away
    /// mid-read, which a poll should treat as "not yet".
    async fn text_of_css(&self, selector: &str) -> Result<Option<String>>;

    /// [`Dom::text_of_css`] addressed by `data-testid`.
    async fn text_of_test_id(&self, id: &str) -> Result<Option<String>>;

    /// The text of every match, in document order; comparing the whole list
    /// also pins the count.
    async fn texts_of(&self, selector: &str) -> Result<Vec<String>>;

    /// Is the checkbox ticked?
    async fn is_checked(&self, id: &str) -> Result<bool>;

    /// Ticks a checkbox if it is not already.
    async fn check(&self, id: &str) -> Result<()>;

    /// Does this element have keyboard focus?
    async fn is_focused(&self, id: &str) -> Result<bool>;

    /// One computed style property of the first element a selector matches.
    async fn computed_style(&self, selector: &str, property: &str) -> Result<String>;

    /// The first match's `(x, y, width, height)` in viewport coordinates
    /// (`WebElement::rect` is in document coordinates).
    async fn bounding_box(&self, selector: &str) -> Result<(f64, f64, f64, f64)>;

    /// Clicks the body, then presses a key, so a focused field doesn't swallow
    /// the shortcut.
    async fn press(&self, key: &str) -> Result<()>;

    /// Presses a key without moving focus (clicking the body would close the
    /// help overlay).
    async fn press_focused(&self, key: &str) -> Result<()>;
}

/// Clicks an element once it is clickable; `WebElement::click` happily clicks
/// a still-disabled control (e.g. Summarize before neighbors load).
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

/// [`Dom::submit_css`] for an element handle.
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

/// `textContent` rather than `WebElement::text`, which applies CSS
/// `text-transform` (e.g. `uppercase`).
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

/// Queries scoped to a descendant of an element.
#[allow(async_fn_in_trait)]
pub trait Within {
    /// The descendant link with this accessible name, if there is one.
    async fn link_named(&self, name: &str) -> Result<Option<WebElement>>;

    /// The descendant button with this accessible name (text or `aria-label`).
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
        // Send Keys appends, so clear first.
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
        // Hand-rolled so the failure can name the text that *was* there.
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
        // `prop`, not `attr`: the attribute is only the initial value.
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
            // A stale handle (document replaced after a post) means "not yet".
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
        // `not(.//*[…])` keeps the innermost match; otherwise every ancestor
        // up to `<body>` matches.
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
        // Not `css`: requiring a displayed element would make `display: none`
        // unobservable.
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
        // Only Enter/Escape are named; WebDriver applies shift for the rest.
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

/// Every match as the page stands, without waiting.
async fn all(driver: &WebDriver, by: By) -> Result<Vec<WebElement>> {
    Ok(driver.query(by).nowait().all_from_selector().await?)
}

/// An `XPath` for a `tag` whose text or `aria-label` contains `name`,
/// case-insensitively (folded with `translate`, as `XPath` 1.0 lacks it).
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

/// Quotes a string for `XPath` (no escapes; both quotes need `concat()`).
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
