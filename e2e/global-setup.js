import { execSync } from "child_process";
import { existsSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export default function globalSetup() {
  const projectRoot = path.resolve(__dirname, "..");
  const binaryPath = path.join(projectRoot, "target", "debug", "rdrs");
  if (!existsSync(binaryPath)) {
    console.log("Building rdrs binary (debug mode)...");
    execSync("cargo build", { cwd: projectRoot, stdio: "inherit" });
  } else {
    console.log("rdrs binary already exists, skipping build.");
  }
}
