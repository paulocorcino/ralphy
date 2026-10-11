// Unit tests for assets/ui/wb-branch-dialog.ts, the Alpine component of the
// Switch branch modal and the checkout menu. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// ---- the component and its markup ---------------------------------------------

test("wbBranchDialog lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbBranchDialog");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbBranchDialog reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbBranchDialog");
  const markup = componentMarkup(HTML, "wbBranchDialog");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variable of the branch rows.
  const loopVars = new Set(["b"]);
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component; `x-ref` names an element.
    if (attr === "x-data" || attr === "x-ref") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 15, `the markup was read: ${read} names`);
});

test("nothing outside the branch dialog element names its state", () => {
  const start = HTML.indexOf('x-data="wbBranchDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["branchModal", "openBranchModal", "closeBranchModal", "branchList", "canCreateBranch", "branchEnter", "switchBranch", "createBranch", "removeWorktree", "worktreeRemoving", "openCheckoutChip", "_mutateBranch"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

// One page with the shell, the files, the Changes view and the branch dialog,
// and a window that delivers the events it is sent, as the browser does.
function page() {
  const listeners = {};
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const dialog = loadComponent("wbBranchDialog", { from: loaded }).scope;
  const files = loadComponent("wbFiles", { from: loaded }).scope;
  const changes = loadComponent("wbChanges", { from: loaded }).scope;
  dialog.init();
  return { shell: loaded.state, dialog, files, changes };
}

test("the branch chip and the two checkout chips reach the branch dialog through events", () => {
  const { shell, dialog, files, changes } = page();
  const calls = [];
  dialog.openBranchModal = (p) => calls.push(["branch", p.slug]);
  dialog.openCheckoutChip = (p, anchor) => calls.push(["menu", p.slug, anchor]);
  const p = { slug: "o/r" };
  let stopped = 0;
  const click = { stopPropagation: () => stopped++ };
  // CONTROL: a collapsed row's chip is only the change count, and its click
  // falls through to the row.
  shell.branchChipClick(p, click);
  assert.deepEqual(calls, []);
  shell.$store.projects.setOpen("o/r");
  shell.branchChipClick(p, click);
  assert.equal(stopped, 1);
  files.checkoutChipClick(p, "files-anchor");
  changes.checkoutChipClick(p, "changes-anchor");
  assert.deepEqual(calls, [
    ["branch", "o/r"],
    ["menu", "o/r", "files-anchor"],
    ["menu", "o/r", "changes-anchor"],
  ]);
});
