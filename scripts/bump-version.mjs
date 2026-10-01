// Sets Hindsight's version everywhere it's recorded, so the four copies can't drift apart:
//
//   pnpm bump-version 0.2.0
//
// Versions are MAJOR.MINOR.PATCH only (see CONTRIBUTING.md, "Versioning"), and must be higher
// than the current one.
import { readFileSync, writeFileSync } from "node:fs";

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

const next = process.argv[2]?.replace(/^v/, "");
if (!next || !SEMVER.test(next)) {
  fail(
    `usage: pnpm bump-version MAJOR.MINOR.PATCH, e.g. 0.2.0` +
      (next ? `\n"${next}" isn't one. Prerelease and build suffixes aren't supported.` : ""),
  );
}

const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const current = packageJson.version;
if (compare(next, current) <= 0) fail(`${next} isn't higher than the current version, ${current}`);

// Every edit is worked out before anything is written, so a failure leaves all four untouched.
const edits = [
  jsonVersionEdit("package.json"),
  jsonVersionEdit("src-tauri/tauri.conf.json"),
  // The first `version` in Cargo.toml is the one under [package].
  replaceOnceEdit("src-tauri/Cargo.toml", /^version = "[^"]*"$/m, `version = "${next}"`),
  // Cargo.lock records the app's own version too; CI builds with --locked, so it must match.
  replaceOnceEdit(
    "Cargo.lock",
    /(\[\[package\]\]\nname = "hindsight"\nversion = ")[^"]*(")/,
    `$1${next}$2`,
  ),
];
for (const { path, text } of edits) writeFileSync(path, text);

console.log(`${current} → ${next}. Commit the four files, then follow "Releasing" in CONTRIBUTING.md.`);

function compare(left, right) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return 0;
}

// Edits the version in place instead of re-serializing the file, so everything else stays
// byte for byte: re-serializing turned tauri.conf.json's "\u00a9" into a literal "©".
function jsonVersionEdit(path) {
  const text = readFileSync(path, "utf8");
  const version = JSON.parse(text).version;
  const pattern = new RegExp(`"version": "${version.replaceAll(".", "\\.")}"`, "g");
  const matches = text.match(pattern)?.length ?? 0;
  if (matches !== 1) fail(`expected one "version": "${version}" in ${path}, found ${matches}`);
  const updated = text.replace(pattern, `"version": "${next}"`);
  if (JSON.parse(updated).version !== next) fail(`couldn't update the version in ${path}`);
  return { path, text: updated };
}

function replaceOnceEdit(path, pattern, replacement) {
  const text = readFileSync(path, "utf8");
  if (!pattern.test(text)) fail(`couldn't find the version in ${path}`);
  return { path, text: text.replace(pattern, replacement) };
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
