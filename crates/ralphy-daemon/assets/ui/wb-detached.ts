// The page script of `detached.html`, the torn-off file window. Importing it
// does nothing: `detached-main.ts` calls `wireDetached` after it created the
// file pane, so `WBViewer` exists when the shell answers "ready" (ADR-0075
// D5, D9).
import { isRecord } from "./wb-api.ts";
import { openerOf } from "./wb-events.ts";
import { isFileDescriptor, isOpenRequest } from "./wb-viewer.ts";

/** A message this window sends the shell. */
export type FilePopupMessage =
  | { type: "wb-detach-ready" }
  | { type: "wb-emit"; action: unknown; detail?: unknown }
  | { type: "wb-open-request"; detail?: OpenRequest }
  | { type: "wb-reattach"; desc?: FileDescriptor };

/** Whether `v` is a message this window sends the shell. The daemon serves
 * this page when the window opens, so after an update of the daemon it can be
 * a newer build than the shell. */
export function isFilePopupMessage(v: unknown): v is FilePopupMessage {
  if (!isRecord(v)) return false;
  if (v.type === "wb-open-request") return v.detail === undefined || isOpenRequest(v.detail);
  if (v.type === "wb-reattach") return v.desc === undefined || isFileDescriptor(v.desc);
  return v.type === "wb-detach-ready" || v.type === "wb-emit";
}

/** The shell's answer to "ready": the file this window shows. */
type DetachOpen = { type: "wb-detach-open"; desc: FileDescriptor };

/** Whether `v` is the shell's answer to "ready". The shell can be an older
 * build than this page, which the daemon served when the window opened. */
export function isDetachOpen(v: unknown): v is DetachOpen {
  return isRecord(v) && v.type === "wb-detach-open" && isFileDescriptor(v.desc);
}

export function wireDetached(window: Window, document: Document) {
  // Where this window will talk. `"*"` would broadcast the file's bytes to
  // whatever opened us, so the concrete origin is both the confidentiality
  // control and the authentication one: only the shell can hear us, and only
  // the shell can answer.
  const PEER = window.location.origin;
  const opener = () => openerOf(window);

  // The backend seam lives in the opener; forward every intent (save/reload)
  // there over postMessage, so a detached window is indistinguishable from
  // an in-shell tab.
  window.WB = {
    emit(action, ...detail) {
      const to = opener();
      if (to) to.postMessage({ type: "wb-emit", action, detail: detail[0] }, PEER);
      else console.log("[workbench:action]", { action, ...detail[0] });
    },
  };

  // A markdown link to another repo file: tabs live in the shell, so the
  // request rides over to it and the file opens there, next to the others.
  document.addEventListener("workbench:open-request", (e) => {
    opener()?.postMessage({ type: "wb-open-request", detail: e.detail }, PEER);
  });

  // Re-attach: hand the current descriptor back to the shell. Sent once:
  // the button's own `window.close()` fires the unload handlers below.
  let sent = false;
  const sendHome = () => {
    const desc = window.WBViewer?.descOf("detached");
    const to = opener();
    if (sent || !desc || !to) return;
    sent = true;
    to.postMessage({ type: "wb-reattach", desc }, PEER);
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
  const shell = opener();
  if (!shell) {
    empty("Open a file in the workbench, then detach it");
    return;
  }
  window.addEventListener("message", (e) => {
    if (e.origin !== window.location.origin) return;
    if (e.source !== opener()) return;
    const m: unknown = e.data;
    if (!isDetachOpen(m)) return;
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
      checkout: desc.checkout,
      encoding: desc.encoding,
      bom: desc.bom,
    });
    WBViewer.setActive("detached");
  });
  shell.postMessage({ type: "wb-detach-ready" }, PEER);
  // A shell that never answers (a foreign opener, or one that closed
  // mid-handover) leaves the pane empty rather than pending forever.
  setTimeout(() => {
    if (!document.querySelector("#viewers .viewer")) empty("The workbench did not send a file to this window");
  }, 3000);
}
