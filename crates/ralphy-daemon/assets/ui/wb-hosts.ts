// The Hosts dialog's state machine (ADR-0067 §11, issue #497, and the
// amendment "the Hosts dialog").
//
// PURE: state + event in, new state out. No DOM, no fetch, no Alpine. The shell
// runs the `host.*` verbs and feeds what they answer to `next`; the progress of
// `host check|add|remove` arrives as raw output chunks of JSON lines (the CLI's
// `--json` mode), which `feed` splits into events.
import { progress, sentence } from "./wb-hosts-progress.ts";

export { feed } from "./wb-hosts-progress.ts";
export { helpTabs, WRONG_ADDRESS } from "./wb-hosts-help.ts";
export type { HelpTab } from "./wb-hosts-help.ts";

/** An SSH config host the form can pick (`host.aliases`). */
export type Alias = { alias: string; hostname?: string; user?: string; port?: number | string };

/** One check of `host check`, as the dialog keeps it. */
export type Check = {
  id: string;
  label: string;
  status: string;
  text: string;
  command: string | null;
};

/** What `ralphy host install` would send. */
export type Install = {
  version: string;
  target: string;
  source: "release" | "this-computer";
  folder: string;
};

export type Failure = { kind: string; message: string; keyLine?: string | null };

/** A host key offered for trust (`host.key`). */
export type HostKey = { fingerprint: string; [field: string]: unknown };

/** The `host.key` answer. */
export type KeyReply = { state?: string; keys?: HostKey[]; reason?: string };

/** A paired host, as the list passes it to Edit. */
export type PeerHost = {
  daemon_id?: string;
  name?: string;
  destination?: string;
  identity_file?: string;
};

export type State = {
  open: boolean;
  tab: "hosts" | "add";
  editing: { daemon: string; name: string } | null;
  step: "connection" | "identity" | "checks" | "done";
  aliases: Alias[];
  alias: string;
  address: string;
  user: string;
  port: string;
  signIn: "config" | "key";
  keyFile: string;
  password: string;
  name: string;
  keys: HostKey[];
  os: string;
  checks: Check[];
  install: Install | null;
  lines: string[];
  failure: Failure | null;
  help: boolean;
  helpTab: string;
  busy: boolean;
  buf: string;
  done: boolean;
  addedName: string;
  autoInstall: boolean;
  autoTried: boolean;
};

/** One JSON line of the CLI's `--json` mode. */
export type Progress =
  | { event: "connected"; os?: string }
  | {
      event: "check";
      id: string;
      label?: string;
      status: string;
      text?: string;
      command?: string | null;
    }
  | { event: "fixed"; id: string }
  | { event: "install"; version?: string; target?: string; source?: string; folder?: string }
  | { event: "note"; text?: string }
  | { event: "failed"; kind?: string; message?: string; key_line?: string }
  | { event: "added"; name?: string };

/** What `next` folds. */
export type Action =
  | { type: "tab"; tab: string }
  | { type: "edit"; host?: PeerHost }
  | { type: "aliases"; aliases: unknown }
  | { type: "pick"; alias: string }
  | { type: "type"; field: keyof State; value: unknown }
  | { type: "host"; value: string }
  | { type: "busy"; value: boolean }
  | { type: "key"; key?: KeyReply | undefined }
  | { type: "close" }
  | { type: "cancel-trust" }
  | { type: "trusted" }
  | { type: "back" }
  | { type: "auto-tried" }
  | { type: "check-again" }
  | { type: "event"; event?: Progress }
  | { type: "exit"; verb: string; code: number | null | undefined }
  | { type: "help-tab"; tab: string };

export function initial(): State {
  return {
    open: false,
    // "hosts" lists the paired hosts; "add" is the form, also used to edit.
    tab: "hosts",
    // The host being edited, `{ daemon, name }`, or null to add one.
    editing: null,
    step: "connection",
    aliases: [],
    alias: "",
    address: "",
    user: "",
    port: "",
    signIn: "config",
    keyFile: "",
    // Used once to add Ralphy's key on the host; emptied once signed in.
    password: "",
    name: "",
    keys: [],
    os: "",
    checks: [],
    // What `ralphy host install` would send, when Ralphy must be installed.
    install: null,
    lines: [],
    failure: null,
    help: false,
    helpTab: "windows",
    busy: false,
    buf: "",
    done: false,
    // The name `host add` gave the host, shown by the last step.
    addedName: "",
    // The operator's opt-in: install Ralphy when the checks offer it. The
    // check box is the permission, as the install button is.
    autoInstall: false,
    // The install ran once for this Connect; a failed one is not repeated.
    autoTried: false,
  };
}

