/* ---------------------------------------------------------------------------
   ralphy workbench shell — settings schema + security helpers

   Two things live here, kept out of app.js so the Alpine component stays small:

   1. WB_SETTINGS — a data-driven description of ralphy's real configuration
      surface (mirrors the persisted `ralphy config` keys plus the daemon /
      events / telegram stores). The Settings modal renders itself
      from this array, so adding a knob is a data edit, not markup. Every item
      carries a plain-English `help` line — the panel is meant to read easily.

      A `scope: "project"` item is an INTENT ON THE CLI: the panel saves it with
      `config.set`, which the daemon relays to `ralphy config set`, which
      refuses any key outside its `SUPPORTED_KEYS`. So an item here that names
      no persisted key is a control that renders, accepts a click, and comes
      back refused — which is what the Schedule section and an "Eligible labels"
      field did until they were removed. `readonly: true` is the declaration for
      the one remaining case: a key the panel can READ but the daemon refuses to
      write. A Rust gate holds both halves —
      `every_settable_key_the_panel_offers_is_a_key_the_cli_accepts`.

      Sources in the real tree (for anyone wiring this to a backend):
        • persisted keys ...... crates/ralphy-cli/src/config.rs  (SUPPORTED_KEYS)
        • queue/branch/verify . crates/ralphy-core/src/settings.rs
        • claude.* ............ crates/ralphy-agent-claude/src/settings.rs
        • opencode.model ...... crates/ralphy-agent-opencode/src/lib.rs
        • events.* ............ crates/ralphy-cli/src/events/config.rs
        • telegram ............ crates/ralphy-cli/src/telegram/config.rs
        • daemon bind/port .... crates/ralphy-daemon/src/lib.rs

   2. wbQr() — turns a TOTP otpauth:// URI into an <img> QR, using the vendored
      qrcode-generator (no runtime CDN). Used by the Security panel.

   NB: in daemon mode, changes persist and authenticate for real via the
   daemon's `config.set`/`config.unset` and auth verbs; this module only
   describes the shape of the settings and emits intents on the
   workbench:action seam.
--------------------------------------------------------------------------- */

// Reusable option sets.
const EFFORTS = ["low", "medium", "high"];
// Tri-state booleans in ralphy persist as unset / true / false — model them the
// same way so "leave at the built-in default" stays distinct from an explicit off.
const TRISTATE = ["unset", "on", "off"];

