// The Hosts dialog's state machine (ADR-0067 §11, issue #497, and the
// amendment "the Hosts dialog").
//
// PURE: state + event in, new state out. No DOM, no fetch, no Alpine. The shell
// runs the `host.*` verbs and feeds what they answer to `next`; the progress of
// `host check|add|remove` arrives as raw output chunks of JSON lines (the CLI's
// `--json` mode), which `feed` splits into events.

/** An SSH config host the form can pick (`host.aliases`). */
export type Alias = { alias: string; hostname?: string; user?: string; port?: number | string };

/** One check of `host check`, as the dialog keeps it. */
export type Check = { id: string; label: string; status: string; text: string; command: string | null };

/** What `ralphy host install` would send. */
export type Install = { version: string; target: string; source: "release" | "this-computer"; folder: string };

export type Failure = { kind: string; message: string; keyLine?: string | null };

/** A host key offered for trust (`host.key`). */
export type HostKey = { fingerprint: string; [field: string]: unknown };

/** The `host.key` answer. */
export type KeyReply = { state?: string; keys?: HostKey[]; reason?: string };

/** A paired host, as the list passes it to Edit. */
export type PeerHost = { daemon_id?: string; name?: string; destination?: string; identity_file?: string };

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
  | { event: "check"; id: string; label?: string; status: string; text?: string; command?: string | null }
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
  | { type: "key"; key?: KeyReply }
  | { type: "close" }
  | { type: "cancel-trust" }
  | { type: "trusted" }
  | { type: "back" }
  | { type: "auto-tried" }
  | { type: "check-again" }
  | { type: "event"; event?: Progress }
  | { type: "exit"; verb: string; code: number | null | undefined }
  | { type: "help-tab"; tab: string };

export type HelpTab = { id: string; name: string; steps: { text: string | null; command: string | null }[] };

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
  const address = String(s.address || "").trim();
  if (!address) return "";
  const user = String(s.user || "").trim();
  const port = String(s.port || "").trim();
  const who = user ? user + "@" + address : address;
  if (port && port !== "22") return "ssh://" + who + ":" + port;
  return who;
}

// The form fields `destination` was built from, to fill the form when a
// host is edited. A destination without `@` is an address or an SSH config
// alias; either way it goes back into the Host field unchanged.
export function fields(dest: unknown): { user: string; address: string; port: string } {
  let d = String(dest || "").trim();
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

// Split raw output into JSON-line events. `buf` carries a partial last line
// to the next chunk; a line that is not an event object is dropped.
export function feed(buf: unknown, chunk: unknown): { rest: string; events: Progress[] } {
  const parts = (String(buf || "") + String(chunk || "")).split("\n");
  // `split` always returns at least one part.
  const rest = parts.pop() ?? "";
  const events: Progress[] = [];
  for (const raw of parts) {
    const line = raw.trim();
    if (!line) continue;
    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }
    if (parsed && typeof parsed === "object" && typeof parsed.event === "string") {
      events.push(parsed);
    }
  }
  return { rest: rest, events: events };
}

