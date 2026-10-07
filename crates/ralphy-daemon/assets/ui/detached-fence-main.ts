// The entry module of `detached-fence.html` (ADR-0075 D5). The page has no
// Alpine: each import sets the `window` name its classic scripts read (D9).
import "./wb-fleet.ts";
import "./wb-fail.ts";
import "./wb-session-route.ts";
import "./wb-columns.ts";
import { createDaemon } from "./wb-daemon.ts";
import { wireDetachedFence } from "./wb-detached-fence.ts";

// A module runs after every classic script, so the page posts "ready" only
// once `WBConsole`, `WBColumns` and `WBNotes`, which the shell's answer
// reaches, exist.
// The verb bridge: a pasted image becomes a clipboard drop through
// `image.write` (ADR-0055), and the consoles module reaches the daemon through
// `window.WBDaemon` exactly as it does in the workbench.
window.WBDaemon = createDaemon(window, document, location);
wireDetachedFence(window, document);
