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
    } else {
      value +=
        "repo=" +
        encodeURIComponent(opts.repo) +
        "&agent=" +
        encodeURIComponent(opts.agent);
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
    };
  }

  function matchesRepo(session, repoRef) {
    return session.repo === repoRef;
  }

  return { url, closeUrl, closeSucceeded, announcement, matchesRepo };
});
