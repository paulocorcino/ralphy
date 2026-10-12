/* ---------------------------------------------------------------------------
   The two questions the page asks about its own screen:

   - `phoneLayout(win)`: is there room only for the phone layout? A question
     of space, so it follows the viewport: a narrow desktop window and a
     browser's device mode get the phone layout too.
   - `touchInput(win)`: can a finger reach this page? A question of input,
     so it follows the pointers, not the screen size.

   Which device the page runs on (phone, tablet, desktop) is not asked here.
   The daemon answers it from the device facts, for the audit log only
   (ADR-0074). Safari sends an iPad the same User-Agent as a Mac and has no
   User-Agent Client Hints, so a page cannot tell the device apart better
   than these two queries do.

   The stylesheets repeat both queries, because CSS cannot import them.
   `ui-tests/wb-viewport.test.mjs` fails when a copy differs.
   --------------------------------------------------------------------------- */

// The workbench's phone breakpoint. Viewport rules (01-base.css,
// 04-canvas.css, 11-appended.css) and `phoneBleed` use this number.
export const PHONE_MAX_WIDTH = 560;

// A phone in either orientation: its SHORT side is under the breakpoint.
// Landscape iPhone is ~750 wide but ~340 tall; an iPad's short side is 744+
// and keeps its desktop chrome. The landscape part reads the PRIMARY pointer:
// it names a phone turned sideways, and `any-pointer` would give the phone
// layout to a touch laptop with a short window.
export const PHONE_QUERY = `(max-width: ${PHONE_MAX_WIDTH}px), (pointer: coarse) and (max-height: ${PHONE_MAX_WIDTH}px)`;

// Any touch surface. `any-pointer` rather than `pointer`: an iPad with a
// trackpad keyboard reports a FINE primary pointer and still has a
// touch screen.
export const TOUCH_QUERY = "(any-pointer: coarse)";

/** The `window` members this module reads. A test passes a stub. */
type MediaWin = { matchMedia?: (query: string) => { matches: boolean } };

function matches(win: MediaWin, query: string) {
  try {
    return !!win.matchMedia?.(query)?.matches;
  } catch {
    return false;
  }
}

export function phoneLayout(win: MediaWin) {
  return matches(win, PHONE_QUERY);
}

export function touchInput(win: MediaWin) {
  return matches(win, TOUCH_QUERY);
}
