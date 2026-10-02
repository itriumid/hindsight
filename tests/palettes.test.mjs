// Checks the palettes as Hindsight uses them: @itrium/palettes's colors with Hindsight's own
// stylesheet on top, every accent line in Hindsight's components, and app.html applying the
// saved choice before the first paint. Run it with `pnpm test`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import {
  STYLESHEET_URL,
  accentLineProblems,
  paletteProblems,
  prePaintProblems,
} from "@itrium/palettes/check";

const source = new URL("../src/", import.meta.url);
const read = (path) => readFileSync(new URL(path, source), "utf8");

test("every palette meets level AA in Hindsight", () => {
  const css = readFileSync(STYLESHEET_URL, "utf8") + read("app.css");
  assert.deepEqual(paletteProblems(css), []);
});

test("accent lines use --accent-edge", () => {
  const files = readdirSync(source, { recursive: true })
    .filter((file) => file.endsWith(".svelte") || file.endsWith(".css"))
    .map((file) => ({ name: `src/${file}`, text: read(file) }));
  assert.deepEqual(accentLineProblems(files), []);
});

test("app.html applies the saved theme and palette before the first paint", () => {
  assert.deepEqual(prePaintProblems(read("app.html"), "hindsight"), []);
});
