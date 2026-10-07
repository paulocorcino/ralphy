// The entry module of `detached-fence.html` (ADR-0075 D5). The page has no
// Alpine: each import sets the `window` name its classic scripts read (D9).
import "./wb-fleet.ts";
import "./wb-fail.ts";
import { wireDetachedFence } from "./wb-detached-fence.ts";

// A module runs after every classic script, so the page posts "ready" only
// once `WBConsole`, `WBColumns` and `WBNotes`, which the shell's answer
// reaches, exist.
wireDetachedFence(window, document);
