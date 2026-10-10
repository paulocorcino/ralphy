/* ---------------------------------------------------------------------------
   Verb-failure presenter — the single "is this reply a failure, and what does
   it say?" adapter (#207).

   Every daemon verb reply is either a one-shot Query/Mutate/Observe reply
   (`{status:"ok"}` or `{status:"error", message|reason}`) or a `spawn` stream
   frame (`output`/`exited`/`error`). `isError` treats ONLY `status === "error"`
   (and a missing reply) as a failure, so a `spawn` stream's `output`/`exited`
   frames are never misread as errors.

   `message` returns the daemon's `message`/`reason` unchanged: a compare site
   reads the code from it. `failed` and `cause` turn the same reply into text
   for the screen (ADR-0065 §6, #432):
   - a code (`exists`, `unknown repo`…) gets its cause from CAUSE;
   - CLI output (the git, changes, branch, worktree and config verbs) comes as
     the `ralphy` binary printed it: `Error: cannot commit: nothing is staged —
     stage a file first`. The prefix, the `Caused by:` block and the leading
     `cannot <x>: ` / `refusing to <verb>: ` go, and the text after ` — `
     becomes a second sentence;
   - a text that is already a sentence (a capital letter, a final period) is
     shown as it is.
--------------------------------------------------------------------------- */
// A cause fits after `Could not <act>: `: lowercase, no final period.
/** One read of a shown fact, as `readFold` keeps it (ADR-0070 D3). */
export type Read = { value: unknown; goodAt: number; error: string; current: boolean };
/** What one read attempt answered. */
export type Outcome = { ok: boolean; value?: unknown; reason?: string; at: number };
/** A verb's reply, a `spawn` frame, or nothing: what a failure is read from. */
type Reply = { status?: string; message?: string; reason?: string } | null | undefined;

const CAUSE: Record<string, string> = {
  "not found": "it does not exist",
  exists: "a file with that name already exists",
  refused: "the daemon refused it",
  "io error": "the disk read or write failed",
  "too large": "the file is too large",
  binary: "the file is binary",
  "not an image": "the file is not an image",
  "not a note": "the file is not a note",
  "unknown encoding": "the workbench does not know that encoding",
  unencodable: "some characters do not exist in that encoding",
  unavailable: "the project is not available",
  transport: "the daemon did not answer",
  "unknown verb": "the daemon does not know that command",
  "unknown repo": "the project is not in the list",
  "unknown checkout": "the worktree does not exist",
  "repo registry unreadable": "the daemon could not read its project list",
  "invalid run options": "the request was not valid",
  "invalid query options": "the request was not valid",
  "invalid mutation options": "the request was not valid",
  "spawn failed": "the daemon could not run Ralphy",
  "query read failed": "the daemon could not run Ralphy",
  "mutation write failed": "the daemon could not run Ralphy",
  "a run is not federated yet": "a run on a peer is not available yet",
};

function isError(reply: Reply) {
  return !reply || reply.status === "error";
}
function message(reply: Reply, fallback: string) {
  return (reply && (reply.message || reply.reason)) || fallback || "refused";
}

const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const bare = (s: string) => s.trim().replace(/[.;:]+$/, "");
// One line only: a block of CLI output can start with a capital and end
// with a period and still hold an `Error: ` line.
const isSentence = (s: string) => /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes("\n");

// One line of text from the CLI's output: the error line and its causes,
// or every line when there is no `Error: ` line.
function oneLine(text: string) {
  let lines = text.replace(/\r/g, "").split("\n");
  const trace = lines.findIndex((l) => /^\s*Stack backtrace:/.test(l));
  if (trace >= 0) lines = lines.slice(0, trace);
  const at = lines.findIndex((l) => l.startsWith("Error: "));
  if (at < 0) return lines.map((l) => l.trim()).filter(Boolean).join(" ");
  const parts = [lines[at].slice("Error: ".length).trim()];
  for (const l of lines.slice(at + 1)) {
    const t = l.trim().replace(/^\d+: /, "");
    if (t && t !== "Caused by:") parts.push(t);
  }
  return parts.join(": ");
}

// [cause, ...remedies] for a reply, or null when it says nothing. A cause
// that is already a sentence comes back as { sentence }.
type Parts = { sentence: string } | { cause: string; rest: string[] };

function parts(reply: Reply): Parts | null {
  const raw = String((reply && (reply.message || reply.reason)) || "").trim();
  if (!raw) return null;
  if (Object.hasOwn(CAUSE, raw)) return { cause: CAUSE[raw], rest: [] };
  if (isSentence(raw) && !raw.startsWith("Error: ")) return { sentence: raw };
  const text = oneLine(raw).replace(/^(cannot|refusing to) [^:]+: /, "");
  const [cause, ...rest] = text.split(" — ").map(bare).filter(Boolean);
  return cause ? { cause, rest } : null;
}

const tail = (rest: string[]) => rest.map((r) => ` ${capital(r)}.`).join("");

// `Could not <act>: <cause>.` The act comes from `fallback`, a whole
// sentence of the same shape that is shown when the reply says nothing.
function failed(reply: Reply, fallback: string) {
  const p = parts(reply);
  if (!p) return fallback;
  if ("sentence" in p) return p.sentence;
  const act = bare(fallback.split(": ")[0]);
  return `${act}: ${p.cause}.${tail(p.rest)}`;
}

// The cause alone, as a sentence, for a place whose title already names
// the act.
function cause(reply: Reply, fallback: string) {
  const p = parts(reply);
  if (!p) return fallback;
  if ("sentence" in p) return p.sentence;
  return `${capital(p.cause)}.${tail(p.rest)}`;
}

// The cause alone, to follow `Could not <act>: ` in a line the caller
// builds: lowercase, no final period. A code never shows as it is.
function why(reply: Reply, fallback: string) {
  const p = parts(reply);
  if (!p) return fallback;
  if ("sentence" in p) return bare(p.sentence);
  return p.cause;
}

// One line of a success report (`warning: …`) as a sentence.
function sentence(line: unknown) {
  const t = bare(String(line || ""));
  return t ? `${capital(t)}.` : "";
}

// One read of a shown fact folded into the last one (ADR-0070 D3). A
// failure after a good read keeps that read's value, marked not current;
// a failure before any good read has no value at all.
function readFold(prev: Read | null | undefined, outcome: Outcome): Read {
  const { ok, value, reason, at } = outcome;
  if (ok) return { value, goodAt: at, error: "", current: true };
  if (prev?.goodAt) {
    return { value: prev.value, goodAt: prev.goodAt, error: reason as string, current: false };
  }
  return { value: null, goodAt: 0, error: reason as string, current: false };
}

// The line a panel shows for a read that is not current; "" when it is.
function notCurrent(read: Read | null | undefined, fmt: (ms: number) => string) {
  if (!read || read.current) return "";
  const error = Object.hasOwn(CAUSE, read.error) ? CAUSE[read.error] : read.error;
  if (read.goodAt) return `Read at ${fmt(read.goodAt)}. Not current: ${error}`;
  return `Could not read: ${error}`;
}

export const WBFail = {
  isError,
  message,
  failed,
  cause,
  why,
  sentence,
  readFold,
  notCurrent,
  CAUSE,
};
