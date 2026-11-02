// The progress events of the Hosts dialog: the JSON lines of the CLI's `--json` mode, split
// from raw output and folded into the dialog's state.
//
// PURE, as `wb-hosts.ts`, which re-exports `feed` and folds `progress` in `next`.
import { isRecord } from "./wb-api.ts";
import type { Check, Install, Progress, State } from "./wb-hosts.ts";

// Split raw output into JSON-line events. `buf` carries a partial last line
// to the next chunk; a line that is not an event object is dropped.
export function feed(
  buf: string | null | undefined,
  chunk: string | null | undefined,
): { rest: string; events: Progress[] } {
  const parts = (String(buf || "") + String(chunk || "")).split("\n");
  // `split` always returns at least one part.
  const rest = parts.pop() ?? "";
  const events: Progress[] = [];
  for (const raw of parts) {
    const line = raw.trim();
    if (!line) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }
    // The one unchecked step: the daemon runs its own `ralphy` executable,
    // so a line is an event of the same build, as a reply of `apiFetch` is.
    if (isRecord(parsed) && typeof parsed.event === "string") {
      events.push(parsed as Progress);
    }
  }
  return { rest: rest, events: events };
}

// The CLI's errors are chained, so they start lowercase; shown alone they
// start a sentence.
export function sentence(text: string | null | undefined): string {
  const t = String(text || "");
  return t.charAt(0).toUpperCase() + t.slice(1);
}

function upsert(checks: Check[], check: Check): Check[] {
  const at = checks.findIndex((c) => c.id === check.id);
  if (at < 0) return checks.concat([check]);
  const copy = checks.slice();
  copy[at] = check;
  return copy;
}

// The check a `check` event reports, as the dialog keeps it.
function checkOf(ev: Extract<Progress, { event: "check" }>): Check {
  return {
    id: ev.id,
    label: ev.label || ev.id,
    status: ev.status,
    text: ev.text || "",
    command: ev.command || null,
  };
}

// What `ralphy host install` would send, from an `install` event.
function installOf(ev: Extract<Progress, { event: "install" }>): Install {
  return {
    version: String(ev.version || ""),
    target: String(ev.target || ""),
    source: ev.source === "release" ? ("release" as const) : ("this-computer" as const),
    folder: String(ev.folder || ""),
  };
}

// The state after a `failed` event.
function failedState(s: State, ev: Extract<Progress, { event: "failed" }>): State {
  const failure = {
    kind: ev.kind || "other",
    message: sentence(ev.message),
    keyLine: typeof ev.key_line === "string" && ev.key_line ? ev.key_line : null,
  };
  const after: State = Object.assign({}, s, { failure: failure, help: ev.kind === "unreachable" });
  // Sign-in failed: back to the fields, where the password is typed.
  if (ev.kind === "auth_refused") after.step = "connection";
  if (ev.kind === "password_refused") Object.assign(after, { step: "connection", password: "" });
  return after;
}

// One progress event of the CLI's `--json` mode.
export function progress(s: State, ev: Progress | undefined): State {
  if (!ev) return s;
  switch (ev.event) {
    // Signed in: a key works now, so the password is not kept.
    case "connected":
      return Object.assign({}, s, { os: ev.os || "", password: "" });
    case "check":
      return Object.assign({}, s, { checks: upsert(s.checks, checkOf(ev)) });
    case "fixed":
      return Object.assign({}, s, {
        checks: s.checks.map((c) =>
          c.id === ev.id ? Object.assign({}, c, { status: "pass" }) : c,
        ),
      });
    case "install":
      return Object.assign({}, s, { install: installOf(ev) });
    case "note":
      return Object.assign({}, s, { lines: s.lines.concat([ev.text || ""]) });
    case "failed":
      return failedState(s, ev);
    case "added":
      return Object.assign({}, s, { done: true, addedName: String(ev.name || "") });
    default:
      return s;
  }
}
