/* ---------------------------------------------------------------------------
   ralphy workbench shell — the settings schema

   WB_SETTINGS — a data-driven description of ralphy's real configuration
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
// store (wb-client-view.ts) and never sent to the daemon.
/** The three stores a setting can live in; see the paragraph above. */
type Scope = "daemon" | "project" | "client";
/** One control of a section. */
export type Item = {
  key: string;
  label: string;
  type: string;
  help: string;
  default: unknown;
  placeholder?: string;
  options?: readonly string[];
  readonly?: boolean;
  min?: number;
  max?: number;
};
/** One nav section of the Settings dialog. */
export type Section = {
  id: string;
  title: string;
  icon: string;
  scope: Scope;
  blurb: string;
  items: Item[];
};

export const WB_SETTINGS: Section[] = [
  {
    id: "consoles",
    title: "Consoles",
    icon: "bi-window-stack",
    scope: "client",
    blurb:
      "How consoles look, and how they are restored when this page opens. Saved in this browser " +
      "only.",
    items: [
      {
        key: "consoles.font_size",
        label: "Console text size",
        type: "number",
        default: 15,
        min: 10,
        max: 28,
        help:
          "The size of the text in every console, in pixels. Smaller text puts more characters " +
          "in each column.",
      },
      {
        key: "consoles.relaunch_on_load",
        label: "Relaunch agent consoles on load",
        type: "toggle",
        default: false,
        help:
          "Start a new agent CLI for each saved agent console on every page load. Plain shells " +
          "are always restored.",
      },
      {
        key: "consoles.key_bar",
        label: "Console key bar",
        type: "tristate",
        default: "unset",
        help:
          "Shows Esc, Tab, Shift, Ctrl, Enter, the arrows and ^C under each console. By default, " +
          "only on touch screens.",
      },
    ],
  },
  {
    id: "notifications",
    title: "Notifications",
    icon: "bi-bell",
    scope: "client",
    blurb: "Notifications for the requests that wait for you. Saved in this browser only.",
    items: [
      {
        key: "notifications.browser",
        label: "Notify me in this browser",
        type: "toggle",
        default: false,
        help:
          "Shows a notification when a new request waits for you and this tab is hidden. " +
          "The browser asks for permission the first time.",
      },
    ],
  },
  {
    id: "daemon",
    title: "Daemon",
    icon: "bi-hdd-network",
    scope: "daemon",
    blurb:
      "The background service that hosts this UI. Shared by every project. Set the password and " +
      "two-factor in the account menu, under “Security settings”.",
    items: [
      {
        key: "daemon.bind",
        label: "Bind address",
        type: "text",
        placeholder: "127.0.0.1",
        default: "127.0.0.1",
        help:
          "Address the daemon listens on. Any value other than 127.0.0.1 opens it to the network " +
          "and requires an access token.",
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
        help: "HTTPS endpoint that receives the events. Leave empty to turn events off.",
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
        help:
          "Sent as ‘Authorization: Bearer …’ with every event. Read-only here. Set it in a " +
          "terminal on the host: ralphy config set events.token '…'",
      },
    ],
  },
  // No items: the section has its own body (wb-telegram-setup.ts). The page
  // sends the bot token once and never reads it back.
  {
    id: "telegram",
    title: "Telegram",
    icon: "bi-send",
    scope: "daemon",
    blurb:
      "A message to your Telegram chat when an agent waits for you and you are away. " +
      "This daemon sends the messages of its peers too, so set the bot up here only.",
    items: [],
  },
  // No items: the section has its own body in index.html, and the list it
  // shows is read from the daemon (wb-desk-history.ts).
  {
    id: "desk-history",
    title: "Desk history",
    icon: "bi-clock-history",
    scope: "daemon",
    blurb:
      "The last 50 layouts of your consoles, fences and notes. Restore one, or save it as a file.",
    items: [],
  },
  // No items: the section has its own body in index.html, an Alpine
  // component that reads the audit log (wb-devices.ts).
  {
    id: "devices",
    title: "Devices",
    icon: "bi-phone",
    scope: "daemon",
    blurb: "The browsers that connected to this daemon, from where, and what each one changed.",
    items: [],
  },
  {
    id: "queue",
    title: "Queue",
    icon: "bi-list-check",
    scope: "project",
    blurb: "How Ralphy decides which issues to pick up and in what order.",
    items: [
      {
        key: "queue.assignee",
        label: "Assignee filter",
        type: "text",
        placeholder: "GitHub login or @me",
        default: "",
        help:
          "Only pick issues assigned to this GitHub login. Leave empty for any issue. Use @me " +
          "for yourself.",
      },
      {
        key: "queue.trust_all_comments",
        label: "Read every issue comment",
        type: "toggle",
        default: false,
        // Shown, never edited here: on, a stranger's comment reaches the
        // agent's prompt, so the daemon denies the key at the remote boundary
        // (dispatch.rs LOCAL_ONLY_KEYS) and would refuse the save.
        readonly: true,
        help:
          "Send comments from any GitHub account to the agent. When off, only comments from " +
          "owners, members and collaborators are read. When on, anyone can add text to the " +
          "prompt of a labelled issue on a public repository. Read-only here. Change it in a " +
          "terminal: ralphy config set queue.trust_all_comments true",
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
        help:
          "With “new”, each run creates an afk/run-… branch. With “current”, runs commit on the " +
          "current branch.",
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
        help:
          "How a file that is not UTF-8 or UTF-16 is read and saved. A file's tab can still " +
          "reopen it with another encoding.",
      },
    ],
  },
  {
    id: "verify",
    title: "Verify gate",
    icon: "bi-shield-check",
    scope: "project",
    blurb: "The check Ralphy runs before closing an issue.",
    items: [
      {
        key: "verify.command",
        label: "Fallback verify command",
        type: "text",
        placeholder: "For example, cargo test",
        default: "",
        // Shown, never edited here: the value becomes argv[0] of a child a
        // LATER run spawns, so the daemon denies it at the remote boundary
        // (dispatch.rs LOCAL_ONLY_KEYS) and would refuse the save. Reading it
        // is the point — a gate you cannot see is worse than one you cannot
        // edit from a browser.
        readonly: true,
        help:
          "Runs before an issue is closed, when the plan has no ‘## Verify’ section. Read-only " +
          "here. Set it in a terminal: ralphy config set verify.command '…'",
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
    blurb: "Model and effort for the Claude adapter. Default: Ralphy's built-in choice.",
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
        help: "Time limit per issue, in minutes. Use 0 for no limit.",
      },
      {
        key: "claude.console_name",
        label: "Give Claude the console name",
        type: "toggle",
        default: false,
        help:
          "Start Claude with the console name as its session name, such as wb-fincal-1. Other " +
          "Claude sessions use that name to send it messages. When off, Claude picks the name.",
      },
      {
        key: "claude.console_classic_screen",
        label: "Keep Claude's text in the console",
        type: "toggle",
        default: false,
        help:
          "Start Claude in its classic view. The conversation stays in the console's text, with " +
          "a scrollbar. When off, Claude follows its own settings; its full-screen view scrolls " +
          "by itself and shows no scrollbar. Applies to consoles opened after the change.",
      },
      {
        key: "claude.run_without_guard_proof_i_understand_the_risk",
        label: "Run without the hook check",
        type: "toggle",
        default: false,
        // Shown, never edited here: on, a session goes on when its guard may
        // not run, so the daemon denies the key at the remote boundary
        // (dispatch.rs LOCAL_ONLY_KEYS) and would refuse the save.
        readonly: true,
        help:
          "Let a Claude session start even when its hooks cannot be checked. The guard may then " +
          "not run. Each such session writes a warning to the run. Read-only here. Change it in " +
          "a terminal: ralphy config set claude.run_without_guard_proof_i_understand_the_risk " +
          "true",
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
        placeholder: "Leave empty to let OpenCode choose",
        default: "",
        help: "The OpenCode model to use. Leave empty for the OpenCode default.",
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
        help: "Let “Remote Control” in the Claude mobile app follow and join a run. Claude only.",
      },
    ],
  },
];

export const WB_TRISTATE = TRISTATE;
