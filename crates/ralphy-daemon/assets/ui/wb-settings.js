/* ---------------------------------------------------------------------------
   ralphy workbench shell — settings schema + security helpers (mock)

   Two things live here, kept out of app.js so the Alpine component stays small:

   1. WB_SETTINGS — a data-driven description of ralphy's real configuration
      surface (mirrors the persisted `ralphy config` keys plus the daemon /
      events / telegram / schedule stores). The Settings modal renders itself
      from this array, so adding a knob is a data edit, not markup. Every item
      carries a plain-English `help` line — the panel is meant to read easily.

      Sources in the real tree (for anyone wiring this to a backend):
        • persisted keys ...... crates/ralphy-cli/src/config.rs  (SUPPORTED_KEYS)
        • queue/branch/verify . crates/ralphy-core/src/settings.rs
        • claude.* ............ crates/ralphy-agent-claude/src/settings.rs
        • opencode.model ...... crates/ralphy-agent-opencode/src/lib.rs
        • events.* ............ crates/ralphy-cli/src/events/config.rs
        • telegram ............ crates/ralphy-cli/src/telegram/config.rs
        • daemon bind/port .... crates/ralphy-daemon/src/lib.rs
        • schedule ............ crates/ralphy-cli/src/schedule.rs

   2. wbQr() — turns a TOTP otpauth:// URI into an <img> QR, using the vendored
      qrcode-generator (no runtime CDN). Used by the Security panel.

   NB: nothing here persists or authenticates for real — the mock only reflects
   the shape of the settings and emits intents on the workbench:action seam.
--------------------------------------------------------------------------- */

// Reusable option sets.
const EFFORTS = ["low", "medium", "high"];
// Tri-state booleans in ralphy persist as unset / true / false — model them the
// same way so "leave at the built-in default" stays distinct from an explicit off.
const TRISTATE = ["unset", "on", "off"];

// `scope` groups the nav: "daemon" settings belong to this machine's background
// service (shared across every project); "project" settings are persisted per
// repo in <repo>/.ralphy/settings.json, so they follow whichever project is open.
window.WB_SETTINGS = [
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
      {
        key: "queue.label",
        label: "Eligible labels",
        type: "text",
        placeholder: "ready-for-agent, AFK",
        default: "ready-for-agent, AFK",
        help: "An open issue is worked when it carries any of these labels. Clearing this falls back to the built-in defaults.",
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
        help: "Run before closing an issue only when the plan has no ‘## Verify’ section. One command line, executed without a shell.",
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
  {
    id: "schedule",
    title: "Schedule",
    icon: "bi-clock-history",
    scope: "project",
    blurb: "Fire runs (and optional triage) on a native OS timer.",
    items: [
      {
        key: "schedule.every",
        label: "Cadence",
        type: "text",
        placeholder: "30m",
        default: "30m",
        help: "How often the scheduled job fires — a duration like 30m or 2h.",
      },
      {
        key: "schedule.with_triage",
        label: "Triage first",
        type: "tristate",
        default: "unset",
        help: "Run ‘triage --yes’ just before each scheduled run.",
      },
    ],
  },
];

window.WB_TRISTATE = TRISTATE;

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
