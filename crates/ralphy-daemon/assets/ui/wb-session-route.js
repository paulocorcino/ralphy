/* Pure browser-session ownership rules, shared by the shell and Node coverage. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  if (root) root.WBSessionRoute = api;
})(typeof window === "undefined" ? null : window, function () {
  function url(origin, opts) {
    let value = origin + "/ws/session?";
    if (opts.id != null) {
      value += "id=" + encodeURIComponent(opts.id);
      if (opts.repo) value += "&repo=" + encodeURIComponent(opts.repo);
      if (opts.takeover) value += "&takeover=1";
      if (opts.watch) value += "&watch=1";
    } else if (opts.console) {
      value += "console=1";
      if (opts.repo) value += "&repo=" + encodeURIComponent(opts.repo);
      // The startup command (the shell runs it and the session ends with it).
      if (opts.command) value += "&command=" + encodeURIComponent(opts.command);
    } else {
      value +=
        "repo=" +
        encodeURIComponent(opts.repo) +
        "&agent=" +
        encodeURIComponent(opts.agent);
      // A worktree NAME, sent only on a new agent launch — a reattach's record
      // owns it (ADR-0063 §3).
      if (opts.checkout) value += "&checkout=" + encodeURIComponent(opts.checkout);
    }
    return value;
  }

  function closeUrl(id, repo) {
    return `/api/sessions/close?id=${encodeURIComponent(id)}&repo=${encodeURIComponent(repo)}`;
  }

  function closeSucceeded(status) {
    return status === 200 || status === 404;
  }

  function announcement(current, payload) {
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

  function matchesRepo(session, repoRef) {
    return session.repo === repoRef;
  }

  return { url, closeUrl, closeSucceeded, announcement, matchesRepo };
});
