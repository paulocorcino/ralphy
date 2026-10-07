/* The release watch, browser side (ADR-0056 §7).
 *
 * One read of /api/release, which answers from the daemon's cache — the fetch
 * to GitHub is the daemon's background job, so nothing here waits on the
 * network and a page load never triggers a request of its own.
 *
 * This module decides nothing. The severity is the daemon's (it comes from the
 * kinds the releases carried, not from the version numbers), and the shell just
 * renders it: `none` shows nothing at all, `quiet` a dot, `notable` a badge,
 * `urgent` a badge that does not go away when dismissed.
 */
/** The release view of `/api/release` (ADR-0056 §7). */
export type ReleaseView = {
  current: string;
  channel: string;
  standing: string;
  severity: string;
  latest: unknown;
  gap: unknown[];
  disabled: boolean;
  can_update: boolean;
};

const EMPTY: ReleaseView = {
  current: '',
  channel: 'rc',
  standing: 'unknown',
  severity: 'none',
  latest: null,
  gap: [],
  disabled: false,
  can_update: false,
};

/* The view, or `null` when there is no answer. `null` is not EMPTY: the
 * page reads again each time the tab comes back, and a daemon that is
 * restarting must not erase what the page already knew. */
async function read(): Promise<ReleaseView | null> {
  try {
    const resp = await fetch('/api/release', { headers: { Accept: 'application/json' } });
    if (!resp.ok) return null;
    const view = await resp.json();
    // A daemon that answers something unexpected is treated as no answer
    // rather than rendered half-way.
    if (!view || typeof view !== 'object' || !Array.isArray(view.gap)) return null;
    return Object.assign({}, EMPTY, view);
  } catch (_e) {
    // No daemon, no network, a page served from file:// — all the same thing:
    // we do not know.
    return null;
  }
}

/* Whether the shell should draw anything at all. `none` covers being level,
 * being ahead of the tag (a development build), and knowing nothing. */
function hasNews(view: ReleaseView | null): boolean {
  return !!view && view.severity !== 'none' && (view.gap || []).length > 0;
}

/* Only `urgent` survives a dismissal: a breaking change or a security fix is
 * not something the operator gets to forget by clicking once. */
function isSticky(view: ReleaseView | null): boolean {
  return !!view && view.severity === 'urgent';
}

/* The line under the title: how many releases, and how far back. */
function gapSummary(view: ReleaseView | null): string {
  const gap = (view && view.gap) || [];
  if (!gap.length) return '';
  const n = gap.length;
  return n === 1 ? '1 new release' : n + ' new releases';
}

async function setWatch(enable: boolean) {
  const body = new URLSearchParams({ enable: enable ? 'true' : 'false' });
  const resp = await fetch('/api/release/watch', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body,
  });
  if (!resp.ok) throw new Error('Could not change the release watch.');
  return resp.json();
}

/* Ask the daemon to take the newest release (ADR-0056 §11). The answer comes
 * when the new binary is in place, so this can take a minute. `{ ok: true }`
 * means the daemon is restarting. A refusal carries the status and the
 * daemon's text, which for a failed download is the update's last lines. */
async function update(code?: string) {
  const body = new URLSearchParams();
  if (code) body.set('code', code);
  const resp = await fetch('/api/release/update', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body,
  });
  if (resp.ok) return { ok: true };
  return {
    ok: false,
    status: resp.status,
    retryAfter: resp.headers.get('Retry-After'),
    message: (await resp.text()).trim(),
  };
}

export const WBRelease = {
  read,
  hasNews,
  isSticky,
  gapSummary,
  setWatch,
  update,
  EMPTY,
};
