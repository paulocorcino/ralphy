/* ---------------------------------------------------------------------------
   The page shortcuts: one list of commands and one key listener (ADR-0087).

   `COMMANDS` is the list: every page shortcut, its keys and where it may
   fire. A component gives a command its behaviour with `bindCommand`. One
   document listener (`wireCommands`) runs the list; `dispatchKey` is the
   same run, for a caller that holds the key first.
--------------------------------------------------------------------------- */

/** The keys of a command. A chord is Alt+Shift (⌥⇧) and `e.code`. */
export type Keys =
  | { kind: "chord"; code: string }
  | { kind: "typed"; key: string }
  | { kind: "modShift"; key: string };

/**
 * Where a command may fire. `page`: not while the focus takes keys (a field,
 * an editor, a terminal). `terminal`: from a terminal too. `always`: also
 * from a field or an editor.
 */
export type Scope = "page" | "terminal" | "always";

export type CommandGroup = "Page" | "Consoles and fences";

export type Command = {
  id: string;
  group: CommandGroup;
  label: string;
  keys: readonly Keys[];
  scope: Scope;
  // `false`: a key with no row in the palette (a walk on the stage).
  palette?: false;
};

/** What a component gives a command; `which` is the code or key pressed. */
export type Binding = {
  can: (which: string) => boolean;
  // `false`: nothing was done, and the key is not taken.
  run: (which: string) => boolean | void;
  // The palette's row for one key, when the label of the command is not enough.
  label?: (which: string) => string;
};

const chord = (code: string): Keys => ({ kind: "chord", code });
const range = (prefix: string, from: number, to: number) =>
  Array.from({ length: to - from + 1 }, (_, i) => chord(prefix + String(from + i)));

/** The fence keys reach every fence the plane can hold: F1 to F12. */
export const FENCE_KEYS = range("F", 1, 12);

export const COMMANDS: readonly Command[] = [
  {
    id: "search.open",
    group: "Page",
    label: "Search files or projects",
    keys: [{ kind: "typed", key: "/" }],
    scope: "page",
  },
  {
    id: "files.search",
    group: "Page",
    label: "Open the files search",
    keys: [{ kind: "modShift", key: "f" }],
    scope: "always",
  },
  {
    id: "palette.open",
    group: "Page",
    label: "Open the command palette",
    keys: [chord("KeyP")],
    scope: "terminal",
  },
  {
    id: "help.open",
    group: "Page",
    label: "Show the keyboard shortcuts",
    keys: [chord("Slash"), { kind: "typed", key: "?" }],
    scope: "terminal",
  },
  {
    id: "settings.open",
    group: "Page",
    label: "Open settings",
    keys: [],
    scope: "page",
  },
  {
    id: "consoles.row",
    group: "Consoles and fences",
    label: "Open the row with that digit in the consoles menu",
    keys: range("Digit", 0, 9),
    scope: "terminal",
  },
  {
    id: "consoles.run",
    group: "Consoles and fences",
    label: "Open the consoles menu on its command field",
    keys: [chord("KeyR")],
    scope: "terminal",
  },
  {
    id: "fence.go",
    group: "Consoles and fences",
    label: "Go to the fence with that number",
    keys: FENCE_KEYS,
    scope: "terminal",
  },
  {
    id: "stage.walk",
    group: "Consoles and fences",
    label: "Walk the columns and their rows, or else the fences",
    keys: ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].map((code) => chord(code)),
    scope: "terminal",
    palette: false,
  },
];

/** The only typed keys and Ctrl+Shift keys the list may hold. */
const TYPED_ALLOWED = new Set(["/", "?"]);
const MOD_SHIFT_ALLOWED = new Set(["f"]);
/** Alt+Shift with these is the system's or the browser's. */
const CHORD_REFUSED = new Set(["Tab", "Escape", "Space"]);

function keyId(k: Keys) {
  return k.kind === "chord" ? `chord:${k.code}` : `${k.kind}:${k.key}`;
}

function refusedReason(k: Keys) {
  switch (k.kind) {
    case "chord":
      return CHORD_REFUSED.has(k.code) ? "a chord the system keeps" : "";
    case "typed":
      return TYPED_ALLOWED.has(k.key) ? "" : "a typed key other than / and ?";
    case "modShift":
      return MOD_SHIFT_ALLOWED.has(k.key) ? "" : "a Ctrl or Cmd key other than Ctrl+Shift+F";
    default: {
      const never: never = k;
      return String(never);
    }
  }
}

/** What is wrong with a list: refused keys and keys used twice. */
export function listProblems(list: readonly Command[]) {
  const problems: string[] = [];
  const seen = new Map<string, string>();
  for (const cmd of list) {
    for (const k of cmd.keys) {
      const id = keyId(k);
      const why = refusedReason(k);
      if (why) problems.push(`${cmd.id}: ${id} is ${why}`);
      const other = seen.get(id);
      if (other !== undefined) problems.push(`${cmd.id}: ${id} is also ${other}`);
      seen.set(id, cmd.id);
    }
  }
  return problems;
}

type KeyEvent = Pick<
  KeyboardEvent,
  "key" | "code" | "altKey" | "shiftKey" | "ctrlKey" | "metaKey" | "preventDefault"
>;

const plain = (e: KeyEvent) => !e.ctrlKey && !e.metaKey && !e.altKey;
/** Alt+Shift (⌥⇧) with no Ctrl and no Meta: the chord of every page shortcut. */
export const altShift = (e: Pick<KeyEvent, "altKey" | "shiftKey" | "ctrlKey" | "metaKey">) =>
  e.altKey && e.shiftKey && !e.ctrlKey && !e.metaKey;
