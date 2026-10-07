// The entry module of `detached-fence.html` (ADR-0075 D5). The page has no
// Alpine.
import { WBDeskSink } from "./wb-desk-sink.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import { createConsole } from "./wb-console.ts";
import { createDaemon } from "./wb-daemon.ts";
import { createNotes } from "./wb-notes.ts";
import { wireDetachedFence } from "./wb-detached-fence.ts";

// Where this window will talk. `"*"` would broadcast this fence's contents
// to whatever opened us, so the concrete origin is both the
// confidentiality control and the authentication one: only the shell can
// hear us, and only the shell can answer.
const PEER = window.location.origin;

// The backend seam lives in the opener; forward every intent there over
// postMessage, so a detached fence is indistinguishable from an in-shell one.
window.WB = {
  emit(action: string, detail?: object) {
    if (window.opener) window.opener.postMessage({ type: "wb-emit", action, detail }, PEER);
    else console.log("[workbench:action]", { action, ...detail });
  },
};

// THE FIVE CAPABILITIES THIS DOCUMENT DENIES ITSELF. This popup holds a
// FRAGMENT of the plane — one fence's members — so a desk write from here
// would replace the operator's whole layout with that fragment, and a view
// write would clobber the shell's stored viewport offset. The detach
// registry is the fifth: `window.open` handed this document a COPY of its
// opener's session storage, so a registry read here answers with a ghost
// that drifts the moment the real tab writes. Denying them by INJECTION
// rather than by a `detached` test at each call site is what makes the
// popup incapable rather than merely careful.
const consoleOpts = {
  deskSink: WBDeskSink.none(),
  viewStore: { read: () => null, patch: () => null },
  autoBoot: false,
  canLaunch: false,
  detachLink: WBDetachLink.none(),
};

// The consoles, before `wireDetachedFence` posts "ready": the shell's answer
// reaches `WBConsole`.
window.WBConsole = createConsole(window, document, location, consoleOpts);
// The verb bridge: a pasted image becomes a clipboard drop through
// `image.write` (ADR-0055), and the consoles module reaches the daemon through
// `window.WBDaemon` exactly as it does in the workbench.
window.WBDaemon = createDaemon(window, document, location);
// The note cards, before `wireDetachedFence` posts "ready".
window.WBNotes = createNotes(window, document);
wireDetachedFence(window, document);
// Last: the console boots once the page has started.
window.WBConsole.boot();
