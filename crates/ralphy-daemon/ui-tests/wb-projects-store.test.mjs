// Unit tests for assets/ui/wb-projects-store.ts, the Alpine store `projects`:
// the open project and the list of projects. It is built here without Alpine
// (ADR-0073 D3, D6), as `main.ts` builds it for `Alpine.store`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";

test("a new store has no open project and no projects", () => {
  const store = projectsStore();
  assert.equal(store.openSlug, null);
  assert.deepEqual(store.projects, []);
  // Each page has its own list.
  assert.notEqual(projectsStore().projects, store.projects);
});

test("setOpen and setProjects are the writers, and the open row is found by its ref", () => {
  const store = projectsStore();
  const local = { slug: "owner/repo", branch: "main" };
  const peer = { key: "01KY/owner/repo", slug: "owner/repo", daemon: "01KY", branch: "dev" };
  store.setProjects([local, peer]);
  assert.equal(store.openProject(), null);
  assert.equal(store.openProjectBranch(), "current");
  // The same slug on a peer is another row.
  store.setOpen(store.repoRef(peer));
  assert.equal(store.openProject(), peer);
  assert.equal(store.openProjectBranch(), "dev");
  assert.equal(store.rowOpen(local), false);
  assert.equal(store.rowOpen(peer), true);
  store.setOpen(null);
  assert.equal(store.openSlug, null);
});
