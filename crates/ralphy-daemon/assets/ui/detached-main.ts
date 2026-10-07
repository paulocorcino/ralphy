// The entry module of `detached.html` (ADR-0075 D5). The page has no Alpine:
// the import sets the `window` name its classic scripts read (D9).
import "./wb-fleet.ts";
import { wireDetached } from "./wb-detached.ts";

// A module runs after every classic script, so the page posts "ready" only
// once `WBViewer`, which the shell's answer reaches, exists.
wireDetached(window, document);
