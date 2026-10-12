/* ---------------------------------------------------------------------------
   The account menu at the head of the rail: the daemon's mark, name and
   uptime, the release dot, and the security and log off entries.

   `wbAccount` is the Alpine component of the menu (#621; ADR-0073 D2, D4). It
   holds only whether the menu is open. The release dot and the What's new row
   read the Alpine store `release` (wb-release-store.ts); the mark, the uptime
   and log off are the store `connection`'s (wb-connection-store.ts). The
   entries open the dialogs with the `workbench:whats-new-open`,
   `workbench:security-open` and `workbench:about-open` events (ADR-0073 D5).

   `main.ts` registers it as `wbAccount`.
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { sendWindow } from "./wb-events.ts";

export function wbAccount() {
  // It reads no `shell()` member.
  return component([], {
    avatarMenu: false,

    init() {
      window.addEventListener("workbench:menus-close", () => this.closeAvatarMenu());
      // Log off (the store `connection`) closes the menu it was chosen from.
      window.addEventListener("workbench:log-off", () => this.closeAvatarMenu());
    },

    // ONE dropdown at a time: the menu hears `workbench:menus-close` (sent by
    // every menu trigger, this one too) and closes here.
    closeAvatarMenu() {
      this.avatarMenu = false;
    },
    toggleAvatarMenu() {
      const was = this.avatarMenu;
      sendWindow(window, "workbench:menus-close");
      this.avatarMenu = !was;
    },
  });
}
