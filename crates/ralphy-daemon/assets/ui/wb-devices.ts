/* ---------------------------------------------------------------------------
   Settings → Devices (ADR-0074 D9, CONTEXT.md → *Device*, *Audit log*): the
   browsers that connected to this daemon, from where, and what each one did.

   The daemon owns the audit log. This section reads `GET /api/audit/devices`
   each time it opens, and `GET /api/audit/events` when a device is opened;
   the page keeps no copy.

   `rows`, `eventRows` and the name helpers are pure functions of a reply.
   `devices` is the Alpine component `wbDevices`; it reads nothing from
   `shell()` (its `uses` list is empty).

   `main.ts` registers it as `wbDevices` (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";

/** A device of `GET /api/audit/devices`. */
type Device = any;
/** A line of `GET /api/audit/events`. */
type AuditEvent = any;

const OS_NAMES: Record<string, string> = {
  windows: "Windows",
  macos: "macOS",
  ios: "iOS",
  ipados: "iPadOS",
  android: "Android",
  linux: "Linux",
  chromeos: "ChromeOS",
};
const BROWSER_NAMES: Record<string, string> = {
  safari: "Safari",
  chrome: "Chrome",
  chromium: "Chromium",
  edge: "Edge",
  firefox: "Firefox",
  opera: "Opera",
  samsung: "Samsung Internet",
  webview: "an app's browser view",
  in_app: "a social app's browser",
};
const FORM_NAMES: Record<string, string> = { phone: "phone", tablet: "tablet", desktop: "computer" };
const CHANGED_NAMES: Record<string, string> = {
  os: "system",
  browser: "browser",
  engine: "browser engine",
  form: "form",
  gpu: "graphics card",
  screen: "screen",
  time_zone: "time zone",
  other: "hardware",
};

// The requests that change state, as a person would say them. A request
// not named here shows its method and path.
const ACTION_NAMES: Record<string, string> = {
  "PUT /api/desk": "Saved the desk",
  "POST /api/desk/new": "Started a new desk",
  "POST /api/sessions/close": "Closed a console",
  "POST /api/release/update": "Started an update",
  "POST /api/security/password": "Changed the password",
  "POST /api/security/token/remint": "Made a new access token",
  "POST /api/security/totp/enroll": "Started to link an authenticator app",
  "POST /api/security/totp/confirm": "Linked an authenticator app",
  "POST /api/security/totp/revoke": "Removed the authenticator app",
};

function actionLine(e: AuditEvent) {
  const name = ACTION_NAMES[`${e.method} ${e.path}`];
  if (!name) return `${e.method} ${e.path} (${e.status})`;
  return e.status >= 200 && e.status < 300 ? name : `${name} (refused: ${e.status})`;
}

function withVersion(name: string, version?: string) {
  return version ? `${name} ${version}` : name;
}

// "Android 12 · Chrome 148 · phone", from the parts the device reported.
export function deviceName(d: Device) {
  const parts = [];
  if (d.os) parts.push(withVersion(OS_NAMES[d.os] || "Other system", d.os_version));
  if (d.browser) parts.push(withVersion(BROWSER_NAMES[d.browser] || "Other browser", d.browser_version));
  if (d.form) parts.push(FORM_NAMES[d.form] || d.form);
  return parts.length ? parts.join(" · ") : "A device that sent no facts yet";
}

export function rows(reply: { devices?: Device[] }, when: (at: any) => string) {
  return (reply.devices || []).map((d) => ({
    device: d.device,
    this: d.this === true,
    name: deviceName(d),
    detail: [d.model, d.gpu, d.ip, `last seen ${when(d.last_seen)}`, d.events === 1 ? "1 event" : `${d.events} events`]
      .filter(Boolean)
      .join(" · "),
  }));
}

export function eventLine(e: AuditEvent) {
  // The daemon names a registered project; a peer's keeps its recorded value.
  const repo = e.repo_name || e.repo;
  switch (e.event) {
    case "login_ok":
      return "Signed in";
    case "login_failed":
      return e.reason === "throttled"
        ? "Sign-in refused: too many attempts"
        : "Sign-in failed: wrong code or password";
    case "logout":
      return "Signed out";
    case "device_facts":
      return "Reported its device facts";
    case "device_profile_changed":
      return `Its device facts changed: ${(e.changed || []).map((c: string) => CHANGED_NAMES[c] || c).join(", ")}`;
    case "action":
      return actionLine(e);
    case "command":
      return repo ? `Command ${e.verb} in ${repo}` : `Command ${e.verb}`;
    case "console_launch": {
      const what = !e.agent || e.agent === "console" ? "a console" : e.agent;
      return repo ? `Opened ${what} in ${repo}` : `Opened ${what}`;
    }
    case "console_takeover":
      return repo ? `Took over a console in ${repo}` : "Took over a console";
    default:
      return e.event;
  }
}

// Newest first. Lines in a row that say the same thing from the same
// address are one row with a count, at the time of the newest; the log
// file keeps each line.
export function eventRows(reply: { events?: AuditEvent[] }, when: (at: any) => string) {
  const out: { when: string; text: string; ip: string; line: string; count: number }[] = [];
  for (const e of reply.events || []) {
    const text = eventLine(e);
    const ip = e.ip || (e.server && e.server.real_ip) || "";
    const last = out[out.length - 1];
    if (last && last.line === text && last.ip === ip) {
      last.count += 1;
      last.text = `${text} · ${last.count} times`;
      continue;
    }
    out.push({ when: when(e.at), text, ip, line: text, count: 1 });
  }
  return out.map(({ when, text, ip }) => ({ when, text, ip }));
}

function when(at: string) {
  const t = new Date(at);
  return Number.isNaN(t.getTime()) ? at : t.toLocaleString();
}

async function readJson(url: string) {
  const res = await fetch(url);
  const body = await res.json().catch(() => null);
  if (!res.ok) throw new Error((body && body.error) || `the daemon answered ${res.status}`);
  return body;
}

// The Alpine component. `loaded` is set only by a list the daemon served,
// so a failed read never shows as "no devices" (ADR-0070 D3).
export function devices() {
  return component([], {
    loaded: false,
    error: "",
    devices: [] as ReturnType<typeof rows>,
    open: null as string | null,
    events: [] as ReturnType<typeof eventRows>,
    raw: "",
    eventsError: "",
    init() {
      this.load();
    },
    async load() {
      this.error = "";
      try {
        this.devices = rows(await readJson("/api/audit/devices"), when);
        this.loaded = true;
      } catch (e: any) {
        this.loaded = false;
        this.devices = [];
        this.error = `Could not read the devices: ${e.message}.`;
      }
    },
    async toggle(d: { device: string }) {
      if (this.open === d.device) {
        this.open = null;
        return;
      }
      this.open = d.device;
      this.events = [];
      this.raw = "";
      this.eventsError = "";
      try {
        const reply = await readJson(`/api/audit/events?device=${encodeURIComponent(d.device)}&limit=50`);
        this.events = eventRows(reply, when);
        this.raw = JSON.stringify(reply.events, null, 2);
      } catch (e: any) {
        this.eventsError = `Could not read the activity of this device: ${e.message}.`;
      }
    },
  });
}
