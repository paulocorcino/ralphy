// The "Desk history" section of Settings (ADR-0050 amendment 2026-10-04, desk
// history): the rows of the version list, the name of a downloaded file, and
// the check of an uploaded one.
//
// PURE: data in, data out. No DOM, no fetch, no Alpine. The shell calls
// `/api/desk/history`, hands the reply to `rows`, and sends what `parseUpload`
// accepts back to the same route.

// The `kind` of a version file; the daemon's `history::VERSION_KIND`.
const VERSION_KIND = "ralphy-desk-version";

// Why a version was written, as the operator reads it.
const REASON_TEXT: Record<string, string> = {
  change: "Changed",
  "before-restore": "Before a restore",
  restore: "Restored",
  upload: "Uploaded",
};

function plural(n: number, one: string, many: string) {
  return `${n} ${n === 1 ? one : many}`;
}

// One row per version, in the reply's order (newest first). `when` turns an
// epoch ms into the time shown; the shell passes the browser's locale.
function rows(list: unknown, when: (ms: number) => string) {
  if (!Array.isArray(list)) return [];
  return list.map((v: any) => ({
    id: v.id,
    when: when(v.savedAt),
    reason: REASON_TEXT[v.reason] || v.reason,
    counts: [
      plural(v.windows, "console", "consoles"),
      plural(v.fences, "fence", "fences"),
      plural(v.notes, "note", "notes"),
    ].join(" · "),
  }));
}

const pad = (n: number) => String(n).padStart(2, "0");

// `ralphy-desk-2026-10-04-1530.json`, in local time.
function fileName(version: { savedAt: number }) {
  const d = new Date(version.savedAt);
  const day = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  return `ralphy-desk-${day}-${pad(d.getHours())}${pad(d.getMinutes())}.json`;
}

// The text of an uploaded file, as `{ version }`, or `{ cause }` for the
// failure line "Could not upload the desk layout: <cause>."
function parseUpload(text: string) {
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { cause: "the file is not JSON" };
  }
  const desk = parsed?.desk;
  if (parsed?.kind !== VERSION_KIND || !desk || typeof desk !== "object") {
    return { cause: "the file is not a desk layout saved by Ralphy" };
  }
  return { version: parsed };
}

export const WBDeskHistory = {
  rows,
  fileName,
  parseUpload,
  VERSION_KIND,
};
