// The entry module of `detached.html` (ADR-0075 D5). The page has no Alpine.
import { createViewer } from "./wb-file-viewer.ts";
import { createMessages } from "./wb-messages.ts";
import { wireDetached } from "./wb-detached.ts";

// The door for operator messages, before the file pane: this page has no
// shell, so a flash shows nothing and the pane asks with its own fallback.
const messages = createMessages(window, document, { shell: () => null });
// The opener reads `popup.WBViewer`, and the page posts "ready" only once it
// exists: the shell's answer reaches it.
window.WBViewer = createViewer(window, document, { messages });
wireDetached(window, document);
