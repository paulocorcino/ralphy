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
(function () {
  const EMPTY = {
    current: '',
    channel: 'rc',
    standing: 'unknown',
    severity: 'none',
    latest: null,
    gap: [],
    disabled: false,
  };

  async function read() {
    try {
      const resp = await fetch('/api/release', { headers: { Accept: 'application/json' } });
      if (!resp.ok) return EMPTY;
      const view = await resp.json();
      // A daemon that answers something unexpected is treated as "nothing to
      // say" rather than rendered half-way.
      if (!view || typeof view !== 'object' || !Array.isArray(view.gap)) return EMPTY;
      return Object.assign({}, EMPTY, view);
    } catch (_e) {
      // No daemon, no network, a page served from file:// — all the same thing:
      // we do not know, so we say nothing.
      return EMPTY;
    }
  }

  /* Whether the shell should draw anything at all. `none` covers being level,
   * being ahead of the tag (a development build), and knowing nothing. */
  function hasNews(view) {
    return !!view && view.severity !== 'none' && (view.gap || []).length > 0;
  }

  /* Only `urgent` survives a dismissal: a breaking change or a security fix is
   * not something the operator gets to forget by clicking once. */
  function isSticky(view) {
    return !!view && view.severity === 'urgent';
  }

  /* The line under the title: how many releases, and how far back. */
  function gapSummary(view) {
    const gap = (view && view.gap) || [];
    if (!gap.length) return '';
    const n = gap.length;
    return n === 1 ? '1 new release' : n + ' new releases';
  }

  async function setWatch(enable) {
    const body = new URLSearchParams({ enable: enable ? 'true' : 'false' });
    const resp = await fetch('/api/release/watch', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body,
    });
    if (!resp.ok) throw new Error('could not change the release watch');
    return resp.json();
  }

  window.WBRelease = { read, hasNews, isSticky, gapSummary, setWatch, EMPTY };
})();
