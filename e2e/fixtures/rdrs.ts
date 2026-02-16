import { test as base } from "@playwright/test";
import { ChildProcess, spawn } from "child_process";
import { mkdtempSync, rmSync } from "fs";
import http from "http";
import { tmpdir } from "os";
import path from "path";
import net from "net";
import { ApiHelper } from "../helpers/api.js";
import { SeedHelper } from "../helpers/seed.js";

const MOCK_RSS_FEED = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://localhost</link>
    <description>A test feed for E2E tests</description>
  </channel>
</rss>`;

/** Find an available TCP port. */
async function findAvailablePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (addr && typeof addr === "object") {
        const port = addr.port;
        server.close(() => resolve(port));
      } else {
        reject(new Error("Could not determine port"));
      }
    });
    server.on("error", reject);
  });
}

/** Wait until the server responds to /health. */
async function waitForServer(
  baseUrl: string,
  timeoutMs = 30_000
): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`${baseUrl}/health`);
      if (res.ok) return;
    } catch {
      // Server not ready yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Server did not become ready within ${timeoutMs}ms`);
}

interface WorkerFixtures {
  /** Base URL of the running server (e.g., http://127.0.0.1:12345) */
  serverUrl: string;
  /** Path to the SQLite database file */
  dbPath: string;
  /** API helper for interacting with the server */
  api: ApiHelper;
  /** Seed helper for writing entries directly to SQLite */
  seed: SeedHelper;
  /** Base URL of a mock feed server that serves valid RSS */
  feedServerUrl: string;
}

export const test = base.extend<{}, WorkerFixtures>({
  // Per-worker fixture: start a dedicated rdrs process
  serverUrl: [
    async ({}, use) => {
      const projectRoot = path.resolve(__dirname, "..", "..");
      const binaryPath = path.join(projectRoot, "target", "release", "rdrs");
      const tempDir = mkdtempSync(path.join(tmpdir(), "rdrs-e2e-"));
      const dbPath = path.join(tempDir, "test.sqlite3");
      const port = await findAvailablePort();
      const baseUrl = `http://127.0.0.1:${port}`;

      // Store dbPath so other fixtures can access it
      (globalThis as Record<string, unknown>)[`__rdrs_db_${port}`] = dbPath;
      (globalThis as Record<string, unknown>)[`__rdrs_tmpdir_${port}`] =
        tempDir;

      const serverProcess: ChildProcess = spawn(binaryPath, [], {
        cwd: projectRoot,
        env: {
          ...process.env,
          DATABASE_URL: dbPath,
          SERVER_PORT: String(port),
          SIGNUP_ENABLED: "true",
          MULTI_USER_ENABLED: "true",
          RUST_LOG: "warn",
        },
        stdio: "pipe",
      });

      // Log server stderr for debugging
      serverProcess.stderr?.on("data", (data: Buffer) => {
        if (process.env.DEBUG) {
          process.stderr.write(`[rdrs:${port}] ${data}`);
        }
      });

      try {
        await waitForServer(baseUrl);
        await use(baseUrl);
      } finally {
        serverProcess.kill("SIGTERM");
        await new Promise<void>((resolve) => {
          serverProcess.on("close", () => resolve());
          setTimeout(resolve, 5_000);
        });
        rmSync(tempDir, { recursive: true, force: true });
      }
    },
    { scope: "worker" },
  ],

  dbPath: [
    async ({ serverUrl }, use) => {
      const port = new URL(serverUrl).port;
      const dbPath = (globalThis as Record<string, unknown>)[
        `__rdrs_db_${port}`
      ] as string;
      await use(dbPath);
    },
    { scope: "worker" },
  ],

  api: [
    async ({ serverUrl }, use) => {
      await use(new ApiHelper(serverUrl));
    },
    { scope: "worker" },
  ],

  seed: [
    async ({ dbPath }, use) => {
      await use(new SeedHelper(dbPath));
    },
    { scope: "worker" },
  ],

  feedServerUrl: [
    async ({}, use) => {
      const server = http.createServer((_req, res) => {
        res.writeHead(200, { "Content-Type": "application/rss+xml" });
        res.end(MOCK_RSS_FEED);
      });
      const port = await findAvailablePort();
      server.listen(port, "127.0.0.1");
      try {
        await use(`http://127.0.0.1:${port}`);
      } finally {
        server.close();
      }
    },
    { scope: "worker" },
  ],
});

export { expect } from "@playwright/test";
