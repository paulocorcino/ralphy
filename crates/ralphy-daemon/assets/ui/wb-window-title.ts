/* ---------------------------------------------------------------------------
   wb-window-title.ts — the title of a torn-off window.
   An installed app's window title is "<manifest name> - <page title>" unless
   the page title starts with the name (Chromium
   `WebAppBrowserController::GetTitle`). So the title starts with the
   manifest name, and the product name shows once.
   --------------------------------------------------------------------------- */

/** The page title of a torn-off window that shows `subject`. */
export function windowTitle(subject: string): string {
  return "Ralphy · " + subject;
}
