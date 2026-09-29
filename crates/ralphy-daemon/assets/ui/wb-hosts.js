// The Add a host dialog's state machine (ADR-0067 §11, issue #497).
//
// PURE: state + event in, new state out. No DOM, no fetch, no Alpine. The shell
// runs the `host.*` verbs and feeds what they answer to `next`; the progress of
// `host check|add|remove` arrives as raw output chunks of JSON lines (the CLI's
// `--json` mode), which `feed` splits into events.
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  if (root) root.WBHosts = api;
})(typeof window === "undefined" ? null : window, function () {
  "use strict";

  function initial() {
    return {
      open: false,
      step: "connection",
      aliases: [],
      alias: "",
      address: "",
      user: "",
      port: "",
      signIn: "config",
      keyFile: "",
      name: "",
      keys: [],
      os: "",
      checks: [],
      lines: [],
      failure: null,
      help: false,
      helpTab: "windows",
      busy: false,
      buf: "",
      done: false,
    };
  }

  // What `ssh` is given: the alias itself (the SSH config applies), else
  // `user@address`, or the `ssh://` form when the port is not 22.
  function destination(s) {
    if (s.alias) return s.alias;
    const address = String(s.address || "").trim();
    if (!address) return "";
    const user = String(s.user || "").trim();
    const port = String(s.port || "").trim();
    const who = user ? user + "@" + address : address;
    if (port && port !== "22") return "ssh://" + who + ":" + port;
    return who;
  }

  // Split raw output into JSON-line events. `buf` carries a partial last line
  // to the next chunk; a line that is not an event object is dropped.
  function feed(buf, chunk) {
    const parts = (String(buf || "") + String(chunk || "")).split("\n");
    const rest = parts.pop();
    const events = [];
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

  function upsert(checks, check) {
    const at = checks.findIndex((c) => c.id === check.id);
    if (at < 0) return checks.concat([check]);
    const copy = checks.slice();
    copy[at] = check;
    return copy;
  }

  // One progress event of the CLI's `--json` mode.
  function progress(s, ev) {
    switch (ev.event) {
      case "connected":
        return Object.assign({}, s, { os: ev.os || "" });
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
      case "note":
        return Object.assign({}, s, { lines: s.lines.concat([ev.text || ""]) });
      case "failed":
        return Object.assign({}, s, {
          failure: { kind: ev.kind || "other", message: ev.message || "" },
          help: ev.kind === "unreachable",
        });
      case "added":
        return Object.assign({}, s, { done: true });
      default:
        return s;
    }
  }

  function next(s, ev) {
    switch (ev.type) {
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
      case "busy":
        return Object.assign({}, s, { busy: !!ev.value });
      case "key": {
        const key = ev.key || {};
        if (key.state === "unknown") {
          return Object.assign({}, s, { step: "identity", keys: key.keys || [] });
        }
        if (key.state === "unreachable") {
          return Object.assign({}, s, {
            help: true,
            failure: { kind: "unreachable", message: key.reason || "" },
          });
        }
        return Object.assign({}, s, { step: "checks", failure: null, help: false });
      }
      case "cancel-trust":
        return Object.assign({}, s, { step: "connection", keys: [] });
      case "trusted":
        return Object.assign({}, s, { step: "checks" });
      case "check-again":
        return Object.assign({}, s, {
          checks: [],
          lines: [],
          failure: null,
          help: false,
          buf: "",
        });
      case "event":
        return progress(s, ev.event || {});
      case "exit": {
        if (ev.code === 0) {
          return ev.verb === "host.add" ? Object.assign({}, s, { open: false, busy: false }) : Object.assign({}, s, { busy: false });
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

  // Connect is possible once the checks ran, Ralphy is ready on the host, and
  // the daemon has a name or gets one.
  function ready(s) {
    if (!s.checks.length || s.failure) return false;
    const ralphy = s.checks.find((c) => c.id === "ralphy");
    if (ralphy && ralphy.status !== "pass") return false;
    const name = s.checks.find((c) => c.id === "name");
    if (name && name.status !== "pass" && name.status !== "fix") return false;
    return true;
  }

  // The host daemon has no name, and a name given here would fix it.
  function needsName(s) {
    const name = s.checks.find((c) => c.id === "name");
    return !!(
      name &&
      name.status === "copy" &&
      typeof name.command === "string" &&
      name.command.indexOf("ralphy host add ") === 0
    );
  }

  // How to turn on the SSH server, per system, when the connection failed
  // before the dialog could learn the host's system.
  function helpTabs() {
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

  const WRONG_ADDRESS = {
    text: "A refused connection can also mean a wrong address. Read the address on the host itself.",
    commands: [
      { name: "macOS", command: "ipconfig getifaddr en0" },
      { name: "Linux", command: "hostname -I" },
      { name: "Windows", command: "ipconfig" },
    ],
  };

  return {
    initial: initial,
    destination: destination,
    feed: feed,
    next: next,
    ready: ready,
    needsName: needsName,
    helpTabs: helpTabs,
    WRONG_ADDRESS: WRONG_ADDRESS,
  };
});