// `scope` groups the nav: "daemon" settings belong to this machine's background
// service (shared across every project); "project" settings are persisted per
// repo in <repo>/.ralphy/settings.json, so they follow whichever project is
// open; "client" settings are this BROWSER profile's own, held in the view
// store (wb-view.js) and never sent to the daemon.
window.WB_SETTINGS = [
  {
    id: "consoles",
    title: "Consoles",
    icon: "bi-window-stack",
    scope: "client",
    blurb:
      "How the console plane comes back when you open this page. Stored in this browser profile — another browser, or another machine, decides for itself.",
    items: [
      {
        key: "consoles.relaunch_on_load",
        label: "Relaunch agent consoles on load",
        type: "toggle",
        default: false,
        help: "A fresh launch, not a reconnect: every load starts one vendor CLI per saved agent console. Plain shells come back on their own either way.",
      },
      {
        key: "consoles.key_bar",
        label: "Console key bar",
        type: "tristate",
        default: "unset",
        help: "A row of Esc / Tab / Ctrl / arrows / ^C under each console, plus copy and text size — the keys a tablet's on-screen keyboard has none of. Left at the default it appears only on a device with a touch screen.",
      },
    ],
  },
  {
    id: "daemon",
    title: "Daemon",
    icon: "bi-hdd-network",
    scope: "daemon",
    blurb: "The background service that hosts this UI. Machine-wide — the same for every project. Password and 2FA live under the account menu → Security.",
    items: [
      {
        key: "daemon.bind",
        label: "Bind address",
        type: "text",
        placeholder: "127.0.0.1",
        default: "127.0.0.1",
        help: "Interface the daemon listens on. Anything other than 127.0.0.1 exposes it to the network and forces an access token.",
      },
      {
        key: "daemon.port",
        label: "Port",
        type: "number",
        default: 7257,
        min: 1,
        max: 65535,
        help: "TCP port for the local HTTP UI.",
      },
    ],
  },
  {
    id: "events",
    title: "Events sink",
    icon: "bi-broadcast",
    scope: "daemon",
    blurb: "Stream run activity as CloudEvents to an external endpoint. Global — shared across all repos.",
    items: [
      {
        key: "events.url",
        label: "Endpoint URL",
        type: "text",
        placeholder: "https://…",
        default: "",
        help: "HTTPS endpoint ralphy POSTs CloudEvents to. Empty turns the event stream off entirely.",
      },
      {
        key: "events.token",
        label: "Bearer token",
        type: "password",
        default: "",
        help: "Sent as ‘Authorization: Bearer …’ with every event POST. Stored masked.",
      },
    ],
  },
  {
    id: "telegram",
    title: "Telegram",
    icon: "bi-send",
    scope: "daemon",
    blurb: "Post run cards to a Telegram chat. Global config, owner-only on disk.",
    items: [
      {
        key: "telegram.token",
        label: "Bot token",
        type: "password",
        default: "",
        help: "The token BotFather gave your bot.",
      },
      {
        key: "telegram.chat_id",
        label: "Chat id",
        type: "text",
        placeholder: "auto-detected from /start",
        default: "",
        help: "Chat the notifier posts to. Left empty, ralphy learns it the first time you /start the bot.",
      },
    ],
  },
  {
    id: "queue",
    title: "Queue",
    icon: "bi-list-check",
    scope: "project",
    blurb: "How ralphy decides which issues to pick up and in what order.",
    items: [
      {
        key: "queue.assignee",
        label: "Assignee filter",
        type: "text",
        placeholder: "e.g. @me or a github login",
        default: "",
        help: "Only queue issues assigned to this GitHub login. Leave empty to consider every eligible issue. Use @me for yourself.",
      },
    ],
  },
  {
    id: "branch",
    title: "Branch & Git",
    icon: "bi-git",
    scope: "project",
    blurb: "Where each run's work lands. Both modes require a clean working tree.",
    items: [
      {
        key: "base_branch",
        label: "Base branch",
        type: "text",
        placeholder: "origin/main",
        default: "origin/main",
        help: "The commit a fresh run branch is cut from (only in ‘new branch’ mode).",
      },
      {
        key: "branch_mode",
        label: "Branch mode",
        type: "select",
        options: ["new", "current"],
        default: "new",
        help: "new: cut a fresh afk/run-… branch for the work. current: commit straight onto the branch you're already on.",
      },
    ],
  },
  {
    id: "verify",
    title: "Verify gate",
    icon: "bi-shield-check",
    scope: "project",
    blurb: "The check ralphy runs before it's allowed to close an issue.",
    items: [
      {
        key: "verify.command",
        label: "Fallback verify command",
        type: "text",
        placeholder: "e.g. cargo test",
        default: "",
        // Shown, never edited here: the value becomes argv[0] of a child a
        // LATER run spawns, so the daemon denies it at the remote boundary
        // (dispatch.rs EXEC_ADJACENT_KEYS) and would refuse the save. Reading it
        // is the point — a gate you cannot see is worse than one you cannot
        // edit from a browser.
        readonly: true,
        help: "Run before closing an issue only when the plan has no ‘## Verify’ section. One command line, executed without a shell. Read-only here — its value names a program a later run executes, so it is set from a terminal in the repo: ralphy config set verify.command '…'",
      },
      {
        key: "verify.require_verify_gate",
        label: "Require a verify gate",
        type: "tristate",
        default: "unset",
        help: "When on, an issue that ends up with no gate at all is parked for a human instead of closing on the agent's own word.",
      },
    ],
  },
  {
    id: "claude",
    title: "Claude",
    icon: "bi-robot",
    scope: "project",
    blurb: "Model and effort for the Claude adapter. Leave a field at its default to use ralphy's built-in choice.",
    items: [
      {
        key: "claude.plan_model",
        label: "Planning model",
        type: "select",
        options: ["opus", "sonnet"],
        default: "opus",
        help: "Which model writes the plan.",
      },
      {
        key: "claude.plan_effort",
        label: "Planning effort",
        type: "select",
        options: EFFORTS,
        default: "medium",
        help: "How hard the planner is allowed to think.",
      },
      {
        key: "claude.default_exec_model",
        label: "Default execution model",
        type: "select",
        options: ["sonnet", "opus"],
        default: "sonnet",
        help: "Model that executes the plan — used only when the plan doesn't name one itself.",
      },
      {
        key: "claude.exec_effort",
        label: "Execution effort",
        type: "select",
        options: EFFORTS,
        default: "medium",
        help: "How hard the executor is allowed to think.",
      },
      {
        key: "claude.max_minutes_per_issue",
        label: "Minutes per issue",
        type: "number",
        default: 60,
        min: 0,
        help: "Wall-clock cap for a single issue, in minutes. 0 means no cap — only the overall run deadline applies.",
      },
      {
        key: "claude.console_name",
        label: "Name the consoles ralphy opens",
        type: "toggle",
        default: false,
        help: "Give a Claude console opened here the address wb-<repo>-<hex>, so a roster row says which project it belongs to. Left off, Claude names the session itself.",
      },
    ],
  },
  {
    id: "opencode",
    title: "OpenCode",
    icon: "bi-terminal",
    scope: "project",
    blurb: "Settings for the OpenCode adapter.",
    items: [
      {
        key: "opencode.model",
        label: "Execution model",
        type: "text",
        placeholder: "leave empty to let OpenCode choose",
        default: "",
        help: "Model id OpenCode runs with. Empty means OpenCode resolves its own default.",
      },
    ],
  },
  {
    id: "remote",
    title: "Remote control",
    icon: "bi-phone",
    scope: "project",
    blurb: "Follow and step into runs from Claude's mobile app.",
    items: [
      {
        key: "remote_control",
        label: "Enable remote control",
        type: "tristate",
        default: "unset",
        help: "Let Claude's mobile Remote Control follow and intervene in a run. Codex and OpenCode ignore this.",
      },
    ],
  },
];

window.WB_TRISTATE = TRISTATE;

// The keys this BROWSER owns. Derived from the schema rather than listed, so a
// new client-scoped item cannot be added and then routed to `config.set` by an
// edit that forgot about it.
window.wbClientKeys = function () {
  const out = new Set();
  for (const sec of window.WB_SETTINGS) {
    if (sec.scope === "client") for (const it of sec.items) out.add(it.key);
  }
  return out;
};

// Seed a flat {key: default} map the Alpine component keeps its live values in.
window.wbSettingsDefaults = function () {
  const out = {};
  for (const sec of window.WB_SETTINGS) {
    for (const it of sec.items) out[it.key] = it.default;
  }
  return out;
};

// Render an otpauth:// URI to an <img> QR (vendored qrcode-generator, offline).
window.wbQr = function (uri) {
  try {
    const qr = qrcode(0, "M"); // type 0 = auto-size, error-correction M
    qr.addData(uri);
    qr.make();
    return qr.createImgTag(4, 8); // cellSize 4px, margin 8 modules
  } catch (e) {
    return '<div class="qr-fail">could not render QR</div>';
  }
};
