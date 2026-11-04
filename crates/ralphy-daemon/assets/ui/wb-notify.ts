/* ---------------------------------------------------------------------------
   The browser notification switch of this device, shown in Settings and kept
   in the per-client view (`wb-client-view.ts`). It is off until the operator
   turns it on, which is when the browser is asked for permission. The
   attention list (`wb-attention.ts`) reads it before it shows a notification.
   --------------------------------------------------------------------------- */
import { WBView } from "./wb-client-view.ts";

/** The saved choice; off when nothing is saved or the store cannot be read. */
function savedNotify() {
  return WBView.read()?.notify === true;
}

/** Keep the choice on this device. A store that cannot be written keeps the
 * switch working until the page is closed. */
function saveNotify(on: boolean) {
  WBView.patch({ notify: on });
}

/** Whether this page has the browser's notifications at all. */
export function canNotify() {
  return typeof Notification !== "undefined";
}

/** Ask the browser for permission to notify, unless it already answered. */
async function askPermission() {
  if (!canNotify()) return "denied";
  if (Notification.permission !== "default") return Notification.permission;
  return Notification.requestPermission();
}

/** The browser notification switch of this device (Settings, Notifications). */
export const WBNotify = {
  /** On: the operator turned it on here, and the browser allows it. */
  isOn() {
    return savedNotify() && canNotify() && Notification.permission === "granted";
  },
  /** Turn it on or off; on asks the browser first. Returns the state it ends in. */
  async turn(on: boolean) {
    const allowed = on && (await askPermission()) === "granted";
    saveNotify(allowed);
    return allowed;
  },
};
