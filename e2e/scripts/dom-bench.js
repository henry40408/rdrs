// Client-side DOM-render benchmark for the entry-list / reading-pane swap.
//
// Measures the part of a swap that is purely the browser's: everything after
// the server's bytes have arrived. `performSwap()` is instrumented from the
// outside — no source edits — by wrapping two globals in an init script that
// runs before app.js:
//
//   parseMs     DOMParser.parseFromString on the response body
//   applyMs     parse-return → `rdrs:swap-complete` dispatch: the skip check
//               (comparableServerMarkup), morph, and node insertion
//   handlerMs   the synchronous duration of the `rdrs:swap-complete` dispatch,
//               i.e. every post-swap hook (time tooltips, sidebar refresh,
//               control rebinding, image init)
//
// Usage:  node scripts/dom-bench.js [--entries 200] [--iterations 40] [--profile]
//         RDRS_BIN=/path/to/rdrs node scripts/dom-bench.js
//
// Reports p50/p90/mean per phase. With --profile it also runs a CDP CPU
// profile over the same loop and prints the top functions by self time, which
// is what attributes a phase to a specific function.

import { chromium } from "playwright";
import { spawnRdrs } from "../support/server.js";
import { ApiHelper } from "../support/api.js";
import { SeedHelper } from "../support/seed.js";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  const v = process.argv[i + 1];
  return v === undefined || v.startsWith("--") ? true : v;
}

const ENTRIES = Number(arg("entries", 200));
const ITERATIONS = Number(arg("iterations", 40));
const WARMUP = Number(arg("warmup", 8));
const PROFILE = arg("profile", false) === true;
const LABEL = String(arg("label", "run"));
const ROWS = Number(arg("rows", 0));           // grow the list to >= N rows via Load More
const THROTTLE = Number(arg("throttle", 1));   // CDP CPU throttling rate (1 = none)
const MODE = String(arg("mode", "nav"));       // nav | markabove

const INIT_SCRIPT = `
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
`;

function stats(xs) {
  if (!xs.length) return { n: 0 };
  const s = [...xs].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return {
    n: s.length,
    mean: xs.reduce((a, b) => a + b, 0) / xs.length,
    p50: q(0.5),
    p90: q(0.9),
    max: s[s.length - 1],
  };
}

const fmt = (x) => (x === undefined ? "  —  " : x.toFixed(2).padStart(7));

