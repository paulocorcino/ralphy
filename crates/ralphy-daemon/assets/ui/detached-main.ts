// The entry module of `detached.html` (ADR-0075 D5). The page has no Alpine:
// the import sets the `window` name its classic scripts read (D9).
import "./wb-fleet.ts";
import "./wb-monaco.ts";
import { createViewer } from "./wb-viewer.ts";
import { wireDetached } from "./wb-detached.ts";

// The opener reads `popup.WBViewer`, and the page posts "ready" only once it
// exists: the shell's answer reaches it.
window.WBViewer = createViewer(window, document);
wireDetached(window, document);
