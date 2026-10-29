// Unit tests for assets/opencode/ralphy-state-plugin.js, the plugin that
// OpenCode loads in a Ralphy console or run and that appends one status line
// per state event to $RALPHY_STATUS_FILE. The plugin is not workbench code;
// its tests are here because this is the node test runner CI runs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PLUGIN = join(HERE, "..", "..", "..", "assets", "opencode", "ralphy-state-plugin.js");

// A fresh copy of the module each time: the query makes Node load it again.
let loads = 0;
async function plugin() {
  loads += 1;
  const mod = await import(`${pathToFileURL(PLUGIN).href}?load=${loads}`);
  const inits = Object.values(mod).filter((v) => typeof v === "function");
  assert.equal(inits.length, 1, "the plugin exports one plugin function");
  return inits[0];
}

// The plugin with RALPHY_STATUS_FILE set to `file` while it starts.
async function started(file) {
  const before = process.env.RALPHY_STATUS_FILE;
  process.env.RALPHY_STATUS_FILE = file;
  try {
    const init = await plugin();
    return await init({ directory: "/repo", worktree: "/repo" });
  } finally {
    if (before === undefined) delete process.env.RALPHY_STATUS_FILE;
    else process.env.RALPHY_STATUS_FILE = before;
  }
}

function lines(file) {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
}

test("the plugin writes a first line when it loads", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ralphy-oc-plugin-"));
  try {
    const file = join(dir, "agent-status.jsonl");
    const hooks = await started(file);
    const first = lines(file);
    assert.equal(first.length, 1, "one line at load");
    assert.equal(first[0].event, "plugin.loaded");
    assert.equal(typeof first[0].ts, "string");

    await hooks.event({
      event: {
        type: "permission.asked",
        properties: {
          id: "per_1",
          sessionID: "ses_lead",
          permission: "bash",
          patterns: ["echo hello"],
          metadata: { command: "echo hello" },
          tool: { messageID: "msg_1", callID: "call_1" },
        },
      },
    });
    const after = lines(file);
    assert.equal(after.length, 2, "one more line for the permission");
    assert.equal(after[1].event, "permission.asked");
    assert.equal(after[1].session_id, "ses_lead");
    assert.equal(after[1].tool_use_id, "per_1");
    assert.equal(after[1].tool_name, "bash");

    // A subagent's line names it as the agent, so the lead is told apart.
    await hooks.event({
      event: { type: "session.created", properties: { info: { id: "ses_sub", parentID: "ses_lead" } } },
    });
    await hooks.event({
      event: { type: "permission.asked", properties: { id: "per_2", sessionID: "ses_sub", permission: "edit" } },
    });
    const sub = lines(file);
    assert.equal(sub.length, 3, "a subagent session writes no line of its own");
    assert.equal(sub[2].session_id, "ses_sub");
    assert.equal(sub[2].agent_id, "ses_sub");
    assert.equal(after[1].agent_id, undefined, "the lead has no agent id");

    // An event outside the table writes nothing.
    await hooks.event({ event: { type: "message.part.updated", properties: { sessionID: "ses_lead" } } });
    assert.equal(lines(file).length, 3, "no line for a text delta");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the plugin never throws and never prints", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ralphy-oc-plugin-"));
  const printed = [];
  const keep = {
    out: process.stdout.write,
    err: process.stderr.write,
    log: console.log,
    error: console.error,
    warn: console.warn,
    info: console.info,
    debug: console.debug,
  };
  const capture = (name) => (...args) => {
    printed.push(`${name}: ${args.map(String).join(" ")}`);
    return true;
  };
  process.stdout.write = capture("stdout");
  process.stderr.write = capture("stderr");
  for (const name of ["log", "error", "warn", "info", "debug"]) console[name] = capture(name);
  try {
    const file = join(dir, "no-such-folder", "agent-status.jsonl");
    const hooks = await started(file);
    const events = [
      { type: "session.created", properties: { info: { id: "ses_lead" } } },
      { type: "session.created", properties: { info: { id: "ses_sub", parentID: "ses_lead" } } },
      { type: "session.status", properties: { sessionID: "ses_lead", status: { type: "busy" } } },
      { type: "permission.asked", properties: { id: "per_1", sessionID: "ses_sub", permission: "bash" } },
      { type: "permission.replied", properties: { sessionID: "ses_sub", requestID: "per_1", reply: "once" } },
      { type: "question.asked", properties: { id: "que_1", sessionID: "ses_lead", questions: [{ question: "Which?" }] } },
      { type: "question.replied", properties: { sessionID: "ses_lead", requestID: "que_1", answers: [["a"]] } },
      { type: "question.rejected", properties: { sessionID: "ses_lead", requestID: "que_1" } },
      { type: "session.idle", properties: { sessionID: "ses_lead" } },
      // Malformed events: OpenCode might send a shape this version does not know.
      { type: "question.asked", properties: { questions: null } },
      { type: "session.created", properties: null },
      { type: "permission.asked" },
      null,
    ];
    for (const event of events) {
      await assert.doesNotReject(async () => hooks.event({ event }), JSON.stringify(event));
    }
    await assert.doesNotReject(async () => hooks.event(undefined), "no argument");
    assert.equal(existsSync(file), false, "the folder does not exist, so no file");
  } finally {
    process.stdout.write = keep.out;
    process.stderr.write = keep.err;
    for (const name of ["log", "error", "warn", "info", "debug"]) console[name] = keep[name];
    rmSync(dir, { recursive: true, force: true });
  }
  assert.deepEqual(printed, [], "nothing on stdout or stderr");
});

test("the plugin imports only the file system module", () => {
  const source = readFileSync(PLUGIN, "utf8");
  const specifiers = [
    ...source.matchAll(/\bimport\s+(?:[^"'`;]*?\s+from\s+)?["'`]([^"'`]+)["'`]/g),
    ...source.matchAll(/\bimport\s*\(\s*["'`]([^"'`]+)["'`]/g),
    ...source.matchAll(/\brequire\s*\(\s*["'`]([^"'`]+)["'`]/g),
  ].map((m) => m[1]);
  assert.deepEqual(specifiers, ["node:fs"]);
  assert.equal(/\bimport\s*\(/.test(source), false, "no dynamic import");
  assert.equal(/\brequire\s*\(/.test(source), false, "no require");
});
