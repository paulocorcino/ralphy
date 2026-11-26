import { test } from "node:test";
import assert from "node:assert/strict";
import { copyEntryPath } from "../assets/ui/wb-files-open.ts";

// A peer project's row: its `slug` is `owner/repo`, its ref
// `<daemon_id>/owner/repo`, so the open ref never equals the slug.
const peer = { slug: "o/r", daemon: "D1", root: "/home/u/r" };
const projects = { openProject: () => peer, projects: [peer], openSlug: "D1/o/r" };

// The other tests share this process: each global is put back.
function withGlobal(name, value, run) {
  const before = Object.getOwnPropertyDescriptor(globalThis, name);
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  try {
    return run();
  } finally {
    if (before) Object.defineProperty(globalThis, name, before);
    else delete globalThis[name];
  }
}

function copyOnce(full) {
  const written = [];
  const clipboard = { writeText: (t) => (written.push(t), Promise.resolve()) };
  const files = {
    $store: { projects },
    relPath: () => "src/a.ts",
    nodeDetail: () => ({}),
  };
  withGlobal("navigator", { clipboard }, () =>
    withGlobal("window", { WB: { emit() {} } }, () => copyEntryPath(files, {}, full)),
  );
  return written[0];
}

test("copy full path of a peer project joins its root; relative stays relative", () => {
  assert.equal(copyOnce(true), "/home/u/r/src/a.ts");
  assert.equal(copyOnce(false), "src/a.ts");
});
