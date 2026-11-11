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
function askPermission(): Promise<NotificationPermission> {
  if (!canNotify()) return Promise.resolve("denied");
  if (Notification.permission !== "default") return Promise.resolve(Notification.permission);
  return Notification.requestPermission();
}

/** How a test notification ended. */
export type NotifyTest = "shown" | "unsupported" | "denied" | "failed";

/** What Settings says after a test notification; `secure` is whether the
 * page is a secure context, which the browser needs to notify. */
export function notifyTestText(result: NotifyTest, secure: boolean) {
  switch (result) {
    case "shown":
      return (
        "The browser took the test notification. If you do not see it, the system hid it. " +
        "Check the notification settings of the system and of the browser."
      );
    case "unsupported":
      return secure
        ? "This browser cannot show notifications."
        : "This page cannot show notifications. " +
            "Open it from an https:// address or from localhost.";
    case "denied":
      return (
        "The browser blocks notifications for this page. Allow them in the site settings " +
        "of the browser, then try again."
      );
    case "failed":
      return "The browser refused to show the notification.";
    default: {
      const unknown: never = result;
      return unknown;
    }
  }
}

/** The browser notification switch of this device (Settings, Notifications). */
export const WBNotify = {
  /** Show one notification now, whether the switch is on or off; asks the
   * browser first when it never answered. */
  async test(): Promise<NotifyTest> {
    if (!canNotify()) return "unsupported";
    if ((await askPermission()) !== "granted") return "denied";
    try {
      const note = new Notification("Ralphy", {
        body: "A test notification from this browser.",
        tag: "ralphy-test",
      });
      // The one handler of a new object, and the ui-tests' fakes have no addEventListener.
      // oxlint-disable-next-line unicorn/prefer-add-event-listener -- see the line above
      note.onclick = () => {
        window.focus();
      };
      return "shown";
    } catch {
      // A browser that notifies only from a service worker throws here.
      return "failed";
    }
  },
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