// What `ssh` is given: the alias itself (the SSH config applies), else
// `user@address`, or the `ssh://` form when the port is not 22.
export function destination(s: State): string {
  if (s.alias) return s.alias;
  const address = s.address.trim();
  if (!address) return "";
  const user = s.user.trim();
  const port = s.port.trim();
  const who = user ? user + "@" + address : address;
  if (port && port !== "22") return "ssh://" + who + ":" + port;
  return who;
}

// The form fields `destination` was built from, to fill the form when a
// host is edited. A destination without `@` is an address or an SSH config
// alias; either way it goes back into the Host field unchanged.
export function fields(dest: string | null | undefined): {
  user: string;
  address: string;
  port: string;
} {
  let d = (dest ?? "").trim();
  let port = "";
  if (d.indexOf("ssh://") === 0) {
    d = d.slice("ssh://".length);
    const colon = d.lastIndexOf(":");
    if (colon > d.indexOf("@")) {
      port = d.slice(colon + 1);
      d = d.slice(0, colon);
    }
  }
  const at = d.lastIndexOf("@");
  return { user: at < 0 ? "" : d.slice(0, at), address: at < 0 ? d : d.slice(at + 1), port: port };
}

// A new form on `tab`, keeping what does not belong to one host.
function fresh(s: State, tab: State["tab"]): State {
  return Object.assign(initial(), { open: s.open, aliases: s.aliases, tab: tab });
}

// Edit is the add flow with the host's connection filled in. The
// password is empty: the host still accepts Ralphy's key, or it asks.
function editState(s: State, h: PeerHost): State {
  const keyFile = typeof h.identity_file === "string" ? h.identity_file : "";
  return Object.assign(
    fresh(s, "add"),
    { editing: { daemon: h.daemon_id ?? "", name: h.name ?? "" } },
    fields(h.destination),
    keyFile ? { signIn: "key" as const, keyFile: keyFile } : {},
  );
}

// An SSH config host picked from the list fills user and port; a name that
// is not in the list empties them.
function pickState(s: State, alias: string): State {
  const found = s.aliases.find((a) => a.alias === alias);
  if (!found) return Object.assign({}, s, { alias: "", user: "", port: "" });
  return Object.assign({}, s, {
    alias: found.alias,
    address: found.hostname ?? "",
    user: found.user ?? "",
    port: found.port === undefined || found.port === 0 ? "" : String(found.port),
  });
}

// The `host.key` answer: a key to trust, a host that does not answer, or a
// known key, which goes on to the checks.
function keyState(s: State, key: KeyReply): State {
  if (key.state === "unknown") {
    return Object.assign({}, s, { step: "identity" as const, keys: key.keys ?? [] });
  }
  if (key.state === "unreachable") {
    return Object.assign({}, s, {
      help: true,
      failure: { kind: "unreachable", message: sentence(key.reason) },
    });
  }
  return Object.assign({}, s, {
    step: "checks" as const,
    failure: null,
    help: false,
    autoTried: false,
  });
}

// A `host.*` command ended with `ev.code`.
function exitState(s: State, ev: Extract<Action, { type: "exit" }>): State {
  if (ev.code === 0) {
    if (ev.verb !== "host.add") return Object.assign({}, s, { busy: false });
    // An edit goes back to the list it started from; a new host gets
    // a last step that says it was added.
    return s.editing
      ? fresh(s, "hosts")
      : Object.assign({}, s, { step: "done" as const, busy: false });
  }
  const failure = s.failure ?? {
    kind: "other",
    message:
      ev.code === null || ev.code === undefined
        ? "The command stopped before it finished."
        : "The command stopped with code " + ev.code + ".",
  };
  return Object.assign({}, s, { busy: false, failure: failure });
}

