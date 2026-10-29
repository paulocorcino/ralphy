// Unit tests for assets/ui/wb-notes-links.ts — what a click on a link inside a
// note card does. `installLinks` is driven with a fake card, a fake anchor and
// a fake document, with no editor and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { installLinks } from "../assets/ui/wb-notes-links.ts";
import { linkTarget } from "../assets/ui/wb-viewer-markdown.ts";

// A card with one anchor, clicked. `viewer` is the page's `WBViewer`; the
// answer is what the click did: whether the browser's own navigation was
// stopped, the anchor's target and rel, and the events the document got.
function click(href, { viewer, record = null, outside = false } = {}) {
  const sent = [];
  const anchor = { getAttribute: (name) => (name === "href" ? href : null), target: "", rel: "" };
  const handlers = {};
  const card = {
    dataset: { noteId: "n1" },
    contains: (a) => a === anchor && !outside,
    addEventListener: (type, fn) => (handlers[type] = fn),
  };
  const document = { dispatchEvent: (ev) => sent.push({ type: ev.type, detail: ev.detail }) };
  const asked = [];
  installLinks({
    window: { WBViewer: viewer },
    document,
    recordOf: (id) => {
      asked.push(id);
      return record;
    },
  })(card);
  let stopped = 0;
  handlers.click({ target: { closest: () => anchor }, preventDefault: () => stopped++ });
  return { stopped, anchor, sent, asked };
}

const VIEWER = { linkTarget };

test("a link with a script scheme is stopped and gets no target", () => {
  for (const href of ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,x", "vbscript:x"]) {
    const done = click(href, { viewer: VIEWER });
    assert.equal(done.stopped, 1, href);
    assert.equal(done.anchor.target, "", href);
    assert.deepEqual(done.sent, [], href);
  }
});

test("a link to https or mailto opens in a new tab without the opener", () => {
  for (const href of ["https://example.com/a", "http://example.com", "mailto:a@example.com"]) {
    const done = click(href, { viewer: VIEWER });
    assert.equal(done.stopped, 0, `${href}: the browser follows the link`);
    assert.equal(done.anchor.target, "_blank", href);
    assert.equal(done.anchor.rel, "noopener", href);
    assert.deepEqual(done.sent, [], href);
  }
});

test("a link to a path in the repo sends an open request and stops the navigation", () => {
  const record = { id: "n1", repo: "owner/repo", checkout: "wt" };
  const done = click("docs/a.md#top", { viewer: VIEWER, record });
  assert.equal(done.stopped, 1);
  assert.equal(done.anchor.target, "");
  assert.deepEqual(done.asked, ["n1"]);
  assert.deepEqual(done.sent, [
    {
      type: "workbench:open-request",
      detail: { project: "owner/repo", path: "docs/a.md", fragment: "top", checkout: "wt" },
    },
  ]);
});

test("a link to a path from a note with no record names no project and no checkout", () => {
  const done = click("docs/a.md", { viewer: VIEWER });
  assert.deepEqual(done.sent[0].detail, { project: null, path: "docs/a.md", fragment: "", checkout: null });
});

test("a link inside the page is stopped and sends nothing", () => {
  const done = click("#section", { viewer: VIEWER });
  assert.equal(done.stopped, 1);
  assert.deepEqual(done.sent, []);
});

test("a page with no viewer reads a scheme link the same way and refuses a path", () => {
  assert.equal(click("javascript:alert(1)").stopped, 1);
  assert.equal(click("https://example.com").anchor.target, "_blank");
  const relative = click("docs/a.md");
  assert.equal(relative.stopped, 1, "a relative path is not a link without the viewer's rule");
  assert.deepEqual(relative.sent, []);
});

test("a click on an anchor that is not inside this card is left alone", () => {
  const done = click("javascript:alert(1)", { viewer: VIEWER, outside: true });
  assert.equal(done.stopped, 0);
  assert.deepEqual(done.sent, []);
});