const modShift = (e: KeyEvent) => (e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey;

function matches(k: Keys, e: KeyEvent) {
  switch (k.kind) {
    case "chord":
      return altShift(e) && e.code === k.code;
    case "typed":
      return plain(e) && e.key === k.key;
    case "modShift":
      return modShift(e) && e.key.toLowerCase() === k.key;
    default: {
      const never: never = k;
      return Boolean(never);
    }
  }
}

/** The command a key event names, and which of its keys it is. */
export function commandFor(e: KeyEvent) {
  for (const cmd of COMMANDS) {
    const k = cmd.keys.find((x) => matches(x, e));
    if (k) return { cmd, which: k.kind === "chord" ? k.code : k.key, scope: scopeOf(cmd, k) };
  }
  return null;
}

const bindings = new Map<string, Binding>();

/** Give a command of `COMMANDS` its behaviour. The last binding wins. */
export function bindCommand(id: string, binding: Binding) {
  if (!COMMANDS.some((c) => c.id === id)) throw new Error(`no command ${id}`);
  bindings.set(id, binding);
}

/** `true` while the page shortcuts are blocked; `fromTerminal` lets a terminal pass. */
export type Blocked = (fromTerminal: boolean) => boolean;
export type Source = "page" | "terminal";

// A typed key never passes a terminal or a field, whatever its command's scope.
function scopeOf(cmd: Command, k: Keys): Scope {
  return k.kind === "typed" ? "page" : cmd.scope;
}

function allowed(scope: Scope, source: Source, blocked: Blocked) {
  if (scope === "always") return true;
  if (scope === "terminal") return !blocked(true);
  return source === "page" && !blocked(false);
}

// A key a terminal already ran: the document listener hears it after.
const ran = new WeakSet<object>();

/** Run the command of a key; `true` when it ran and the key is taken. */
export function dispatchKey(e: KeyEvent, source: Source, blocked: Blocked) {
  if (ran.has(e)) return true;
  const hit = commandFor(e);
  const binding = hit ? bindings.get(hit.cmd.id) : undefined;
  if (!hit || !binding) return false;
  if (!allowed(hit.scope, source, blocked)) return false;
  if (!binding.can(hit.which) || binding.run(hit.which) === false) return false;
  ran.add(e);
  e.preventDefault();
  return true;
}

/** The one document listener of the page shortcuts. */
export function wireCommands(doc: Pick<Document, "addEventListener">, blocked: Blocked) {
  doc.addEventListener("keydown", (e) => {
    dispatchKey(e, "page", blocked);
  });
}

/** One row of the palette: a command that can run now, for one of its keys. */
export type PaletteRow = { id: string; which: string; label: string; keys: Keys | null };

function whichOf(k: Keys) {
  return k.kind === "chord" ? k.code : k.key;
}

// The commands that may have rows: bound, not the palette itself, not a walk.
function listed() {
  return COMMANDS.flatMap((cmd) => {
    const binding = bindings.get(cmd.id);
    const shown = binding !== undefined && cmd.id !== "palette.open" && cmd.palette !== false;
    return shown ? [{ cmd, binding }] : [];
  });
}

/** The commands that can run now, one row per key; the palette is not one. */
export function paletteRows(): PaletteRow[] {
  const rows: PaletteRow[] = [];
  for (const { cmd, binding } of listed()) {
    const keys: (Keys | null)[] = cmd.keys.length > 0 ? [...cmd.keys] : [null];
    for (const k of keys) {
      const which = k ? whichOf(k) : "";
      if (!binding.can(which)) continue;
      rows.push({ id: cmd.id, which, label: binding.label?.(which) ?? cmd.label, keys: k });
    }
  }
  return rows;
}

/** Run one palette row, if its command can still run. */
export function runRow(row: Pick<PaletteRow, "id" | "which">) {
  const binding = bindings.get(row.id);
  if (binding?.can(row.which) === true) binding.run(row.which);
}

const ARROWS: Readonly<Record<string, string | undefined>> = {
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
};

// The key of an `e.code`, as a person reads it: `Digit3` is 3, `KeyR` is R.
function codeText(code: string) {
  return ARROWS[code] ?? code.replace(/^(?:Digit|Key)/u, "").replace("Slash", "/");
}

/** The keys as a person reads them on their system (ADR-0087 D11). */
export function keysText(k: Keys, mac: boolean) {
  switch (k.kind) {
    case "chord":
      return (mac ? "⌥⇧" : "Alt+Shift+") + codeText(k.code);
    case "typed":
      return k.key;
    case "modShift":
      return (mac ? "⌘⇧" : "Ctrl+Shift+") + k.key.toUpperCase();
    default: {
      const never: never = k;
      return String(never);
    }
  }
}

/** Whether this page runs on a Mac (or an iPad or iPhone). */
export const IS_MAC = /Mac|iPhone|iPad/u.test(navigator.platform || navigator.userAgent || "");

/** All the keys of a command, as a person reads them: `Alt+Shift+0 to 9`. */
export function commandKeysText(cmd: Command, mac: boolean) {
  const chords = cmd.keys.filter((k) => k.kind === "chord");
  const parts = cmd.keys.filter((k) => k.kind !== "chord").map((k) => keysText(k, mac));
  const [first, ...rest] = chords;
  if (first) {
    const tail = rest.map((k) => codeText(k.code));
    const more = tail.length > 3 ? ` to ${tail.at(-1) ?? ""}` : tail.map((t) => " " + t).join("");
    parts.unshift(keysText(first, mac) + more);
  }
  return parts.join(" or ");
}
