// The page script of `detached.html`, the torn-off file window. Importing it
// does nothing: `detached-main.ts` calls `wireDetached` after it created the
// file pane, so `WBViewer` exists when the shell answers "ready" (ADR-0075
// D5, D9).

export function wireDetached(window: Window, document: Document) {
  // Where this window will talk. `"*"` would broadcast the file's bytes to
  // whatever opened us, so the concrete origin is both the confidentiality
  // control and the authentication one: only the shell can hear us, and only
  // the shell can answer.
  const PEER = window.location.origin;

  // The backend seam lives in the opener; forward every intent (save/reload)
  // there over postMessage, so a detached window is indistinguishable from
  // an in-shell tab.
  window.WB = {
    emit(action, detail) {
      if (window.opener) window.opener.postMessage({ type: "wb-emit", action, detail }, PEER);
      else console.log("[workbench:action]", { action, ...detail });
    },
  };

  // A markdown link to another repo file: tabs live in the shell, so the
  // request rides over to it and the file opens there, next to the others.
  document.addEventListener("workbench:open-request", (e) => {
    if (window.opener) window.opener.postMessage({ type: "wb-open-request", detail: e.detail }, PEER);
  });

  // Re-attach: hand the current descriptor back to the shell. Sent once:
  // the button's own `window.close()` fires the unload handlers below.
  let sent = false;
  const sendHome = () => {
    const desc = window.WBViewer?.descOf("detached");
    if (sent || !desc || !window.opener) return;
    sent = true;
    window.opener.postMessage({ type: "wb-reattach", desc }, PEER);
  };
  document.addEventListener("workbench:reattach-request", () => {
    sendHome();
    window.close();
  });
  // Closing this window also sends the file home, with its unsaved edits.
  // BOTH events, as in wb-detached-fence.ts: `beforeunload` is the one a
  // close fires reliably, `pagehide` the one a bfcache-eligible navigation
  // fires. The opener also polls `closed` for a popup that fires neither.
  window.addEventListener("beforeunload", sendHome);
  window.addEventListener("pagehide", sendHome);

  // Boot: ASK the shell for the file descriptor (incl. current, possibly
  // edited content). It does not ride in the URL, because a URL is composed
  // by whoever sends the link — a `detached.html#<json>` hash let anyone
  // render content of their choosing on the daemon's own origin, and this
  // page is served without a credential under every auth policy.
  //
  // The request goes to `window.opener` with a concrete `targetOrigin`, so a
  // page on any other origin never receives it and therefore can never
  // answer it. No opener, or a foreign one, means no file — and we say so
  // rather than rendering anything.
  const empty = (why: string) => {
    document.getElementById("viewers")!.innerHTML =
      '<p class="detached-empty">Nothing to show. ' + why + ".</p>";
  };
  if (!window.opener) {
    empty("Open a file in the workbench, then detach it");
    return;
  }
  window.addEventListener("message", (e) => {
    if (e.origin !== window.location.origin) return;
    if (e.source !== window.opener) return;
    const m = e.data;
    if (!m || m.type !== "wb-detach-open" || !m.desc) return;
    const desc = m.desc;
    document.title = desc.path + " · Ralphy";
    WBViewer.open({
      id: "detached",
      project: desc.project,
      label: desc.label,
      path: desc.path,
      ftype: desc.ftype,
      content: desc.content,
      detached: true,
      checkout: desc.checkout ?? null,
      encoding: desc.encoding,
      bom: desc.bom,
    });
    WBViewer.setActive("detached");
  });
  window.opener.postMessage({ type: "wb-detach-ready" }, PEER);
  // A shell that never answers (a foreign opener, or one that closed
  // mid-handover) leaves the pane empty rather than pending forever.
  setTimeout(() => {
    if (!document.querySelector("#viewers .viewer")) empty("The workbench did not send a file to this window");
  }, 3000);
}
