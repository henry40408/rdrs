//! Client-side DOM-render benchmark for the entry-list / reading-pane swap.
//!
//! Measures the part of a swap that is purely the browser's: everything after
//! the server's bytes have arrived. `performSwap()` is instrumented from the
//! outside — no source edits — by wrapping two globals in an init script that
//! runs before `app.js`:
//!
//! | phase       | what it covers                                              |
//! |-------------|-------------------------------------------------------------|
//! | `parseMs`   | `DOMParser.parseFromString` on the response body            |
//! | `applyMs`   | parse-return → `rdrs:swap-complete`: the skip check, the morph, and node insertion |
//! | `handlerMs` | the synchronous `rdrs:swap-complete` dispatch — every post-swap hook (time tooltips, sidebar refresh, control rebinding, image init) |
//!
//! ```text
//! cd e2e && cargo run --bin dom-bench -- [--entries 200] [--iterations 40] [--profile]
//! ```
//!
//! Reports p50/p90/mean per phase. With `--profile` it also runs a CDP CPU
//! profile over the same loop and prints the top functions by self time, which
//! is what attributes a phase to a specific function.

use std::collections::HashMap;

use anyhow::{Result, bail};
use rdrs_e2e::api::{Api, PASSWORD};
use rdrs_e2e::browser::{Browser, Scripting};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::seed::Seed;
use rdrs_e2e::server::Harness;
use rdrs_e2e::wait::eventually;
use serde::Deserialize;
use thirtyfour::prelude::*;

/// Wraps `DOMParser.parseFromString` and `document.dispatchEvent` so each swap
/// records its own timings, without touching `app.js`.
const INSTRUMENT: &str = r"
(() => {
  window.__bench = { swaps: [], current: null };
  const OrigParser = DOMParser.prototype.parseFromString;
  DOMParser.prototype.parseFromString = function (...args) {
    const t0 = performance.now();
    const doc = OrigParser.apply(this, args);
    const t1 = performance.now();
    // The last parse before a dispatch is the one that produced the swap.
    window.__bench.current = { parseMs: t1 - t0, parsedAt: t1, bytes: (args[0] || '').length };
    return doc;
  };
  const origDispatch = document.dispatchEvent.bind(document);
  document.dispatchEvent = function (ev) {
    if (!ev || ev.type !== 'rdrs:swap-complete') return origDispatch(ev);
    const cur = window.__bench.current;
    const t0 = performance.now();
    const r = origDispatch(ev);
    const t1 = performance.now();
    window.__bench.swaps.push({
      parseMs: cur ? cur.parseMs : null,
      applyMs: cur ? t0 - cur.parsedAt : null,
      handlerMs: t1 - t0,
      bytes: cur ? cur.bytes : null,
    });
    window.__bench.current = null;
    return r;
  };
})();
";

/// What each iteration does — the interaction whose swap is being measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `j`: a reading-pane swap.
    Nav,
    /// Load More: appends a page of rows to a list that keeps growing — the
    /// path where a per-swap sweep of the whole document compounds.
    LoadMore,
    /// Mark Above: re-renders the whole `[data-entries-list]` container, so the
    /// morph runs over every rendered row.
    MarkAbove,
}

/// One swap's timings, as the page recorded them.
#[derive(Debug, Clone, Deserialize)]
struct Swap {
    #[serde(rename = "parseMs")]
    parse_ms: Option<f64>,
    #[serde(rename = "applyMs")]
    apply_ms: Option<f64>,
    #[serde(rename = "handlerMs")]
    handler_ms: f64,
    bytes: Option<f64>,
}

struct Options {
    entries: u32,
    iterations: usize,
    warmup: usize,
    profile: bool,
    label: String,
    rows: usize,
    throttle: f64,
    mode: Mode,
}

/// Reads one option at the field's own type.
///
/// Parsing straight to `usize` or `u32` rather than through `f64` is what
/// keeps a count from arriving negative or fractional.
fn parse<T>(raw: Option<&str>, name: &str, fallback: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.map_or(Ok(fallback), |value| {
        value
            .parse()
            .map_err(|error| anyhow::anyhow!("--{name} takes a number, got {value:?}: {error}"))
    })
}

impl Options {
    /// Parses `--name value` pairs, the same shape the old script accepted.
    fn from_args() -> Result<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let value = |name: &str| -> Option<&str> {
            let index = args.iter().position(|arg| arg == &format!("--{name}"))?;
            args.get(index + 1)
                .map(String::as_str)
                .filter(|next| !next.starts_with("--"))
        };
        let flag = |name: &str| args.iter().any(|arg| arg == &format!("--{name}"));