async function main() {
  const server = await spawnRdrs();
  let browser;
  try {
    const api = new ApiHelper(server.url);
    const seed = new SeedHelper(server.dbPath);
    const user = "benchuser";
    const pass = "vulture-mango-77-quilt";
    await api.setupFirstAccount(user, pass);
    const userId = seed.getUserId(user);
    const categoryId = seed.createCategory(userId, "Bench");
    const feedId = seed.createFeed(categoryId, "https://example.com/bench.xml", "Bench Feed");
    seed.seedTestEntries(feedId, ENTRIES);
    const { cookie } = await api.login(user, pass);

    browser = await chromium.launch();
    const context = await browser.newContext();
    await context.addCookies(
      cookie.split("; ").map((kv) => {
        const i = kv.indexOf("=");
        return {
          name: kv.slice(0, i),
          value: kv.slice(i + 1),
          domain: "127.0.0.1",
          path: "/",
        };
      })
    );
    await context.addInitScript(INIT_SCRIPT);
    const page = await context.newPage();
    await page.goto(`${server.url}/`, { waitUntil: "networkidle" });

    let rows = await page.locator("[data-entry-row]").count();
    if (rows === 0) throw new Error("no entry rows rendered — seeding failed");

    // Grow the rendered list: the swap cost that matters is the one a reader
    // pays after paging a long backlog into the DOM, not the first 50 rows.
    while (ROWS && rows < ROWS) {
      const more = page.locator("[data-testid=load-more-btn]");
      if ((await more.count()) === 0) break;
      await more.first().click();
      await page.waitForFunction((n) => document.querySelectorAll("[data-entry-row]").length > n, rows, { timeout: 15000 });
      rows = await page.locator("[data-entry-row]").count();
    }

    const cdp = await context.newCDPSession(page);
    if (THROTTLE > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: THROTTLE });

    // Open the first entry so `j` becomes a reading-pane swap rather than a
    // pure cursor move, which is the interaction being measured.
    await page.locator("[data-entry-row]").first().click();
    await page.waitForFunction(() => window.__bench.swaps.length >= 1);

    const step = async () => {
      const before = await page.evaluate(() => window.__bench.swaps.length);
      if (MODE === "loadmore") {
        // Each click appends a page of rows to a list that keeps growing —
        // the path where a per-swap sweep of the whole document compounds.
        const more = page.locator("[data-testid=load-more-btn]");
        if ((await more.count()) === 0) throw new Error("no Load More button left — seed more entries");
        await more.first().click();
      } else if (MODE === "markabove") {
        // Re-renders the whole `[data-entries-list]` container: the morph path
        // over every rendered row.
        await page.keyboard.press("j");
        await page.locator("[data-testid=mark-above-btn]").first().click();
      } else {
        await page.keyboard.press("j");
      }
      await page.waitForFunction((n) => window.__bench.swaps.length > n, before, { timeout: 15000 });
    };

    for (let i = 0; i < WARMUP; i++) await step();
    await page.evaluate(() => { window.__bench.swaps.length = 0; });

    let profiler = null;
    if (PROFILE) {
      profiler = cdp;
      await profiler.send("Profiler.enable");
      await profiler.send("Profiler.setSamplingInterval", { interval: 100 });
      await profiler.send("Profiler.start");
    }

    for (let i = 0; i < ITERATIONS; i++) await step();

    const swaps = await page.evaluate(() => window.__bench.swaps);
    const parse = stats(swaps.map((s) => s.parseMs).filter((x) => x != null));
    const apply = stats(swaps.map((s) => s.applyMs).filter((x) => x != null));
    const handler = stats(swaps.map((s) => s.handlerMs));
    const total = stats(swaps.map((s) => (s.parseMs || 0) + (s.applyMs || 0) + s.handlerMs));

    console.log(`\n=== dom-bench [${LABEL}] mode=${MODE} entries=${ENTRIES} rows=${rows} throttle=${THROTTLE}x swaps=${swaps.length} ===`);
    console.log("phase        mean     p50     p90     max   (ms)");
    for (const [name, s] of [
      ["parse   ", parse],
      ["apply   ", apply],
      ["handlers", handler],
      ["TOTAL   ", total],
    ]) {
      console.log(`${name} ${fmt(s.mean)} ${fmt(s.p50)} ${fmt(s.p90)} ${fmt(s.max)}`);
    }
    console.log(`payload  ${(stats(swaps.map((s) => s.bytes)).mean / 1024).toFixed(1)} KiB mean`);
    console.log(`JSON ${JSON.stringify({ label: LABEL, entries: ENTRIES, parse, apply, handler, total })}`);

    if (profiler) {
      const { profile } = await profiler.send("Profiler.stop");
      const byId = new Map(profile.nodes.map((n) => [n.id, n]));
      const parent = new Map();
      for (const n of profile.nodes) for (const c of n.children || []) parent.set(c, n.id);
      const totalSamples = profile.samples.length;
      const ms = (profile.endTime - profile.startTime) / 1000;
      const label = (n) => {
        const cf = n.callFrame;
        return `${cf.functionName || "(anonymous)"} @ ${(cf.url || "").split("/").pop()}:${cf.lineNumber + 1}`;
      };
      const isApp = (n) => (n.callFrame.url || "").includes("/static/js/");
      const self = new Map();
      const owner = new Map();
      for (const id of profile.samples) {
        const n = byId.get(id);
        if (!n) continue;
        self.set(label(n), (self.get(label(n)) || 0) + 1);
        // Attribute native frames (querySelector, replaceState…) to the nearest
        // app-level caller — self time alone cannot say which hook paid for it.
        let cur = n;
        while (cur && !isApp(cur)) cur = byId.get(parent.get(cur.id));
        const key = cur ? label(cur) : "(outside app js)";
        owner.set(key, (owner.get(key) || 0) + 1);
      }
      const table = (m, title) => {
        console.log(`\n--- ${title} (${totalSamples} samples over ${ms.toFixed(0)} ms wall) ---`);
        [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 18)
          .forEach(([k, c]) => console.log(`${((c / totalSamples) * 100).toFixed(1).padStart(5)}%  ${((c / totalSamples) * ms).toFixed(0).padStart(5)} ms  ${k}`));
      };
      table(self, "CPU self time");
      table(owner, "CPU time by nearest app-level frame");
    }
  } finally {
    if (browser) await browser.close();
    await server.cleanup();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
