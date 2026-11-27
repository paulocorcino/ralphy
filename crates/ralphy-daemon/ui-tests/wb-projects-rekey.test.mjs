// Unit tests for assets/ui/wb-projects-rekey.ts: the open project after a
// read of the local projects, and the store's `hasOpen`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { openAfterRead } from "../assets/ui/wb-projects-rekey.ts";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";
import { loadShell } from "./harness.mjs";

const repo = (slug, former_slugs) => ({ slug, former_slugs });

test("the open project follows a re-key to its new slug", () => {
  const repos = [repo("owner/other"), repo("owner/repo", ["path-3f2a"])];
  assert.equal(openAfterRead("path-3f2a", repos), "owner/repo");
});

test("the open project stays when its row is still listed", () => {
  assert.equal(openAfterRead("owner/repo", [repo("owner/repo")]), "owner/repo");
});

test("the open project closes a project that is gone", () => {
  assert.equal(openAfterRead("path-3f2a", [repo("owner/repo")]), null);
});

test("an open peer project is left for the fleet read", () => {
  const ref = "01KYZ8Q3W4E5R6T7Y8V9J0P1A2/owner/repo";
  assert.equal(openAfterRead(ref, []), ref);
});

test("hasOpen is false while the open slug names no row", () => {
  const store = projectsStore();
  store.setProjects([{ slug: "owner/repo" }]);
  store.setOpen("path-3f2a");
  assert.equal(store.hasOpen(), false);
  store.setOpen("owner/repo");
  assert.equal(store.hasOpen(), true);
});

test("a read that re-keys the open project sends workbench:project-changed", async () => {
  const sent = [];
  const { state } = loadShell({
    window: {
      addEventListener: () => {},
      dispatchEvent: (e) => {
        if (e.type === "workbench:project-changed") sent.push(e.detail);
        return true;
      },
    },
  });
  const realFetch = globalThis.fetch;
  globalThis.fetch = (url) => {
    const rows = String(url).endsWith("/api/repos")
      ? [{ slug: "owner/repo", former_slugs: ["path-3f2a"], reachable: true }]
      : [];
    return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(rows) });
  };
  try {
    state.$store.projects.setOpen("path-3f2a");
    await state.$store.projects.loadRepos({ git: false });
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(state.$store.projects.openSlug, "owner/repo");
  assert.deepEqual(sent, [{ slug: "owner/repo", previous: "path-3f2a" }]);
});
