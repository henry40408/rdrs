import { spawn } from "child_process";
import { mkdtempSync, rmSync, existsSync } from "fs";
import http from "http";
import { tmpdir } from "os";
import path from "path";
import net from "net";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const MOCK_RSS_FEED = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://localhost</link>
    <description>A test feed for E2E tests</description>
  </channel>
</rss>`;

export function findAvailablePort() {
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

async function waitForServer(baseUrl, timeoutMs = 30_000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`${baseUrl}/health`);
      if (res.ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Server did not become ready within ${timeoutMs}ms`);
}

export async function spawnRdrs() {
  const projectRoot = path.resolve(__dirname, "..", "..");
  const binaryPath = path.join(projectRoot, "target", "debug", "rdrs");
  if (!existsSync(binaryPath)) {
    throw new Error(`rdrs binary not found at ${binaryPath} — run cargo build first`);
  }
  const tempDir = mkdtempSync(path.join(tmpdir(), "rdrs-e2e-"));
  const dbPath = path.join(tempDir, "test.sqlite3");
  const port = await findAvailablePort();
  const baseUrl = `http://127.0.0.1:${port}`;

  const proc = spawn(binaryPath, [], {
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
  proc.stderr?.on("data", (data) => {
    if (process.env.DEBUG) process.stderr.write(`[rdrs:${port}] ${data}`);
  });

  await waitForServer(baseUrl);
  return {
    url: baseUrl,
    dbPath,
    cleanup: async () => {
      proc.kill("SIGTERM");
      await new Promise((resolve) => {
        proc.on("close", () => resolve());
        setTimeout(resolve, 5_000);
      });
      rmSync(tempDir, { recursive: true, force: true });
    },
  };
}

export async function spawnMockFeedServer() {
  const server = http.createServer((_req, res) => {
    res.writeHead(200, { "Content-Type": "application/rss+xml" });
    res.end(MOCK_RSS_FEED);
  });
  const port = await findAvailablePort();
  server.listen(port, "127.0.0.1");
  return {
    url: `http://127.0.0.1:${port}`,
    cleanup: async () => {
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
