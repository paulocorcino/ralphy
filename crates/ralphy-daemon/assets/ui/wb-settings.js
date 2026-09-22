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
      "How consoles are restored when this page opens. Saved in this browser only.",
    items: [
      {
        key: "consoles.relaunch_on_load",
        label: "Relaunch agent consoles on load",
        type: "toggle",
        default: false,
        help: "Start a new agent CLI for each saved agent console on every page load. Plain shells always restore.",
      },
      {
        key: "consoles.key_bar",
        label: "Console key bar",
        type: "tristate",
        default: "unset",
        help: "Shows Esc, Tab, Ctrl, arrows and ^C under each console. Default: only on touch screens.",
      },
      {
        key: "consoles.startup_command",
        label: "Console startup command",
        type: "text",
        default: "",
        placeholder: "htop",
        help: "Adds a New console entry (Alt+Shift+9) that opens a console running this command in your login shell — htop, btop, lazygit… The console closes when the command exits. Leave empty to hide the entry.",
      },
    ],
  },
  {
    id: "daemon",
    title: "Daemon",
    icon: "bi-hdd-network",
    scope: "daemon",
    blurb: "The background service that hosts this UI. Shared by every project. Password and 2FA: account menu → Security.",
    items: [
      {
        key: "daemon.bind",
        label: "Bind address",
        type: "text",
        placeholder: "127.0.0.1",
        default: "127.0.0.1",
        help: "Address the daemon listens on. Any value other than 127.0.0.1 opens it to the network and requires an access token.",
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
    blurb: "Send run activity as CloudEvents to an external endpoint. Shared by every project.",
    items: [
      {
        key: "events.url",
        label: "Endpoint URL",
        type: "text",
        placeholder: "https://…",
        default: "",
        help: "HTTPS endpoint that receives the events. Empty: events off.",
      },
      {
        key: "events.token",
        label: "Bearer token",
        type: "password",
        default: "",
        // Shown as set/unset, never edited here: the bearer every event carries
        // to the sink is a credential a browser session must not be able to
        // redirect, so the daemon denies the key at the remote boundary
        // (dispatch.rs LOCAL_ONLY_KEYS) and would refuse the save.
        readonly: true,
        help: "Sent as ‘Authorization: Bearer …’ with every event. Read-only here; set it in a terminal on the host: ralphy config set events.token '…'",
      },
    ],
  },
  {
    id: "telegram",
    title: "Telegram",
    icon: "bi-send",
    scope: "daemon",
    blurb: "Post run summaries to a Telegram chat. Shared by every project.",
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
        help: "Chat to post to. Empty: set automatically the first time you /start the bot.",
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
        help: "Only pick issues assigned to this GitHub login. Empty: any issue. @me: yourself.",
      },
      {
        key: "queue.trust_all_comments",
        label: "Read every issue comment",
        type: "toggle",
        default: false,
        help: "Feed comments from any GitHub account to the agent. Off: only owners, members and collaborators are read (a labelled issue on a public repo is otherwise a prompt anyone can append to).",
      },
    ],
  },
  {
    id: "branch",
    title: "Branch & Git",
    icon: "bi-git",
    scope: "project",
    blurb: "Where each run commits. Both modes need a clean working tree.",
    items: [
      {
        key: "base_branch",
        label: "Base branch",
        type: "text",
        placeholder: "origin/main",
        default: "origin/main",
        help: "Branch or commit a new run branch starts from (‘new branch’ mode only).",
      },
      {
        key: "branch_mode",
        label: "Branch mode",
        type: "select",
        options: ["new", "current"],
        default: "new",
        help: "new: create an afk/run-… branch. current: commit on the current branch.",
      },
    ],
  },
  {
    id: "files",
    title: "Files",
    icon: "bi-file-earmark-text",
    scope: "project",
    blurb: "How the workbench reads this project's text files.",
    items: [
      {
        key: "files.encoding",
        label: "Fallback encoding",
        type: "select",
        // WHATWG labels the daemon's decoder names (ADR-0036 amendment
        // 2026-09-22). UTF-8 and UTF-16 are recognised before this applies, so
        // they are not choices here.
        options: [
          "windows-1252",
          "iso-8859-2",
          "iso-8859-15",
          "windows-1250",
          "windows-1251",
          "koi8-r",
          "shift_jis",
          "euc-jp",
          "gbk",
          "big5",
          "euc-kr",
        ],
        default: "windows-1252",
        help: "How a file that is not UTF-8 or UTF-16 is read and saved. A file's tab can still reopen it with another encoding.",
      },
    ],
  },
  {
    id: "verify",
    title: "Verify gate",
    icon: "bi-shield-check",
    scope: "project",
    blurb: "The check ralphy runs before closing an issue.",
    items: [
      {
        key: "verify.command",
        label: "Fallback verify command",
        type: "text",
        placeholder: "e.g. cargo test",
        default: "",
        // Shown, never edited here: the value becomes argv[0] of a child a
        // LATER run spawns, so the daemon denies it at the remote boundary
        // (dispatch.rs LOCAL_ONLY_KEYS) and would refuse the save. Reading it
        // is the point — a gate you cannot see is worse than one you cannot
        // edit from a browser.
        readonly: true,
        help: "Runs before an issue is closed, when the plan has no ‘## Verify’ section. Read-only here; set it in a terminal: ralphy config set verify.command '…'",
      },
      {
        key: "verify.require_verify_gate",
        label: "Require a verify gate",
        type: "tristate",
        default: "unset",
        help: "When on, an issue with no verify gate is held for review instead of closed.",
      },
    ],
  },
  {
    id: "claude",
    title: "Claude",
    icon: "bi-robot",
    scope: "project",
    blurb: "Model and effort for the Claude adapter. Default: ralphy's built-in choice.",
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
        help: "Reasoning effort for the planner.",
      },
      {
        key: "claude.default_exec_model",
        label: "Default execution model",
        type: "select",
        options: ["sonnet", "opus"],
        default: "sonnet",
        help: "Model that executes the plan, unless the plan names one.",
      },
      {
        key: "claude.exec_effort",
        label: "Execution effort",
        type: "select",
        options: EFFORTS,
        default: "medium",
        help: "Reasoning effort for the executor.",
      },
      {
        key: "claude.max_minutes_per_issue",
        label: "Minutes per issue",
        type: "number",
        default: 60,
        min: 0,
        help: "Time limit per issue, in minutes. 0: no limit.",
      },
      {
        key: "claude.console_name",
        label: "Name the consoles ralphy opens",
        type: "toggle",
        default: false,
        help: "Name Claude sessions wb-<repo>-<hex> so the roster shows their project. Off: Claude picks the name.",
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
        help: "Model id for OpenCode. Empty: OpenCode's default.",
      },
    ],
  },
  {
    id: "remote",
    title: "Remote control",
    icon: "bi-phone",
    scope: "project",
    blurb: "Follow and join runs from Claude's mobile app.",
    items: [
      {
        key: "remote_control",
        label: "Enable remote control",
        type: "tristate",
        default: "unset",
        help: "Let Claude's mobile Remote Control follow and join a run. Claude only.",
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
