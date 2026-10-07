// The settings schema in `wb-settings.ts` and the way index.html renders it.
// The dialog's code (`saveSetting` and the rest) is tested in
// wb-settings-dialog.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { UI, withoutComments } from "./harness.mjs";
import { WB_SETTINGS } from "../assets/ui/wb-settings.ts";

// A password field is kept out of the DOM while its modal is closed (the
// browser's password manager pairs any `type="password"` in the document with
// the next text field typed into). The gate must be the flag of the modal the
// field sits in: gated on another modal's flag, the field never renders at all.
test("each modal's password fields render when THAT modal is open", () => {
  // Comments dropped first: their prose quotes tags like `<div>`.
  const html = withoutComments(readFileSync(join(UI, "index.html"), "utf8"));
  // The scrim's body runs to the `</div>` that closes it, found by depth.
  const scrimBody = (start) => {
    let depth = 0;
    for (const t of html.slice(start).matchAll(/<div[\s>]|<\/div>/g)) {
      depth += t[0] === "</div>" ? -1 : 1;
      if (depth === 0) return html.slice(start, start + t.index);
    }
    throw new Error(`the modal scrim at offset ${start} is never closed`);
  };
  let checked = 0;
  for (const m of html.matchAll(/<div class="modal-scrim" x-cloak x-bind="scrim\('([^']+)'/g)) {
    const flag = m[1];
    const body = scrimBody(m.index);
    for (const g of body.matchAll(/<template x-if="([^"]+)">\s*<input[^>]*type="password"/g)) {
      checked += 1;
      assert.ok(
        g[1].split("&&").map((t) => t.trim()).includes(flag),
        `a password field in the modal shown by \`${flag}\` is gated by \`${g[1]}\``,
      );
    }
  }
  assert.ok(checked >= 4, `expected the Settings and Security password fields, checked ${checked}`);
});

// The daemon refuses to write `queue.trust_all_comments` from a browser, so the
// row declares it read-only and the toggle that renders it is disabled: a
// checkbox that takes a click and answers "refused" is the failure.
test("the read-every-comment toggle is read-only and rendered disabled", () => {
  const item = WB_SETTINGS.flatMap((s) => s.items).find(
    (it) => it.key === "queue.trust_all_comments",
  );
  assert.equal(item.type, "toggle");
  assert.equal(item.readonly, true);
  assert.match(item.help, /ralphy config set queue\.trust_all_comments true/);
  const html = withoutComments(readFileSync(join(UI, "index.html"), "utf8"));
  const toggle = html.match(/<template x-if="it\.type === 'toggle'">\s*<input[^>]*>/);
  assert.ok(toggle, "the toggle template is gone");
  assert.match(toggle[0], /:disabled="it\.readonly === true"/);
});