// The CLI's errors are chained, so they start lowercase; shown alone they
// start a sentence.
function sentence(text: unknown): string {
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

// One progress event of the CLI's `--json` mode.
function progress(s: State, ev: Progress | undefined): State {
  if (!ev) return s;
  switch (ev.event) {
    // Signed in: a key works now, so the password is not kept.
    case "connected":
      return Object.assign({}, s, { os: ev.os || "", password: "" });
    case "check":
      return Object.assign({}, s, {
        checks: upsert(s.checks, {
          id: ev.id,
          label: ev.label || ev.id,
          status: ev.status,
          text: ev.text || "",
          command: ev.command || null,
        }),
      });
    case "fixed":
      return Object.assign({}, s, {
        checks: s.checks.map((c) =>
          c.id === ev.id ? Object.assign({}, c, { status: "pass" }) : c,
        ),
      });
    case "install":
      return Object.assign({}, s, {
        install: {
          version: String(ev.version || ""),
          target: String(ev.target || ""),
          source: ev.source === "release" ? ("release" as const) : ("this-computer" as const),
          folder: String(ev.folder || ""),
        },
      });
    case "note":
      return Object.assign({}, s, { lines: s.lines.concat([ev.text || ""]) });
    case "failed": {
      const failure = {
        kind: ev.kind || "other",
        message: sentence(ev.message),
        keyLine: typeof ev.key_line === "string" && ev.key_line ? ev.key_line : null,
      };
      const next: State = Object.assign({}, s, { failure: failure, help: ev.kind === "unreachable" });
      // Sign-in failed: back to the fields, where the password is typed.
      if (ev.kind === "auth_refused") next.step = "connection";
      if (ev.kind === "password_refused") Object.assign(next, { step: "connection", password: "" });
      return next;
    }
    case "added":
      return Object.assign({}, s, { done: true, addedName: String(ev.name || "") });
    default:
      return s;
  }
}

export function next(s: State, ev: Action): State {
  switch (ev.type) {
    case "tab":
      return fresh(s, ev.tab === "add" ? "add" : "hosts");
    // Edit is the add flow with the host's connection filled in. The
    // password is empty: the host still accepts Ralphy's key, or it asks.
    case "edit": {
      const h: PeerHost = ev.host || {};
      const keyFile = typeof h.identity_file === "string" ? h.identity_file : "";
      return Object.assign(
        fresh(s, "add"),
        { editing: { daemon: String(h.daemon_id || ""), name: String(h.name || "") } },
        fields(h.destination),
        keyFile ? { signIn: "key" as const, keyFile: keyFile } : {},
      );
    }
    case "aliases":
      return Object.assign({}, s, { aliases: Array.isArray(ev.aliases) ? ev.aliases : [] });
    case "pick": {
      const found = s.aliases.find((a) => a.alias === ev.alias);
      if (!found) return Object.assign({}, s, { alias: "", user: "", port: "" });
      return Object.assign({}, s, {
        alias: found.alias,
        address: found.hostname || "",
        user: found.user || "",
        port: found.port ? String(found.port) : "",
      });
    }
    case "type":
      return Object.assign({}, s, { [ev.field]: ev.value });
    // Typing in the Host field leaves the SSH config host: the text is an
    // address now, and User and Port are the operator's again.
    case "host":
      return Object.assign({}, s, { alias: "", address: ev.value });
    case "busy":
      return Object.assign({}, s, { busy: !!ev.value });
    case "key": {
      const key: KeyReply = ev.key || {};
      if (key.state === "unknown") {
        return Object.assign({}, s, { step: "identity" as const, keys: key.keys || [] });
      }
      if (key.state === "unreachable") {
        return Object.assign({}, s, {
          help: true,
          failure: { kind: "unreachable", message: sentence(key.reason) },
        });
      }
      return Object.assign({}, s, { step: "checks" as const, failure: null, help: false, autoTried: false });
    }
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
    case "exit": {
      if (ev.code === 0) {
        if (ev.verb !== "host.add") return Object.assign({}, s, { busy: false });
        // An edit goes back to the list it started from; a new host gets
        // a last step that says it was added.
        return s.editing ? fresh(s, "hosts") : Object.assign({}, s, { step: "done" as const, busy: false });
      }
      const failure = s.failure || {
        kind: "other",
        message:
          ev.code === null || ev.code === undefined
            ? "The command stopped before it finished."
            : "The command stopped with code " + ev.code + ".",
      };
      return Object.assign({}, s, { busy: false, failure: failure });
    }
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
  if (!s.checks.length || s.failure) return false;
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
      command: found.command || null,
    };
  }
  const passed = s.checks.filter((c) => c.status === "pass");
  const fixes = s.checks.filter((c) => c.status === "fix");
  const advice = found
    ? []
    : s.checks.filter((c) => c.status === "warn" || c.status === "copy");
  return {
    blocker: blocker,
    passed: passed.map((c) => Object.assign({}, c, { text: sentence(c.text) })),
    passedText: passed.length === 1 ? "1 check passed" : passed.length + " checks passed",
    fixText: fixes.length
      ? "In the next step, Ralphy also sets up: " +
        fixes.map((c) => String(c.label || c.id).toLowerCase()).join(", ") +
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
  return !!(
    name &&
    name.status === "copy" &&
    typeof name.command === "string" &&
    name.command.indexOf("ralphy host add ") === 0
  );
}

// Ralphy on the host must be installed, and this computer can send it.
export function needsInstall(s: State): boolean {
  const ralphy = s.checks.find((c) => c.id === "ralphy");
  return !!(s.install && ralphy && ralphy.status !== "pass");
}

// The last step of a new host.
export function addedText(s: State): string {
  const name = s.addedName || "The host";
  return "Added " + name + ". Its projects are now in the list of projects.";
}

// The operator asked for the install, the checks offer one, and it has not
// run yet for this Connect. No offer comes when the host has a newer Ralphy.
export function wantsAutoInstall(s: State): boolean {
  return !!(s.autoInstall && !s.autoTried && needsInstall(s));
}

// What the install button sends, in one sentence.
export function installText(s: State): string {
  const i = s.install;
  if (!i) return "";
  const from = i.source === "release" ? "downloaded from its release" : "from this computer";
  return "Ralphy " + i.version + " for " + i.target + ", " + from + ", into " + i.folder + " on the host.";
}

// How to turn on the SSH server, per system, when the connection failed
// before the dialog could learn the host's system.
export function helpTabs(): HelpTab[] {
  return [
    {
      id: "windows",
      name: "Windows",
      steps: [
        {
          text: "In PowerShell as administrator, install the SSH server and start it.",
          command: "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
        },
        { text: null, command: "Start-Service sshd" },
        { text: null, command: "Set-Service -Name sshd -StartupType Automatic" },
        {
          text: "Check that the firewall lets SSH in.",
          command: "Get-NetFirewallRule -Name OpenSSH-Server-In-TCP",
        },
        {
          text: "When the rule is missing, create it.",
          command:
            "New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22",
        },
        {
          text: "Sign in with the account password, not the “Windows Hello” PIN.",
          command: null,
        },
        {
          text: "With a Microsoft account, the user name is the local one. This command shows it.",
          command: "whoami",
        },
      ],
    },
    {
      id: "macos",
      name: "macOS",
      steps: [
        {
          text: "On macOS 13 and later, turn on “Remote Login” in “System Settings › General › Sharing”.",
          command: null,
        },
        {
          text: "On macOS 12, turn on “Remote Login” in “System Preferences › Sharing”.",
          command: null,
        },
      ],
    },
    {
      id: "linux",
      name: "Linux",
      steps: [
        {
          text: "Install the SSH server and start it. On Debian and Ubuntu:",
          command: "sudo apt install openssh-server && sudo systemctl enable --now ssh",
        },
        {
          text: "On Fedora:",
          command: "sudo dnf install openssh-server && sudo systemctl enable --now sshd",
        },
      ],
    },
  ];
}

export const WRONG_ADDRESS = {
  text: "A refused connection can also mean a wrong address. Read the address on the host itself.",
  commands: [
    { name: "macOS", command: "ipconfig getifaddr en0" },
    { name: "Linux", command: "hostname -I" },
    { name: "Windows", command: "ipconfig" },
  ],
};