        let mode = match value("mode").unwrap_or("nav") {
            "nav" => Mode::Nav,
            "loadmore" => Mode::LoadMore,
            "markabove" => Mode::MarkAbove,
            other => bail!("unknown --mode {other:?}: expected nav, loadmore or markabove"),
        };
        Ok(Self {
            entries: parse(value("entries"), "entries", 200)?,
            iterations: parse(value("iterations"), "iterations", 40)?,
            warmup: parse(value("warmup"), "warmup", 8)?,
            profile: flag("profile"),
            label: value("label").unwrap_or("run").to_owned(),
            rows: parse(value("rows"), "rows", 0)?,
            throttle: parse(value("throttle"), "throttle", 1.0)?,
            mode,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args()?;
    let harness = Harness::start().await?;
    let endpoints = harness.endpoints().clone();

    // ---- seed a long backlog ----
    let api = Api::new(&endpoints.base_url)?;
    let user = "benchuser";
    api.setup_first_account(user, PASSWORD).await?;
    let seed = Seed::open(&endpoints.db_path).await?;
    let user_id = seed.user_id(user).await?;
    let category_id = seed.create_category(user_id, "Bench").await?;
    let feed_id = seed
        .create_feed(
            category_id,
            "https://example.com/bench.xml",
            Some("Bench Feed"),
        )
        .await?;
    seed.seed_test_entries(feed_id, options.entries).await?;

    let browser = Browser::open(Scripting::Enabled).await?;
    let result = run(&browser, &endpoints.base_url, user, &options).await;
    browser.quit().await?;
    result
}

async fn run(browser: &Browser, base: &str, user: &str, options: &Options) -> Result<()> {
    let driver = browser.driver();
    driver
        .cdp()
        .send_raw(
            "Page.addScriptToEvaluateOnNewDocument",
            serde_json::json!({ "source": INSTRUMENT }),
        )
        .await?;

    driver.goto(format!("{base}/login")).await?;
    driver.fill("username-input", user).await?;
    driver.fill("password-input", PASSWORD).await?;
    driver.submit("login-submit").await?;

    let mut rows = driver.css_all("[data-entry-row]").await?.len();
    if rows == 0 {
        bail!("no entry rows rendered — seeding failed");
    }

    // Grow the rendered list: the swap cost that matters is the one a reader
    // pays after paging a long backlog into the DOM, not the first 50 rows.
    while options.rows > rows {
        if driver.test_id_opt("load-more-btn").await?.is_none() {
            break;
        }
        driver.click("load-more-btn").await?;
        let before = rows;
        eventually("more rows to arrive", || async {
            Ok(driver.css_all("[data-entry-row]").await?.len() > before)
        })
        .await?;
        rows = driver.css_all("[data-entry-row]").await?.len();
    }

    if options.throttle > 1.0 {
        driver
            .cdp()
            .send_raw(
                "Emulation.setCPUThrottlingRate",
                serde_json::json!({ "rate": options.throttle }),
            )
            .await?;
    }

    // Open the first entry so `j` becomes a reading-pane swap rather than a
    // pure cursor move, which is the interaction being measured.
    driver.css("[data-entry-row]").await?.click().await?;
    eventually("the first swap to be recorded", || async {
        Ok(swap_count(driver).await? >= 1)
    })
    .await?;

    for _ in 0..options.warmup {
        step(driver, options.mode).await?;
    }
    driver
        .eval("window.__bench.swaps.length = 0; return true;")
        .await?;

    if options.profile {
        let cdp = driver.cdp();
        cdp.send_raw("Profiler.enable", serde_json::json!({}))
            .await?;
        cdp.send_raw(
            "Profiler.setSamplingInterval",
            serde_json::json!({ "interval": 100 }),
        )
        .await?;
        cdp.send_raw("Profiler.start", serde_json::json!({}))
            .await?;
    }

    for _ in 0..options.iterations {
        step(driver, options.mode).await?;
    }

    let swaps: Vec<Swap> =
        serde_json::from_value(driver.eval("return window.__bench.swaps;").await?)?;
    report(&swaps, rows, options);

    if options.profile {
        let profile = driver
            .cdp()
            .send_raw("Profiler.stop", serde_json::json!({}))
            .await?;
        report_profile(&serde_json::from_value(profile["profile"].clone())?);
    }
    Ok(())
}

async fn swap_count(driver: &WebDriver) -> Result<usize> {
    let count = driver.eval("return window.__bench.swaps.length;").await?;
    Ok(count.as_u64().unwrap_or(0) as usize)
}

async fn step(driver: &WebDriver, mode: Mode) -> Result<()> {
    let before = swap_count(driver).await?;
    match mode {
        Mode::LoadMore => {
            if driver.test_id_opt("load-more-btn").await?.is_none() {
                bail!("no Load More button left — seed more entries");
            }
            driver.click("load-more-btn").await?;
        }
        Mode::MarkAbove => {
            driver.press("j").await?;
            driver.click("mark-above-btn").await?;
        }
        Mode::Nav => driver.press("j").await?,
    }
    eventually("the next swap to land", || async {
        Ok(swap_count(driver).await? > before)
    })
    .await
}

/// Mean, median, 90th percentile and worst case for one phase.
#[derive(Debug, Default)]
struct Stats {
    n: usize,
    mean: f64,
    p50: f64,
    p90: f64,
    max: f64,
}

fn stats(values: &[f64]) -> Stats {
    if values.is_empty() {
        return Stats::default();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    // Taken as a ratio of the length in integers — the same index the old
    // script's `Math.floor(p * len)` produced, without a float conversion that
    // has to be reasoned about for sign or range.
    let quantile = |numerator: usize, denominator: usize| {
        let index = sorted.len() * numerator / denominator;
        sorted[index.min(sorted.len() - 1)]
    };
    Stats {
        n: sorted.len(),
        mean: values.iter().sum::<f64>() / values.len() as f64,
        p50: quantile(1, 2),
        p90: quantile(9, 10),
        max: sorted[sorted.len() - 1],
    }
}

fn report(swaps: &[Swap], rows: usize, options: &Options) {
    let parse: Vec<f64> = swaps.iter().filter_map(|s| s.parse_ms).collect();
    let apply: Vec<f64> = swaps.iter().filter_map(|s| s.apply_ms).collect();
    let handler: Vec<f64> = swaps.iter().map(|s| s.handler_ms).collect();
    let total: Vec<f64> = swaps
        .iter()
        .map(|s| s.parse_ms.unwrap_or(0.0) + s.apply_ms.unwrap_or(0.0) + s.handler_ms)
        .collect();
    let bytes: Vec<f64> = swaps.iter().filter_map(|s| s.bytes).collect();

    println!(
        "\n=== dom-bench [{}] mode={:?} entries={} rows={rows} throttle={}x swaps={} ===",
        options.label,
        options.mode,
        options.entries,
        options.throttle,
        swaps.len()
    );
    println!("phase        mean     p50     p90     max   (ms)");
    for (name, values) in [
        ("parse   ", &parse),
        ("apply   ", &apply),
        ("handlers", &handler),
        ("TOTAL   ", &total),
    ] {
        let s = stats(values);
        println!(
            "{name} {:>7.2} {:>7.2} {:>7.2} {:>7.2}",
            s.mean, s.p50, s.p90, s.max
        );
    }
    println!("payload  {:.1} KiB mean", stats(&bytes).mean / 1024.0);
    let json = |name: &str, s: &Stats| {
        format!(
            r#""{name}":{{"n":{},"mean":{:.4},"p50":{:.4},"p90":{:.4},"max":{:.4}}}"#,
            s.n, s.mean, s.p50, s.p90, s.max
        )
    };
    println!(
        r#"JSON {{"label":"{}","entries":{},{},{},{},{}}}"#,
        options.label,
        options.entries,
        json("parse", &stats(&parse)),
        json("apply", &stats(&apply)),
        json("handler", &stats(&handler)),
        json("total", &stats(&total)),
    );
}

// ── CPU profile ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Profile {
    nodes: Vec<Node>,
    samples: Vec<u32>,
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(rename = "endTime")]
    end_time: f64,
}

#[derive(Debug, Deserialize)]
struct Node {
    id: u32,
    #[serde(rename = "callFrame")]
    call_frame: CallFrame,
    #[serde(default)]
    children: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct CallFrame {
    #[serde(rename = "functionName")]
    function_name: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "lineNumber")]
    line_number: i64,
}

