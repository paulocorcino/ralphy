// Unit tests for assets/ui/wb-viewer-html.ts — the document and the frame of
// an HTML file in the rendered pane. The DOM is a fake; the three browser
// engines are checked by tests/browser/files/wb_html_file.py.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  frameDocument,
  frameSource,
  mountFrame,
  remoteImagesAllowed,
} from "../assets/ui/wb-viewer-html.ts";

const XLINK = "http://www.w3.org/1999/xlink";

function element(tag, attrs = {}) {
  const held = new Map(Object.entries(attrs));
  return {
    tag,
    className: "",
    title: "",
    getAttribute: (name) => held.get(name) ?? null,
    setAttribute: (name, value) => held.set(name, value),
    removeAttribute: (name) => held.delete(name),
    removeAttributeNS: (ns, name) => ns === XLINK && held.delete(`xlink:${name}`),
    attrs: () => Object.fromEntries(held),
  };
}

// A parsed document with `first` already in its head, and `body` as its elements.
function parserOf(first, body) {
  const head = {
    children: [first],
    prepend: (el) => head.children.unshift(el),
    append: (el) => head.children.push(el),
  };
  const parsed = {
    head,
    createElement: (tag) => element(tag),
    querySelectorAll: (selector) => {
      const tags = selector.split(", ");
      return body.filter((el) => tags.includes(el.tag));
    },
  };
  return { parseFromString: (_text, type) => (type === "text/html" ? parsed : null) };
}

test("the policy of the document takes web images only when the daemon allows them", () => {
  // [the daemon allows web images, the policy]
  const rows = [
    [false, "default-src 'none'; style-src 'unsafe-inline'; img-src data:"],
    [true, "default-src 'none'; style-src 'unsafe-inline'; img-src data: https:"],
  ];
  for (const [remote, want] of rows) {
    const parsed = frameDocument("", parserOf(element("title"), []), remote);
    assert.equal(parsed.head.children[0].attrs().content, want, String(remote));
  }
});

// A reply of `GET /api/security/state`.
function stateReply(ok, remote) {
  return { ok, status: ok ? 200 : 401, json: () => Promise.resolve({ remote_images: remote }) };
}

test("the daemon's answer says if web images are allowed, and no answer is a no", async () => {
  // [what the request does, web images allowed]
  const rows = [
    [() => Promise.resolve(stateReply(true, true)), true],
    [() => Promise.resolve(stateReply(true, false)), false],
    [() => Promise.resolve(stateReply(false, true)), false],
    [() => Promise.reject(new TypeError("no network")), false],
  ];
  const real = globalThis.fetch;
  try {
    for (const [answer, want] of rows) {
      const asked = [];
      globalThis.fetch = (url, init) => {
        asked.push([init.method, url]);
        return answer();
      };
      assert.equal(await remoteImagesAllowed(), want);
      assert.deepEqual(asked, [["GET", "/api/security/state"]]);
    }
  } finally {
    globalThis.fetch = real;
  }
});

test("the policy is the first element of the head of the document", () => {
  const style = element("style");
  const parsed = frameDocument("<style></style>", parserOf(style, []), false);
  assert.equal(parsed.head.children.length, 2);
  assert.equal(parsed.head.children[1], style);
  assert.equal(parsed.head.children[0].tag, "meta");
  assert.deepEqual(parsed.head.children[0].attrs(), {
    "http-equiv": "Content-Security-Policy",
    content: "default-src 'none'; style-src 'unsafe-inline'; img-src data:",
  });
});

test("a link to a fragment stays on the frame, and any other link has no target", () => {
  // [tag, attributes in the file, attributes in the frame]
  const rows = [
    ["a", { href: "#part" }, { href: "about:srcdoc#part" }],
    ["area", { href: "#map" }, { href: "about:srcdoc#map" }],
    ["a", { href: "https://example.com/x" }, {}],
    ["a", { href: "/api/session" }, {}],
    ["a", { href: "other.html#part" }, {}],
    ["a", { href: "" }, {}],
    ["area", { href: "http://example.com" }, {}],
    ["a", { "xlink:href": "https://example.com/x" }, {}],
    ["a", { href: "#part", "xlink:href": "/api/session" }, { href: "about:srcdoc#part" }],
    ["a", { name: "part" }, { name: "part" }],
    // Control: an element that is not a link keeps what it has.
    ["p", { href: "https://example.com/x" }, { href: "https://example.com/x" }],
  ];
  const body = rows.map(([tag, before]) => element(tag, before));
  frameDocument("", parserOf(element("title"), body), false);
  rows.forEach(([tag, before, want], i) => {
    assert.deepEqual(body[i].attrs(), want, `${tag} ${JSON.stringify(before)}`);
  });
});

test("the text of the frame keeps the doctype of the file, and adds none", () => {
  const documentElement = { outerHTML: "<html><head></head><body>x</body></html>" };
  // [the file has a doctype, the text of the frame]
  const rows = [
    [{ name: "html" }, "<!doctype html><html><head></head><body>x</body></html>"],
    [null, "<html><head></head><body>x</body></html>"],
  ];
  for (const [doctype, want] of rows) {
    assert.equal(frameSource({ doctype, documentElement }), want);
  }
});

test("the frame replaces the body of the article and has an empty sandbox", () => {
  const appended = [];
  const article = { innerHTML: "<p>old</p>", append: (el) => appended.push(el) };
  const frame = mountFrame(article, { createElement: (tag) => element(tag) }, "");
  // oxlint-disable-next-line no-restricted-properties -- reads a fake element; the rule is for writes
  assert.equal(article.innerHTML, "");
  assert.deepEqual(appended, [frame]);
  assert.equal(frame.tag, "iframe");
  assert.equal(frame.className, "html-frame");
  assert.deepEqual(frame.attrs(), { sandbox: "" });
});
