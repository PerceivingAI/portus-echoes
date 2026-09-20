import { readdir, readFile } from "node:fs/promises";
import { extname, join, relative } from "node:path";

const ROOT = process.cwd();
const FRONTEND_ROOT = join(ROOT, "src");

const ROUTING_SUBSTRINGS = [
  "429",
  "api key",
  "connect",
  "openai",
  "groq",
  "credential",
  "keyring",
  "rejected",
  "hotkey",
  "shortcut",
  "modifier",
  "setwindowshookex",
  "registerhotkey",
  "model file not found",
  "local model not found",
  "no model path is set",
  "not enough ram",
  "memory",
  "not a supported whisper",
  "unsupported model",
  "invalid model format",
  "magic",
  "download",
  "https url",
  "storage limit",
  "partial download",
  "network",
  "dns",
  "offline",
  "reqwest",
  "rate limit",
  "too large",
  "413",
  "25 mb limit",
  "cloud",
  "transcription failed",
  "save settings",
  "settings.toml",
];

const escaped = ROUTING_SUBSTRINGS.map((value) =>
  value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
).join("|");
const substringRouting = new RegExp(
  String.raw`\.includes\(\s*["'](?:${escaped})["']\s*\)`,
  "i"
);

const violations = [];

async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      await walk(path);
      continue;
    }

    if (![".ts", ".tsx"].includes(extname(entry.name))) continue;
    if (/\.test\.[cm]?[jt]sx?$/.test(entry.name)) continue;

    const source = await readFile(path, "utf8");
    const rel = relative(ROOT, path).replaceAll("\\", "/");

    if (source.includes("mapErrorMessage(")) {
      violations.push(`${rel}: mapErrorMessage() is forbidden in production code`);
    }
    if (source.includes("setError(String(")) {
      violations.push(`${rel}: raw String(error) UI routing is forbidden`);
    }
    if (substringRouting.test(source)) {
      violations.push(`${rel}: substring-based error routing is forbidden`);
    }
  }
}

await walk(FRONTEND_ROOT);

if (violations.length > 0) {
  console.error(violations.join("\n"));
  process.exit(1);
}

console.log("Error-routing architecture check passed.");
