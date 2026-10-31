/* Pure browser-session ownership rules, shared by the shell and Node coverage. */
// What the open of a console channel is built from: a reattach (`id`), a
// free console, or an agent launch.
// `gzip`: the page can decode a gzip replay, so it asks for one.
// `attempt`: the tab's number for this connect, sent beside the holder.
// `since`: on a reattach, the count of the session's output bytes the
// terminal shows.
// `link`: the network the browser reports, for the daemon's traffic log.
type SessionOpenOpts = { link?: LinkFacts } & (ReattachOpts | ConsoleOpts | LaunchOpts);
type ReattachOpts = {
  id: number;
  since?: number;
  repo?: string;
  takeover?: boolean;
  watch?: boolean;
  holder?: string;
  gzip?: boolean;
  attempt?: number;
};
type ConsoleOpts = {
  id?: null;
  console: true;
  repo?: string;
  command?: string;
  record?: string;
  holder?: string;
  gzip?: boolean;
  attempt?: number;
};
type LaunchOpts = {
  id?: null;
  console?: false;
  repo: string;
  agent: string;
  checkout?: string | null;
  name?: string | null;
  record?: string;
  holder?: string;
  gzip?: boolean;
  attempt?: number;
};

// The members of `navigator.connection` the daemon's traffic log records.
// Safari has no `navigator.connection`, and then nothing is sent.
export type LinkFacts = { effectiveType?: string; rtt?: number; downlink?: number };

// The open payload of a console channel: the fields of the daemon's session
// query (`SessionQuery`), each with the type it reads. `1` is a flag that is on.
export type SessionPayload = { [field: string]: string | number };

