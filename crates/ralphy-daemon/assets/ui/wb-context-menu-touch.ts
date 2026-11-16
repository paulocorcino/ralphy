/* ---------------------------------------------------------------------------
   The page's one `#ctxmenu`: its dismissal, and a finger held still on a tab
   or a file row opening it. iOS Safari fires no `contextmenu` on a hold, so
   the hold sends one to the element under the finger, and every existing
   `contextmenu` listener opens its menu as for a right click.
   --------------------------------------------------------------------------- */
import { HOLD_MS, dragBegins, dragThreshold } from "./wb-console-input.ts";

function hideContextMenu(document: Document) {
  const menu = document.querySelector<HTMLElement>("#ctxmenu");
  if (menu) menu.style.display = "none";
}

function swallow(ev: MouseEvent) {
  ev.preventDefault();
  ev.stopPropagation();
}

// The `click` of the release that ends a hold would close the menu it opened.
function swallowNextClick(window: Window) {
  window.addEventListener("click", swallow, { capture: true, once: true });
  // A release with no `click` (the finger moved off) leaves no listener behind.
  setTimeout(() => {
    window.removeEventListener("click", swallow, { capture: true });
  }, 600);
}

function watchHold(window: Window, document: Document, e: PointerEvent, target: HTMLElement) {
  const pressed = { x: e.clientX, y: e.clientY };
  const threshold = dragThreshold(e.pointerType);
  // Android fires its own `contextmenu` on a hold: then the hold sends none.
  let nativeMenu = false;
  const onNativeMenu = (ev: MouseEvent) => {
    if (ev.isTrusted) nativeMenu = true;
  };
  const end = () => {
    clearTimeout(timer);
    document.removeEventListener("pointermove", onMove);
    document.removeEventListener("pointerup", end);
    document.removeEventListener("pointercancel", end);
    document.removeEventListener("contextmenu", onNativeMenu, true);
  };
  const onMove = (ev: PointerEvent) => {
    if (ev.pointerId !== e.pointerId) return;
    if (dragBegins(pressed, { x: ev.clientX, y: ev.clientY }, threshold)) end();
  };
  const timer = setTimeout(() => {
    end();
    if (nativeMenu || !target.isConnected) return;
    swallowNextClick(window);
    const at = document.elementFromPoint(pressed.x, pressed.y) ?? target;
    at.dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: pressed.x,
        clientY: pressed.y,
      }),
    );
  }, HOLD_MS);
  document.addEventListener("pointermove", onMove);
  document.addEventListener("pointerup", end);
  document.addEventListener("pointercancel", end);
  document.addEventListener("contextmenu", onNativeMenu, true);
}

/** Close the menu on any outside click or scroll; open it on a held finger. */
export function wireContextMenu(window: Window, document: Document) {
  document.addEventListener("click", () => {
    hideContextMenu(document);
  });
  document.addEventListener(
    "scroll",
    () => {
      hideContextMenu(document);
    },
    true,
  );
  document.addEventListener("pointerdown", (e: PointerEvent) => {
    if (e.pointerType === "mouse" || !e.isPrimary || !(e.target instanceof Element)) return;
    // Where a hold opens a menu: the canvas tabs and the files tree.
    const target = e.target.closest<HTMLElement>(".tabstrip .tab, .files-pane .wb-host");
    if (target) watchHold(window, document, e, target);
  });
}
