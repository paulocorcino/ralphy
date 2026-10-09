/* Pure browser-session ownership rules, shared by the shell and Node coverage. */
// What a `/ws/session` url is built from: a reattach (`id`), a free console,
// or an agent launch.
type SessionUrlOpts =
  | { id: number; repo?: string; takeover?: boolean; watch?: boolean; holder?: string }
  | { id?: null; console: true; repo?: string; command?: string; record?: string; holder?: string }
  | {
      id?: null;
      console?: false;
      repo: string;
      agent: string;
      checkout?: string | null;
      name?: string | null;
      record?: string;
      holder?: string;
    };

// The owner of a session as a terminal knows it.
type SessionOwner = {
  sessionId: number | null;
  daemonId: string | null;
  environment: string | null;
  name?: string | null;
  checkout?: string | null;
};

// The payload of the daemon's `session-open` announcement.
type SessionOpen = {
  session?: number;
  session_id?: number;
  daemon_id?: string;
  environment?: string;
  name?: string | null;
  checkout?: string | null;
};

export const WBSessionRoute = (function () {
  // The tab's holder id (ADR-0051 §9 amendment 2026-09-22): what the writer slot
  // is claimed as, so this tab's reattach can reclaim a slot its own earlier
  // socket still holds half-open behind a tunnel. The daemon's rule, mirrored:
  // anything else is not a holder.
  const HOLDER_KEY = "ralphy.holder";
  const HOLDER_RE = /^[A-Za-z0-9_-]{1,64}$/;

  // `sessionStorage` is per tab and survives a reload, which is the scope that
  // must keep the name. A storage that throws or is absent still yields a
  // holder; it just will not outlive the document.
  function holder(storage: Storage | null | undefined, mint: () => string) {
    try {
      const kept = storage?.getItem(HOLDER_KEY);
      if (kept && HOLDER_RE.test(kept)) return kept;
    } catch {}
    const fresh = mint();
    try {
      storage?.setItem(HOLDER_KEY, fresh);
    } catch {}
    return fresh;
  }

  // This document's holder, resolved on the first connect and never at load.
  // Here, not in wb-console.ts: the console module names no browser store
  // (#347). `getRandomValues`, not `randomUUID`: the latter exists only in a
  // secure context, and a LAN bind is plain HTTP.
  let tabHolderMemo: string | null = null;
  function tabHolder() {
    if (tabHolderMemo == null) {
      let storage = null;
      try {
        storage = globalThis.sessionStorage ?? null;
      } catch {}
      tabHolderMemo = holder(storage, () =>
        Array.from(globalThis.crypto.getRandomValues(new Uint8Array(16)), (b) =>
          b.toString(16).padStart(2, "0"),
        ).join(""),
      );
    }
    return tabHolderMemo;
  }

  function url(origin: string, opts: SessionUrlOpts) {
    let value = origin + "/ws/session?";
    if (opts.id != null) {
      value += "id=" + encodeURIComponent(opts.id);
      if (opts.repo) value += "&repo=" + encodeURIComponent(opts.repo);
      if (opts.takeover) value += "&takeover=1";
      // A watcher claims nothing, so it names no holder.
      if (opts.watch) value += "&watch=1";
      else value += holderParam(opts.holder);
    } else if (opts.console) {
      value += "console=1";
      if (opts.repo) value += "&repo=" + encodeURIComponent(opts.repo);
      // The startup command (the shell runs it and the session ends with it).
      if (opts.command) value += "&command=" + encodeURIComponent(opts.command);
      value += recordParam(opts.record);
      value += holderParam(opts.holder);
    } else {
      value +=
        "repo=" +
        encodeURIComponent(opts.repo) +
        "&agent=" +
        encodeURIComponent(opts.agent);
      // A worktree NAME, sent only on a new agent launch — a reattach's record
      // owns it (ADR-0063 §3).
      if (opts.checkout) value += "&checkout=" + encodeURIComponent(opts.checkout);
      // The console name: the daemon folds it into Claude's session name.
      if (typeof opts.name === "string" && opts.name)
        value += "&name=" + encodeURIComponent(opts.name);
      value += recordParam(opts.record);
      value += holderParam(opts.holder);
    }
    return value;
  }

  // The window record of a NEW launch: the daemon keeps one live session per
  // record, so a second page relaunching it joins the first one's session.
  // Validated like the holder, as the daemon validates both.
  function recordParam(r: string | undefined) {
    return typeof r === "string" && HOLDER_RE.test(r) ? "&record=" + r : "";
  }

  function holderParam(h: string | undefined) {
    return typeof h === "string" && HOLDER_RE.test(h) ? "&holder=" + h : "";
  }

  function closeUrl(id: number, repo: string) {
    return `/api/sessions/close?id=${encodeURIComponent(id)}&repo=${encodeURIComponent(repo)}`;
  }

  function closeSucceeded(status: number) {
    return status === 200 || status === 404;
  }

  function announcement(current: SessionOwner, payload: SessionOpen | null | undefined) {
    const announcedId = payload?.session_id ?? payload?.session;
    return {
      sessionId: announcedId == null ? current.sessionId : Number(announcedId),
      daemonId: payload?.daemon_id ?? current.daemonId,
      environment: payload?.environment ?? current.environment,
      // The name the hosting vendor knows this child by, when it takes one. A
      // payload that omits it KEEPS the prior — the same rule the two fields
      // above follow, so a re-announcement can never blank a live name.
      name: payload?.name ?? current.name ?? null,
      // The worktree the console lives in; keeps the prior for the same reason.
      checkout: payload?.checkout ?? current.checkout ?? null,
    };
  }

  function matchesRepo(session: { repo: string }, repoRef: string) {
    return session.repo === repoRef;
  }

  // The `x-ralphy-unanswered` header of `/api/sessions`: the daemon ids of the
  // peers the list did not hear from. Their sessions are UNKNOWN on this read,
  // not absent.
  const UNANSWERED_HEADER = "x-ralphy-unanswered";
  function unanswered(value: string | null | undefined) {
    return new Set(
      String(value ?? "")
        .split(",")
        .map((id) => id.trim())
        .filter(Boolean),
    );
  }

  return {
    url,
    holder,
    tabHolder,
    closeUrl,
    closeSucceeded,
    announcement,
    matchesRepo,
    UNANSWERED_HEADER,
    unanswered,
  };
})();
