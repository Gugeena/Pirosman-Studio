// Run a build script for this platform: scripts/<name>.ps1 on Windows,
// scripts/<name>.sh elsewhere. Used by `npm run fetch-ffmpeg` and `npm run build:ofx`.
// Usage: node scripts/run-platform.mjs <name> [args...]
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const [name, ...args] = process.argv.slice(2);
const dir = dirname(fileURLToPath(import.meta.url));
const [cmd, cmdArgs] = process.platform === "win32"
  ? ["powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(dir, `${name}.ps1`), ...args]]
  : ["bash", [join(dir, `${name}.sh`), ...args]];
const r = spawnSync(cmd, cmdArgs, { stdio: "inherit" });
if (r.error) console.error(`run-platform: could not start ${cmd}: ${r.error.message}`);
process.exit(r.status ?? 1);
