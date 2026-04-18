import { test } from "../fixtures/rdrs.js";
import fs from "fs";
import path from "path";
import * as csstree from "css-tree";

interface Range {
  start: number;
  end: number;
}

interface RuleInfo {
  selector: string;
  context: string | null;
  start: number;
  end: number;
}

interface Report {
  stylesheet: string;
  textLength: number;
  totalRules: number;
  coveredRules: number;
  coveredBytes: number;
  uncoveredRules: { selector: string; context: string | null }[];
  caveats: string[];
}

/**
 * Walk representative user flows with Playwright's CSS Coverage API enabled,
 * then diff against the parsed rule set from static/css/app.css to produce
 * a list of rules that were never applied.
 *
 * Run via:  npm run coverage:css
 *
 * Playwright's `resetOnNavigation: false` doesn't reliably accumulate ranges
 * across navigations (stylesheet IDs churn on each load), so we start+stop
 * coverage per page and merge the resulting ranges manually.
 */
test(
  "walk flows and report unused CSS rules",
  { tag: "@coverage" },
  async ({ page, serverUrl, api, seed }) => {
    test.setTimeout(180_000);

    // ── Seed ───────────────────────────────────────────────────────────
    await api.register("cssuser", "password123");
    const userId = seed.getUserId("cssuser");
    const categoryId = seed.createCategory(userId, "Coverage");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/coverage.xml",
      "Coverage Feed"
    );
    const entryIds = seed.seedTestEntries(feedId, 5);

    const accumulator: {
      text: string | null;
      url: string | null;
      ranges: Range[];
    } = { text: null, url: null, ranges: [] };

    async function samplePage(fn: () => Promise<void>): Promise<void> {
      await page.coverage.startCSSCoverage();
      await fn();
      await page.waitForLoadState("domcontentloaded");
      // Force layout so CSS rules get applied before sampling
      await page.evaluate(() => document.body.getBoundingClientRect());
      const entries = await page.coverage.stopCSSCoverage();
      const hit = entries.find((e) => e.url.includes("/static/css/app.css"));
      if (hit && hit.text) {
        accumulator.text ??= hit.text;
        accumulator.url ??= hit.url;
        accumulator.ranges.push(...hit.ranges);
      }
    }

    // ── Anonymous pages (login + register layouts) ─────────────────────
    await samplePage(async () => {
      await page.goto(`${serverUrl}/login`);
    });
    await samplePage(async () => {
      await page.goto(`${serverUrl}/register`);
    });

    // ── Login once (fills session for subsequent requests) ─────────────
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("cssuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    // ── Authenticated desktop routes ───────────────────────────────────
    const routes = [
      "/",
      "/entries",
      "/entries/read",
      "/entries/starred",
      "/entries/summarized",
      "/search",
      "/feeds",
      "/categories",
      "/statistics",
      "/user-settings",
      "/settings",
      `/entries/${entryIds[0]}`,
    ];
    for (const route of routes) {
      await samplePage(async () => {
        await page.goto(`${serverUrl}${route}`);
      });
    }

    // ── Split-view: open an entry in the reading pane ──────────────────
    await samplePage(async () => {
      await page.goto(`${serverUrl}/`);
      const firstItem = page.getByTestId("entry-item").first();
      if (await firstItem.isVisible().catch(() => false)) {
        await firstItem.click();
        await page.waitForTimeout(300);
      }
    });

    // ── Keyboard-help modal ────────────────────────────────────────────
    await samplePage(async () => {
      await page.goto(`${serverUrl}/`);
      await page.keyboard.press("Shift+/");
      await page.waitForTimeout(300);
      await page.keyboard.press("Escape");
    });

    // ── Dark theme ─────────────────────────────────────────────────────
    await samplePage(async () => {
      await page.goto(`${serverUrl}/`);
      await page.evaluate(() =>
        document.documentElement.setAttribute("data-theme", "dark")
      );
      // Force recompute
      await page.evaluate(() => document.body.getBoundingClientRect());
    });
    // Reset theme
    await page.evaluate(() =>
      document.documentElement.removeAttribute("data-theme")
    );

    // ── Tablet viewport (< 1024px: sidebar collapses) ──────────────────
    await page.setViewportSize({ width: 900, height: 800 });
    await samplePage(async () => {
      await page.goto(`${serverUrl}/`);
      const toggle = page.locator(".sidebar-toggle");
      if (await toggle.isVisible().catch(() => false)) {
        await toggle.click();
        await page.waitForTimeout(200);
        const closeBtn = page.locator(".sidebar-close");
        if (await closeBtn.isVisible().catch(() => false)) {
          await closeBtn.click();
        }
      }
    });

    // ── Phone viewport (< 600px: card-table mode) ──────────────────────
    await page.setViewportSize({ width: 375, height: 667 });
    await samplePage(async () => {
      await page.goto(`${serverUrl}/feeds`);
    });
    await samplePage(async () => {
      await page.goto(`${serverUrl}/settings`);
    });

    // ── Wide viewport (≥ 1600px) ───────────────────────────────────────
    await page.setViewportSize({ width: 1920, height: 1080 });
    await samplePage(async () => {
      await page.goto(`${serverUrl}/`);
    });

    // ── Analyze ────────────────────────────────────────────────────────
    if (!accumulator.text || !accumulator.url) {
      throw new Error(
        "static/css/app.css was not reported by CSSCoverage on any page"
      );
    }

    const report = analyze(
      accumulator.text,
      accumulator.ranges,
      accumulator.url
    );

    const reportDir = path.resolve(__dirname, "..", "test-results");
    fs.mkdirSync(reportDir, { recursive: true });
    const reportPath = path.join(reportDir, "css-coverage.json");
    fs.writeFileSync(reportPath, JSON.stringify(report, null, 2));

    printSummary(report, reportPath);
  }
);