// What one connect of a terminal asks for, before it is one of the shapes
// above: `id` set is a reattach, `console` a free console, else an agent launch.
export type SessionRequest = {
  id?: number | null;
  repo?: string | null;
  agent?: string | null;
  console?: boolean;
  command?: string;
  checkout?: string | null;
  name?: string | null;
  record?: string;
  takeover?: boolean;
  watch?: boolean;
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
  // "gzip" when the replay frame that follows is gzipped. A peer of an older
  // build sends no such field, and its replay is raw.
  replay_encoding?: "gzip";
  // A replay frame follows.
  replay?: boolean;
  // The count of the session's output bytes at the end of the replay. A peer
  // of an older build sends neither this nor `resume`.
  offset?: number;
  // The replay is only the bytes after the `since` this socket sent: the
  // terminal keeps what it shows.
  resume?: boolean;
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

  // The tab's attempt number (ADR-0079 D7): raised on every connect, so the
  // daemon can tell a late socket of this tab from the one that replaced it.
  // Kept in `sessionStorage` beside the holder, so it keeps growing across a
  // reload; `attemptMemo` keeps it growing when the storage throws. It is at
  // least the clock in ms: a duplicated tab or a fence popup copies the
  // storage and so the holder, and the clock keeps the windows of that
  // holder in the order of their connects.
  const ATTEMPT_KEY = "ralphy.attempt";
  let attemptMemo = 0;
  function nextAttempt(storage: Storage | null | undefined, now = Date.now()) {
    let kept = 0;
    try {
      const n = Number(storage?.getItem(ATTEMPT_KEY));
      if (Number.isSafeInteger(n) && n > 0) kept = n;
    } catch {}
    attemptMemo = Math.max(Math.floor(now), Math.max(kept, attemptMemo) + 1);
    try {
      storage?.setItem(ATTEMPT_KEY, String(attemptMemo));
    } catch {}
    return attemptMemo;
  }

  function tabAttempt() {
    let storage = null;
    try {
      storage = globalThis.sessionStorage ?? null;
    } catch {}
    return nextAttempt(storage);
  }

  // The open payload of one console channel (ADR-0082 D3). A field the daemon
  // would not read is left out.
  function openPayload(opts: SessionOpenOpts): SessionPayload {
    return { ...(opts.gzip ? { gzip: 1 } : {}), ...kindFields(opts), ...linkFields(opts.link) };
  }

  // The fields that each kind of connect reads.
  function kindFields(opts: SessionOpenOpts) {
    if (opts.id != null) return reattachFields(opts);
    if (opts.console) return consoleFields(opts);
    return launchFields(opts);
  }

  function reattachFields(opts: ReattachOpts): SessionPayload {
    const out: SessionPayload = { id: opts.id };
    if (opts.repo) out.repo = opts.repo;
    if (Number.isSafeInteger(opts.since) && opts.since != null && opts.since >= 0)
      out.since = opts.since;
    if (opts.takeover) out.takeover = 1;
    // A watcher claims nothing, so it names no holder.
    if (opts.watch) return { ...out, watch: 1 };
    return { ...out, ...holderFields(opts.holder, opts.attempt) };
  }

  function consoleFields(opts: ConsoleOpts): SessionPayload {
    const out: SessionPayload = { console: 1 };
    if (opts.repo) out.repo = opts.repo;
    // The startup command (the shell runs it and the session ends with it).
    if (opts.command) out.command = opts.command;
    return { ...out, ...recordFields(opts.record), ...holderFields(opts.holder, opts.attempt) };
  }

  function launchFields(opts: LaunchOpts): SessionPayload {
    const out: SessionPayload = { repo: opts.repo, agent: opts.agent };
    // A worktree NAME, sent only on a new agent launch — a reattach's record
    // owns it (ADR-0063 §3).
    if (opts.checkout) out.checkout = opts.checkout;
    // The console name: the daemon folds it into Claude's session name.
    if (typeof opts.name === "string" && opts.name) out.name = opts.name;
    return { ...out, ...recordFields(opts.record), ...holderFields(opts.holder, opts.attempt) };
  }

  // Only the values the daemon reads; any other value is not sent. The
  // daemon reads the three as strings, so a bad one is absent there.
  const EFFECTIVE_TYPE_RE = /^(slow-2g|2g|3g|4g)$/;
  function linkFields(link: LinkFacts | undefined): SessionPayload {
    const out: SessionPayload = {};
    if (!link) return out;
    const { effectiveType, rtt, downlink } = link;
    if (typeof effectiveType === "string" && EFFECTIVE_TYPE_RE.test(effectiveType))
      out.net = effectiveType;
    if (typeof rtt === "number" && Number.isSafeInteger(rtt) && rtt >= 0) out.net_rtt = String(rtt);
    if (typeof downlink === "number" && Number.isFinite(downlink) && downlink >= 0)
      out.net_down = String(downlink);
    return out;
  }

  // The network this browser reports now, read at each connect.
  function tabLink(): LinkFacts | undefined {
    try {
      return (
        (globalThis.navigator as Navigator & { connection?: LinkFacts }).connection ?? undefined
      );
    } catch {
      return undefined;
    }
  }

  // The replay is gzipped only for a page that can decode it.
  function canGunzip() {
    return typeof DecompressionStream === "function";
  }

  // The open options of one connect, in the shape its kind reads. An agent
  // launch with no repo or no agent sends the value empty, never `null`: the
  // daemon refuses it as an unknown repo or agent.
  // `since` is sent only on a reattach: a launch opens a new terminal.
  function requestOpts(
    req: SessionRequest,
    holderId: string,
    gzip = false,
    attempt?: number,
    since?: number,
  ): SessionOpenOpts {
    const repo = req.repo ?? undefined;
    if (req.id != null)
      return {
        id: req.id,
        since,
        repo,
        takeover: req.takeover,
        watch: req.watch,
        holder: holderId,
        gzip,
        attempt,
      };
    if (req.console)
      return {
        console: true,
        repo,
        command: req.command,
        record: req.record,
        holder: holderId,
        gzip,
        attempt,
      };
    const { agent, checkout, name, record } = req;
    return {
      repo: req.repo ?? "",
      agent: agent ?? "",
      checkout,
      name,
      record,
      holder: holderId,
      gzip,
      attempt,
    };
  }
  // The window record of a NEW launch: the daemon keeps one live session per
  // record, so a second page relaunching it joins the first one's session.
  // Validated like the holder, as the daemon validates both.
  function recordFields(r: string | undefined): SessionPayload {
    return typeof r === "string" && HOLDER_RE.test(r) ? { record: r } : {};
  }

  // The attempt rides only with a holder: it orders the claims of one holder.
  function holderFields(h: string | undefined, attempt?: number): SessionPayload {
    if (typeof h !== "string" || !HOLDER_RE.test(h)) return {};
    if (Number.isSafeInteger(attempt) && attempt != null && attempt >= 0)
      return { holder: h, attempt };
    return { holder: h };
  }

  function closeUrl(id: number, repo: string) {
    return `/api/sessions/close?id=${encodeURIComponent(id)}&repo=${encodeURIComponent(repo)}`;
  }

  function closeSucceeded(status: number) {
    return status === 200 || status === 404;
  }

  function announcement(current: SessionOwner, payload: SessionOpen | null | undefined) {
    return {
      sessionId: announcedSessionId(current, payload),
      daemonId: payload?.daemon_id ?? current.daemonId,
      environment: payload?.environment ?? current.environment,
      ...announcedPlace(current, payload),
    };
  }

  function announcedSessionId(current: SessionOwner, payload: SessionOpen | null | undefined) {
    const announcedId = payload?.session_id ?? payload?.session;
    return announcedId == null ? current.sessionId : Number(announcedId);
  }

  function announcedPlace(current: SessionOwner, payload: SessionOpen | null | undefined) {
    return {
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
    openPayload,
    requestOpts,
    canGunzip,
    tabLink,
    holder,
    tabHolder,
    nextAttempt,
    tabAttempt,
    closeUrl,
    closeSucceeded,
    announcement,
    matchesRepo,
    UNANSWERED_HEADER,
    unanswered,
  };
})();
