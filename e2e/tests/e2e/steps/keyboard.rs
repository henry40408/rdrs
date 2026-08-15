//! Keyboard shortcuts, the selection they move, the go-to hint and the help
//! overlay.
//!
//! Split out of `entries.steps.js` — see [`super::entries`].

use anyhow::{Result, ensure};
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::wait::{eventually, eventually_some};
use rdrs_e2e::world::RdrsWorld;

#[when(expr = "I press the {string} key")]
async fn press_key(world: &mut RdrsWorld, key: String) -> Result<()> {
    world.driver()?.press(&key).await
}

/// The plain press step clicks `<body>` first, which would blur the help
/// overlay (focus sits on its Esc button after `show()`) and can trigger its
/// click-outside-to-close handler. This sends the key to whatever holds focus.
#[when(expr = "I press the {string} key without refocusing")]
async fn press_key_focused(world: &mut RdrsWorld, key: String) -> Result<()> {
    world.driver()?.press_focused(&key).await
}

/// Pre-arms a one-shot handler so the next `window.confirm` auto-accepts, for
/// the shortcuts that go through a confirmation prompt (Shift+K → "Mark all as
/// read?"). Must be registered *before* the keystroke.
///
/// Overriding `window.confirm` rather than answering a driver-level alert: the
/// `WebDriver` alert commands race the page, which resumes the moment the dialog
/// is dismissed, and the shortcut's own handler is what has to observe `true`.
#[when("I confirm the next dialog")]
async fn confirm_next_dialog(world: &mut RdrsWorld) -> Result<()> {
    accept_next_dialog(world).await
}

/// Arms the one-shot `window.confirm` override, shared with the triage steps
/// whose dropdown and Mark-Above button go through the same prompt.
pub async fn accept_next_dialog(world: &RdrsWorld) -> Result<()> {
    world
        .driver()?
        .eval(
            r"
            const original = window.confirm;
            window.confirm = function () {
              window.confirm = original;
              return true;
            };
            return true;
            ",
        )
        .await?;
    Ok(())
}

// ── Selection ────────────────────────────────────────────────────────────────

#[then("the first entry is selected")]
async fn first_selected(world: &mut RdrsWorld) -> Result<()> {
    expect_selected_index(world, 0).await
}

#[then("the second entry is selected")]
async fn second_selected(world: &mut RdrsWorld) -> Result<()> {
    expect_selected_index(world, 1).await
}

/// `.selected` is client-side only, so a stale highlight left on a row the
/// reader has navigated away from shows up here as a count of 2.
#[then("exactly one entry is selected")]
async fn exactly_one_selected(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("exactly one selected row", || async {
        Ok(driver.css_all("[data-entry-row].selected").await?.len() == 1)
    })
    .await
}

#[then(expr = "the selected entry is titled {string}")]
async fn selected_entry_titled(world: &mut RdrsWorld, title: String) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("the selected row is {title:?}"), || async {
        let Some(row) = driver.css_opt("[data-entry-row].selected").await? else {
            return Ok(false);
        };
        Ok(row.content_text().await?.contains(&title))
    })
    .await
}

async fn expect_selected_index(world: &RdrsWorld, index: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually(&format!("entry {index} is selected"), || async {
        let rows = driver.test_ids("entry-item").await?;
        let Some(row) = rows.get(index) else {
            return Ok(false);
        };
        let classes = row.class_name().await?.unwrap_or_default();
        Ok(classes
            .split_whitespace()
            .any(|name| name == "selected" || name == "active"))
    })
    .await
}

// ── The go-to hint ───────────────────────────────────────────────────────────

#[then("the go-to hint is visible")]
async fn goto_hint_visible(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.css(".kbd-hint").await.map(|_| ())
}

#[then("the go-to hint is gone")]
async fn goto_hint_gone(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the go-to hint to disappear", || async {
        Ok(driver.css_all(".kbd-hint").await?.is_empty())
    })
    .await
}

// ── The help overlay ─────────────────────────────────────────────────────────

#[then("the keyboard shortcut help overlay is visible")]
async fn help_visible(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("kb-help").await
}

#[then("the keyboard shortcut help overlay is hidden")]
async fn help_hidden(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_hidden("kb-help").await
}

