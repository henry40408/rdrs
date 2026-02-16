import { execSync } from "child_process";
import { existsSync } from "fs";
import path from "path";

export default function globalSetup() {
  const projectRoot = path.resolve(__dirname, "..");
  const binaryPath = path.join(projectRoot, "target", "release", "rdrs");

  if (!existsSync(binaryPath)) {
    console.log("Building rdrs binary (release mode)...");
    execSync("cargo build --release", {
      cwd: projectRoot,
      stdio: "inherit",
    });
  } else {
    console.log("rdrs binary already exists, skipping build.");
  }
}