export function next(s: State, ev: Action): State {
  switch (ev.type) {
    case "tab":
      return fresh(s, ev.tab === "add" ? "add" : "hosts");
    case "edit":
      return editState(s, ev.host ?? {});
    case "aliases":
      return Object.assign({}, s, { aliases: Array.isArray(ev.aliases) ? ev.aliases : [] });
    case "pick":
      return pickState(s, ev.alias);
    case "type":
      return Object.assign({}, s, { [ev.field]: ev.value });
    // Typing in the Host field leaves the SSH config host: the text is an
    // address now, and User and Port are the operator's again.
    case "host":
      return Object.assign({}, s, { alias: "", address: ev.value });
    case "busy":
      return Object.assign({}, s, { busy: ev.value });
    case "key":
      return keyState(s, ev.key ?? {});
    case "close":
      return Object.assign({}, s, { open: false, password: "" });
    case "cancel-trust":
      return Object.assign({}, s, { step: "connection" as const, keys: [] });
    case "trusted":
      return Object.assign({}, s, { step: "checks" as const });
    // Back to the fields, which keep what was typed.
    case "back":
      return Object.assign({}, s, {
        step: "connection" as const,
        checks: [],
        install: null,
        lines: [],
        failure: null,
        help: false,
        buf: "",
        autoTried: false,
      });
    case "auto-tried":
      return Object.assign({}, s, { autoTried: true });
    case "check-again":
      return Object.assign({}, s, {
        checks: [],
        install: null,
        lines: [],
        failure: null,
        help: false,
        buf: "",
      });
    case "event":
      return progress(s, ev.event);
    case "exit":
      return exitState(s, ev);
    case "help-tab":
      return Object.assign({}, s, { helpTab: ev.tab });
    default:
      return s;
  }
}

// `ralphy host add` cannot go on while this check stands. The same rule as
// the CLI's `HostCheck::is_blocking`.
function blocks(c: Check): boolean {
  if (c.id === "ralphy") return c.status !== "pass";
  if (c.id === "name") return c.status !== "pass" && c.status !== "fix";
  return false;
}

// Add host is possible once the checks ran, Ralphy is ready on the host, and
// the daemon has a name or gets one.
export function ready(s: State): boolean {
  if (s.checks.length === 0 || s.failure) return false;
  return !s.checks.some(blocks);
}

// The checks as the dialog shows them: the one check that blocks Add host,
// the checks that passed, what Add host sets up, and advice. Advice is shown
// only when nothing blocks, so one thing to do is on screen at a time. A
// check that waits for an earlier one is not shown.
export function view(s: State) {
  const found = s.checks.find(blocks);
  let blocker = null;
  if (found) {
    // The first sentence is the heading; the rest explains it.
    const text = sentence(found.text);
    const cut = text.indexOf(". ");
    blocker = {
      id: found.id,
      title: cut < 0 ? text : text.slice(0, cut),
      detail: cut < 0 ? "" : text.slice(cut + 2),
      command: (found.command ?? "") || null,
    };
  }
  const passed = s.checks.filter((c) => c.status === "pass");
  const fixes = s.checks.filter((c) => c.status === "fix");
  const advice = found ? [] : s.checks.filter((c) => c.status === "warn" || c.status === "copy");
  return {
    blocker: blocker,
    passed: passed.map((c) => Object.assign({}, c, { text: sentence(c.text) })),
    passedText: passed.length === 1 ? "1 check passed" : passed.length + " checks passed",
    fixText:
      fixes.length > 0
        ? "In the next step, Ralphy also sets up: " +
          fixes.map((c) => (c.label || c.id).toLowerCase()).join(", ") +
          "."
        : "",
    advice: advice.map((c) => Object.assign({}, c, { text: sentence(c.text) })),
  };
}

// The footer's main button, which is always the next thing to do.
export function primary(s: State): "" | "connect" | "install" | "check" {
  if (s.step !== "checks") return "";
  if (ready(s)) return "connect";
  if (needsInstall(s)) return "install";
  return "check";
}

// The host daemon has no name, and a name given here would fix it.
export function needsName(s: State): boolean {
  const name = s.checks.find((c) => c.id === "name");
  return (
    name !== undefined &&
    name.status === "copy" &&
    typeof name.command === "string" &&
    name.command.indexOf("ralphy host add ") === 0
  );
}

// Ralphy on the host must be installed, and this computer can send it.
export function needsInstall(s: State): boolean {
  const ralphy = s.checks.find((c) => c.id === "ralphy");
  return s.install !== null && ralphy !== undefined && ralphy.status !== "pass";
}

// The last step of a new host.
export function addedText(s: State): string {
  const name = s.addedName || "The host";
  return "Added " + name + ". Its projects are now in the list of projects.";
}

// The operator asked for the install, the checks offer one, and it has not
// run yet for this Connect. No offer comes when the host has a newer Ralphy.
export function wantsAutoInstall(s: State): boolean {
  return s.autoInstall && !s.autoTried && needsInstall(s);
}

// What the install button sends, in one sentence.
export function installText(s: State): string {
  const i = s.install;
  if (!i) return "";
  const from = i.source === "release" ? "downloaded from its release" : "from this computer";
  return (
    "Ralphy " +
    i.version +
    " for " +
    i.target +
    ", " +
    from +
    ", into " +
    i.folder +
    " on the host."
  );
}
