// Unit tests for assets/ui/wb-console-strips.ts: a strip is inserted before
// the window's current body, also after `sleepWindow` replaced the body node.
import { test } from "node:test";
import assert from "node:assert/strict";
import { showPeerDown, showLinkLost } from "../assets/ui/wb-console-strips.ts";

// A node with the DOM members the strips use. `insertBefore` throws like the
// browser when the reference is not a child.
function fakeElement(tag = "div") {
  const el = {
    tagName: tag,
    className: "",
    textContent: "",
    hidden: false,
    dataset: {},
    children: [],
    parent: null,
    append(...kids) {
      for (const k of kids) {
        k.parent = el;
        el.children.push(k);
      }
    },
    insertBefore(node, ref) {
      const i = el.children.indexOf(ref);
      if (i < 0) throw new Error("NotFoundError: the reference is not a child");
      node.parent = el;
      el.children.splice(i, 0, node);
    },
    addEventListener() {},
    remove() {
      if (el.parent) el.parent.children.splice(el.parent.children.indexOf(el), 1);
    },
    querySelector(sel) {
      const match = (n) =>
        sel.startsWith(".") ? n.className.split(" ").includes(sel.slice(1)) : n.tagName === sel;
      for (const k of el.children) {
        if (match(k)) return k;
        const deeper = k.querySelector(sel);
        if (deeper) return deeper;
      }
      return null;
    },
  };
  return el;
}

const document = { createElement: (tag) => fakeElement(tag) };

// A window whose body node was replaced after the strip code first saw it.
function windowWithReplacedBody() {
  const win = fakeElement();
  const body = fakeElement();
  body.className = "session-body";
  win.append(body);
  body.remove();
  const live = fakeElement();
  live.className = "session-body";
  win.append(live);
  return { win, live };
}

test("showPeerDown uses the live body", () => {
  const { win, live } = windowWithReplacedBody();
  const deps = { document, peerDaemon: "peer", wakePeerNow: () => null, readFleetNow: () => null };
  showPeerDown(deps, win, null);
  const at = win.children.indexOf(live);
  assert.equal(win.children[at - 1]?.className, "session-peer-down");
});

test("showLinkLost uses the live body", () => {
  const { win, live } = windowWithReplacedBody();
  showLinkLost(document, win);
  const at = win.children.indexOf(live);
  assert.equal(win.children[at - 1]?.className, "session-link-lost");
});
