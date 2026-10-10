// The PER-CLIENT view (issue #339, ADR-0051 §8): the viewport offset, the open
// file tabs, and the restore preference, in this browser profile only.
//
// This is the ONE module in the workbench allowed to touch `localStorage`, and
// it holds ONE key. ADR-0050 §3 dropped the browser desk store — that rejection
// was of a second copy of the DESK (windows, and later fences), authoritative in
// no mode; the desk stays daemon-owned. The view is different state with a
// different lifetime: shared, one operator's panning would drag the other's
// view, which is the flapping defect translated to the canvas. So it lives here,
// per profile, and NOTHING about the layout may join it — `wb_desk_327.py`
// scenario 5 and `shell_stores_only_the_view_in_the_browser` assert exactly that.
//
// `relaunch` joins them for the same reason and NOT as an exception: it is a
// preference about what THIS browser is allowed to start, not a record of what
// the desk holds. Per-client is also the only lifetime that is safe — daemon-
// wide, every tab pointed at the same desk would relaunch the same consoles,
// and two tabs would spend the quota twice.
//
// The column grid and its direction (ADR-0051 §8, columns and rows amendments)
// are per-client for the same reason: window ids only, and never a rect or a
// field of the desk.
//
// Three writers share the key (the console's offset, the shell's tabs, the
// settings toggle), which is why `patch` is read-modify-write: any one of them
// writing the whole record would clobber the others'.
import { isNullableString, isOptionalString, isRecord } from "./wb-api.ts";

/** A stored file tab: what `persistView` in `app.ts` writes. */
export type StoredTab = { project: string; path: string; title?: string; kind: string; checkout?: string | null };

// A record written by an older build may hold a tab of another shape: such a
// tab is not restored.
function isStoredTab(v: unknown): v is StoredTab {
  return (
    isRecord(v) &&
    typeof v.project === "string" &&
    typeof v.path === "string" &&
    isOptionalString(v.title) &&
    typeof v.kind === "string" &&
    (v.checkout === undefined || isNullableString(v.checkout))
  );
}

export const WBView = (function () {
  const KEY = "wb.view.v1";

  // A disabled store (private mode, a blocked third-party context, a full quota)
  // must degrade to "nothing stored", never throw into a caller's boot path —
  // both the landing and the tab restore run before anything else is on screen.
  function read() {
    try {
      const raw = localStorage.getItem(KEY);
      if (!raw) return null;
      const parsed: unknown = JSON.parse(raw);
      if (!isRecord(parsed)) return null;
      // A record from a future (or corrupt) version is not ours to interpret.
      if (parsed.v !== 1) return null;
      // `command` was the free console's stored startup command. The command is
      // now typed per console in the Consoles menu, so the field is dropped
      // here, and the next `patch` writes the record without it.
      const { command: _dropped, ...stored } = parsed;
      // NORMALISED, not merely returned: both callers run before anything is on
      // screen, and `for (const t of stored.tabs)` on a `tabs` that is a number
      // throws straight into the boot path — the failure mode this whole
      // try/catch exists to prevent, one level down.
      return {
        ...stored,
        tabs: Array.isArray(parsed.tabs) ? parsed.tabs.filter(isStoredTab) : [],
        // The tab that was active: a file tab's id, or "consoles".
        active: typeof parsed.active === "string" ? parsed.active : null,
        // Read STRICTLY: anything that is not a stored `true` means "do not
        // launch". A truthy coercion here would turn a corrupt or half-written
        // record into permission to spawn a vendor CLI per saved console.
        relaunch: parsed.relaunch === true,
        // The console key bar's mode. Only the two EXPLICIT choices are stored;
        // anything else — absent, corrupt, a stale spelling — is auto, which is
        // what the pure `keyBarVisible` reads a null as.
        keys: parsed.keys === "on" || parsed.keys === "off" ? parsed.keys : null,
        // The terminal font size, in px. Clamped to the same range the buttons
        // step through: a hand-edited 400 would paint one glyph per console.
        font:
          typeof parsed.font === "number" && Number.isInteger(parsed.font) && parsed.font >= 10 && parsed.font <= 28
            ? parsed.font
            : null,
        off: isRecord(parsed.off) ? parsed.off : null,
        // The secondary pane (ADR-0037 §3c): a pin names a file the way `tabs`
        // does; a mirror names nothing. The ratio is the left column's share,
        // held to the range the divider drag can produce. Anything else is
        // "no slot" — `WBSplit.fromStored` folds the survivor against the tabs.
        split: splitOf(parsed.split),
        // The column grid (ADR-0051 §8, columns and rows amendments): window
        // ids only, as a list of columns that are lists of ids, or the flat
        // list stored before rows. `WBColumns.fromStored` folds both and
        // checks them against the desk on restore.
        columns: Array.isArray(parsed.columns)
          ? parsed.columns.flatMap((c: unknown): (string | string[])[] => {
              if (typeof c === "string") return [c];
              return Array.isArray(c) ? [c.filter((s): s is string => typeof s === "string")] : [];
            })
          : null,
        // Where "Slice" opens, as last picked in this browser.
        columnDir: parsed.columnDir === "right" || parsed.columnDir === "down" ? parsed.columnDir : null,
      };
    } catch {
      return null;
    }
  }

  function splitOf(raw: unknown) {
    if (!isRecord(raw)) return null;
    const ratio = typeof raw.ratio === "number" && raw.ratio >= 0.2 && raw.ratio <= 0.8 ? raw.ratio : null;
    if (raw.kind === "mirror") return { kind: "mirror", ratio };
    if (raw.kind === "pin" && typeof raw.project === "string" && typeof raw.path === "string") {
      return {
        kind: "pin",
        project: raw.project,
        path: raw.path,
        checkout: typeof raw.checkout === "string" ? raw.checkout : null,
        ratio,
      };
    }
    return null;
  }

  // A writer passes the value it holds: `read` checks each field on the way back.
  function patch(part: { [K in keyof NonNullable<ReturnType<typeof read>>]?: unknown }) {
    try {
      const next = { ...read(), ...part, v: 1 };
      localStorage.setItem(KEY, JSON.stringify(next));
      return next;
    } catch {
      return null;
    }
  }

  return { KEY, read, patch };
})();

// `app.ts`, the Settings dialog and browser checks read this name (ADR-0075 D9).
if (typeof window !== "undefined") window.WBView = WBView;

declare global {
  interface Window {
    WBView: typeof WBView;
  }
}
