/* ---------------------------------------------------------------------------
   Settings → Devices (ADR-0074 D9, CONTEXT.md → *Device*, *Audit log*): the
   browsers that connected to this daemon, from where, and what each one did.

   The daemon owns the audit log. This section reads `GET /api/audit/devices`
   each time it opens, and `GET /api/audit/events` when a device is opened;
   the page keeps no copy.

   `rows`, `eventRows` and the name helpers are pure functions of a reply.
   `component` is the Alpine component `wbDevices`; it reads nothing from
   `shell()` (its `uses` list is empty).

   Load order: before `app.js` and before Alpine, on `index.html` only.
   --------------------------------------------------------------------------- */
window.WBDevices = (function () {
  const OS_NAMES = {
    windows: "Windows",
    macos: "macOS",
    ios: "iOS",
    ipados: "iPadOS",
    android: "Android",
    linux: "Linux",
    chromeos: "ChromeOS",
  };
  const BROWSER_NAMES = {
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
  const FORM_NAMES = { phone: "phone", tablet: "tablet", desktop: "computer" };
  const CHANGED_NAMES = {
    os: "system",
    browser: "browser",
    engine: "browser engine",
    form: "form",
    gpu: "graphics card",
    screen: "screen",
    time_zone: "time zone",
    other: "hardware",
  };

  function withVersion(name, version) {
    return version ? `${name} ${version}` : name;
  }

  // "Android 12 · Chrome 148 · phone", from the parts the device reported.
  function deviceName(d) {
    const parts = [];
    if (d.os) parts.push(withVersion(OS_NAMES[d.os] || "Other system", d.os_version));
    if (d.browser) parts.push(withVersion(BROWSER_NAMES[d.browser] || "Other browser", d.browser_version));
    if (d.form) parts.push(FORM_NAMES[d.form] || d.form);
    return parts.length ? parts.join(" · ") : "A device that sent no facts yet";
  }

  function rows(reply, when) {
    return (reply.devices || []).map((d) => ({
      device: d.device,
      this: d.this === true,
      name: deviceName(d),
      detail: [d.model, d.gpu, d.ip, `last seen ${when(d.last_seen)}`, d.events === 1 ? "1 event" : `${d.events} events`]
        .filter(Boolean)
        .join(" · "),
    }));
  }

  function eventLine(e) {
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
        return `Its device facts changed: ${(e.changed || []).map((c) => CHANGED_NAMES[c] || c).join(", ")}`;
      case "action":
        return `${e.method} ${e.path} (${e.status})`;
      default:
        return e.event;
    }
  }

  function eventRows(reply, when) {
    return (reply.events || []).map((e) => ({
      when: when(e.at),
      text: eventLine(e),
      ip: e.ip || (e.server && e.server.real_ip) || "",
    }));
  }

  function when(at) {
    const t = new Date(at);
    return Number.isNaN(t.getTime()) ? at : t.toLocaleString();
  }

  async function readJson(url) {
    const res = await fetch(url);
    const body = await res.json().catch(() => null);
    if (!res.ok) throw new Error((body && body.error) || `the daemon answered ${res.status}`);
    return body;
  }

  // The Alpine component. `loaded` is set only by a list the daemon served,
  // so a failed read never shows as "no devices" (ADR-0070 D3).
  function component() {
    return {
      uses: [],
      loaded: false,
      error: "",
      devices: [],
      open: null,
      events: [],
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
        } catch (e) {
          this.loaded = false;
          this.devices = [];
          this.error = `Could not read the devices: ${e.message}.`;
        }
      },
      async toggle(d) {
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
        } catch (e) {
          this.eventsError = `Could not read the activity of this device: ${e.message}.`;
        }
      },
    };
  }

  if (typeof document !== "undefined" && document.addEventListener) {
    document.addEventListener("alpine:init", () => window.Alpine.data("wbDevices", component));
  }

  return { rows, eventRows, deviceName, eventLine, component };
})();