fn report_profile(profile: &Profile) {
    let by_id: HashMap<u32, &Node> = profile.nodes.iter().map(|node| (node.id, node)).collect();
    let mut parent: HashMap<u32, u32> = HashMap::new();
    for node in &profile.nodes {
        for child in &node.children {
            parent.insert(*child, node.id);
        }
    }

    let label = |node: &Node| {
        let file = node.call_frame.url.rsplit('/').next().unwrap_or_default();
        let name = if node.call_frame.function_name.is_empty() {
            "(anonymous)"
        } else {
            &node.call_frame.function_name
        };
        format!("{name} @ {file}:{}", node.call_frame.line_number + 1)
    };
    let is_app = |node: &Node| node.call_frame.url.contains("/static/js/");

    let mut self_time: HashMap<String, usize> = HashMap::new();
    let mut owner_time: HashMap<String, usize> = HashMap::new();
    for id in &profile.samples {
        let Some(node) = by_id.get(id) else { continue };
        *self_time.entry(label(node)).or_default() += 1;
        // Attribute native frames (querySelector, replaceState…) to the nearest
        // app-level caller — self time alone cannot say which hook paid for it.
        let mut current = Some(*node);
        while let Some(node) = current
            && !is_app(node)
        {
            current = parent.get(&node.id).and_then(|id| by_id.get(id)).copied();
        }
        let key = current.map_or_else(|| "(outside app js)".to_owned(), &label);
        *owner_time.entry(key).or_default() += 1;
    }

    let total_samples = profile.samples.len();
    let wall_ms = (profile.end_time - profile.start_time) / 1000.0;
    let table = |counts: &HashMap<String, usize>, title: &str| {
        println!("\n--- {title} ({total_samples} samples over {wall_ms:.0} ms wall) ---");
        let mut rows: Vec<_> = counts.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in rows.into_iter().take(18) {
            let share = *count as f64 / total_samples as f64;
            println!(
                "{:>5.1}%  {:>5.0} ms  {name}",
                share * 100.0,
                share * wall_ms
            );
        }
    };
    table(&self_time, "CPU self time");
    table(&owner_time, "CPU time by nearest app-level frame");
}