/// Compares the x of the first four Navigation-group descriptions (j/k,
/// o/Enter, Space — the wide key combo — and Esc): pre-fix, the Space row's key
/// cell overflows its column and pushes its description right.
///
/// Read through a script rather than a CSS query: Playwright's selectors pierce
/// an open shadow root and `WebDriver`'s do not, so the measurement has to happen
/// inside the page.
#[then("the help overlay descriptions are aligned")]
async fn help_aligned(world: &mut RdrsWorld) -> Result<()> {
    let offsets = world
        .driver()?
        .eval(
            r"
            const root = document.querySelector('rdrs-kb-help').shadowRoot;
            return [...root.querySelectorAll('.shortcut-desc')]
              .slice(0, 4)
              .map((el) => el.getBoundingClientRect().x);
            ",
        )
        .await?;
    let offsets: Vec<f64> = offsets
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("the overlay probe returned no descriptions"))?
        .iter()
        .map(|value| value.as_f64().unwrap_or_default())
        .collect();
    ensure!(
        offsets.len() == 4,
        "the overlay renders {} descriptions, expected at least 4",
        offsets.len()
    );
    for (index, x) in offsets.iter().enumerate().skip(1) {
        ensure!(
            (x - offsets[0]).abs() < 1.0,
            "description {index} starts at {x}, but the first starts at {}",
            offsets[0]
        );
    }
    Ok(())
}

/// The shadow stylesheet references tokens with no `var(--x, fallback)`
/// defaults, so a token renamed in `app.css` would leave the modal unstyled —
/// transparent panel, default text colour — while every other help-overlay
/// assertion still passed.
///
/// Each token is resolved through a throwaway element in the light DOM and read
/// back as a *computed* value, so both sides of the comparison go through the
/// same normalisation: the browser rewrites colours to `rgb()` and strips
/// quotes from font stacks, and comparing against the raw token text fails on
/// formatting alone.
#[then("the help overlay resolves its design tokens")]
async fn help_tokens(world: &mut RdrsWorld) -> Result<()> {
    let probe = world
        .driver()?
        .eval(
            r"
            const modal = document.querySelector('rdrs-kb-help').shadowRoot
              .querySelector('.modal');
            const probeValue = (property, token) => {
              const el = document.createElement('span');
              el.style[property] = `var(${token})`;
              document.body.appendChild(el);
              const value = getComputedStyle(el)[property];
              el.remove();
              return value;
            };
            const computed = getComputedStyle(modal);
            return {
              background: computed.backgroundColor,
              expectedBackground: probeValue('backgroundColor', '--color-panel'),
              color: computed.color,
              expectedColor: probeValue('color', '--color-text'),
              fontFamily: computed.fontFamily,
              expectedFontFamily: probeValue('fontFamily', '--font-ui'),
            };
            ",
        )
        .await?;

    for (actual, expected, what) in [
        ("background", "expectedBackground", "background colour"),
        ("color", "expectedColor", "text colour"),
        ("fontFamily", "expectedFontFamily", "font stack"),
    ] {
        let actual = probe[actual].as_str().unwrap_or_default();
        let expected = probe[expected].as_str().unwrap_or_default();
        ensure!(
            actual == expected,
            "the overlay's {what} is {actual:?}, but the token resolves to {expected:?}"
        );
    }
    Ok(())
}

// ── Shortcuts that open a tab ────────────────────────────────────────────────

/// Seeded entry links point at `https://example.com/…`. What this asserts is
/// *which URL* the shortcut targets, not that the page loads — so the origin is
/// stubbed rather than fetched. Without the stub the popup's navigation fails
/// DNS resolution and its URL collapses to `chrome-error://chromewebdata/` on
/// any machine without internet.
#[then(expr = "pressing the {string} key opens a new tab at {string}")]
async fn key_opens_tab(world: &mut RdrsWorld, key: String, expected: String) -> Result<()> {
    world.stub_external_pages().await?;

    let driver = world.driver()?;
    let before = driver.windows().await?;
    driver.press(&key).await?;

    // A popup can surface before its navigation commits, so this waits for a
    // *new* handle and then for that handle to report a real URL.
    let handle = eventually_some("a new tab to open", || async {
        let now = driver.windows().await?;
        Ok(now.into_iter().find(|handle| !before.contains(handle)))
    })
    .await?;

    let original = driver.window().await?;
    driver.switch_to_window(handle).await?;
    let result = eventually(&format!("the new tab to reach {expected:?}"), || async {
        Ok(driver.current_url().await?.as_str().contains(&expected))
    })
    .await;
    let url = driver.current_url().await?;
    driver.close_window().await?;
    driver.switch_to_window(original).await?;
    result.map_err(|_| anyhow::anyhow!("the new tab opened at {url}, expected {expected:?}"))
}
