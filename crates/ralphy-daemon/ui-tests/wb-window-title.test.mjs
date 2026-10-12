// Unit tests for assets/ui/wb-window-title.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { windowTitle } from "../assets/ui/wb-window-title.ts";

const manifest = JSON.parse(readFileSync(new URL("../assets/ui/manifest.webmanifest", import.meta.url), "utf8"));

test("a torn-off window's title starts with the installed app name and names what it shows", () => {
  const title = windowTitle("src/main.rs");
  assert.ok(title.startsWith(manifest.name), `${JSON.stringify(title)} must start with ${JSON.stringify(manifest.name)}`);
  assert.ok(title.endsWith("src/main.rs"), title);
});
