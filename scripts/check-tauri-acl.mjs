import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const ROOT = process.cwd();

const [buildSource, mainSource, apiSource, onboardingCapability, overlayCapability] =
  await Promise.all([
    readFile(join(ROOT, "src-tauri", "build.rs"), "utf8"),
    readFile(join(ROOT, "src-tauri", "src", "main.rs"), "utf8"),
    readFile(join(ROOT, "src", "lib", "api.ts"), "utf8"),
    readFile(join(ROOT, "src-tauri", "capabilities", "default.json"), "utf8").then(JSON.parse),
    readFile(join(ROOT, "src-tauri", "capabilities", "overlay.json"), "utf8").then(JSON.parse),
  ]);

function uniqueSorted(values, label) {
  const unique = new Set(values);
  assert.equal(unique.size, values.length, `${label} contains duplicate command entries`);
  return [...unique].sort();
}

function applicationPermissions(capability) {
  return capability.permissions
    .filter((permission) => typeof permission === "string" && permission.startsWith("allow-"))
    .map((permission) => permission.slice("allow-".length).replaceAll("-", "_"));
}

const buildBlock = buildSource.match(
  /const COMMANDS:\s*&\[&str\]\s*=\s*&\[(?<body>[\s\S]*?)\];/
);
assert.ok(buildBlock?.groups?.body, "src-tauri/build.rs COMMANDS manifest was not found");
const buildCommands = uniqueSorted(
  [...buildBlock.groups.body.matchAll(/"([a-z0-9_]+)"/g)].map((match) => match[1]),
  "build command manifest"
);

const handlerBlock = mainSource.match(
  /\.invoke_handler\(tauri::generate_handler!\[(?<body>[\s\S]*?)\]\)/
);
assert.ok(handlerBlock?.groups?.body, "Tauri generate_handler! command list was not found");
const handlerCommands = uniqueSorted(
  [...handlerBlock.groups.body.matchAll(/commands::[a-z0-9_]+::([a-z0-9_]+)/g)].map(
    (match) => match[1]
  ),
  "Tauri invoke handler"
);

const frontendCommands = uniqueSorted(
  [...apiSource.matchAll(/invoke(?:<[^>]+>)?\(\s*"([a-z0-9_]+)"/g)].map(
    (match) => match[1]
  ),
  "frontend invoke API"
);

assert.deepEqual(
  handlerCommands,
  buildCommands,
  "Tauri invoke handler must exactly match the build-time application command manifest"
);
assert.deepEqual(
  frontendCommands,
  buildCommands,
  "frontend invoke API must exactly match the registered application command surface"
);

assert.deepEqual(onboardingCapability.windows, ["onboarding"]);
assert.deepEqual(overlayCapability.windows, ["overlay"]);

const onboardingCommands = uniqueSorted(
  applicationPermissions(onboardingCapability),
  "onboarding capability"
);
const overlayCommands = uniqueSorted(
  applicationPermissions(overlayCapability),
  "overlay capability"
);
const expectedOnboarding = buildCommands.filter(
  (command) => command !== "get_recording_state"
);

assert.deepEqual(
  onboardingCommands,
  expectedOnboarding,
  "onboarding capability must grant every registered Settings command except overlay-only recording state"
);
assert.deepEqual(
  overlayCommands,
  ["get_recording_state"],
  "overlay capability must grant only read-only recording state"
);
assert.deepEqual(
  uniqueSorted([...onboardingCommands, ...overlayCommands], "combined application capabilities"),
  buildCommands,
  "application capabilities must cover every registered command exactly through the intended window split"
);

console.log(
  `Tauri command/ACL architecture check passed (${buildCommands.length} registered commands).`
);