function analyze(cssText: string, ranges: Range[], url: string): Report {
  const merged = mergeRanges(ranges);

  const hasCoverage = (start: number, end: number): boolean => {
    for (const m of merged) {
      if (m.end <= start) continue;
      if (m.start >= end) return false;
      return true;
    }
    return false;
  };

  const ast = csstree.parse(cssText, { positions: true });
  const rules: RuleInfo[] = [];
  const atRuleStack: string[] = [];

  csstree.walk(ast, {
    enter(node: csstree.CssNode) {
      if (node.type === "Atrule") {
        const prelude = node.prelude
          ? csstree.generate(node.prelude)
          : "";
        atRuleStack.push(prelude ? `@${node.name} ${prelude}` : `@${node.name}`);
      } else if (node.type === "Rule" && node.loc) {
        rules.push({
          selector: csstree.generate(node.prelude),
          context: atRuleStack[atRuleStack.length - 1] ?? null,
          start: node.loc.start.offset,
          end: node.loc.end.offset,
        });
      }
    },
    leave(node: csstree.CssNode) {
      if (node.type === "Atrule") atRuleStack.pop();
    },
  });

  const uncovered = rules
    .filter((r) => !hasCoverage(r.start, r.end))
    .map(({ selector, context }) => ({ selector, context }));

  const coveredBytes = merged.reduce((acc, r) => acc + (r.end - r.start), 0);

  return {
    stylesheet: url,
    textLength: cssText.length,
    totalRules: rules.length,
    coveredRules: rules.length - uncovered.length,
    coveredBytes,
    uncoveredRules: uncovered,
    caveats: [
      "hover/focus/active pseudo-state rules will show as uncovered unless explicitly triggered",
      "rules inside @media blocks are only covered if a matching viewport was tested",
      "rare states (errors, loading, masquerade banner, admin-only views) may be false positives",
    ],
  };
}

function mergeRanges(ranges: Range[]): Range[] {
  const sorted = [...ranges].sort((a, b) => a.start - b.start);
  const merged: Range[] = [];
  for (const r of sorted) {
    const last = merged[merged.length - 1];
    if (last && r.start <= last.end) {
      last.end = Math.max(last.end, r.end);
    } else {
      merged.push({ ...r });
    }
  }
  return merged;
}

function printSummary(report: Report, reportPath: string): void {
  const rulesPct = (
    (report.coveredRules / report.totalRules) * 100
  ).toFixed(1);
  const bytesPct = (
    (report.coveredBytes / report.textLength) * 100
  ).toFixed(1);
  const lines = [
    "",
    "CSS Coverage Report",
    "===================",
    `Stylesheet:   ${report.stylesheet}`,
    `Total rules:  ${report.totalRules}`,
    `Covered:      ${report.coveredRules} (${rulesPct}%)`,
    `Uncovered:    ${report.uncoveredRules.length}`,
    `Byte coverage: ${report.coveredBytes} / ${report.textLength} (${bytesPct}%)`,
    "",
    "Uncovered selectors (likely unused — verify before deleting):",
    "",
  ];
  const preview = report.uncoveredRules.slice(0, 60);
  for (const r of preview) {
    const ctx = r.context ? `  [${r.context}]` : "";
    lines.push(`  ${r.selector}${ctx}`);
  }
  if (report.uncoveredRules.length > preview.length) {
    const extra = report.uncoveredRules.length - preview.length;
    lines.push(`  ... and ${extra} more`);
  }
  lines.push("");
  lines.push(`Full report: ${reportPath}`);
  lines.push("");
  lines.push("Caveats:");
  for (const c of report.caveats) lines.push(`  - ${c}`);
  lines.push("");

  console.log(lines.join("\n"));
}
