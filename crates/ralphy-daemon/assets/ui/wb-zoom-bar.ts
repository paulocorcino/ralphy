/* ---------------------------------------------------------------------------
   ralphy workbench — the zoom bar: -, percent, +, Fit and 100%

   `buildZoomBar` makes the bar of one zoomable pane and drives its zoom
   controller. `bindZoomKeys` gives the keys `+`, `-` and `0` to the pane
   that has the focus. The steps are those of the Enlarge view.
--------------------------------------------------------------------------- */
import { bindCommand } from "./wb-commands.ts";
import { ZOOM_STEP } from "./wb-diagram-zoom.ts";
import type { Point } from "./wb-diagram-zoom.ts";
import type { Controller } from "./wb-zoom-surface.ts";

/** A zoom controller that also says its scale. */
export type ZoomDrive = Controller & { scale(): number };

export type ZoomAct = "out" | "in" | "fit" | "actual";

export type ZoomBar = {
  el: HTMLElement;
  /** Run one act, and show the new percent. */
  run(act: ZoomAct): void;
  /** Show the scale now; a wheel or a pinch changed it without the bar. */
  refresh(): void;
};

const ORDER: readonly ZoomAct[] = ["out", "in", "fit", "actual"];
const LABELS: Record<ZoomAct, string> = {
  out: "Zoom out",
  in: "Zoom in",
  fit: "Fit to window",
  actual: "Actual size",
};
// The Actual size button shows its own text, not an icon.
const ICONS: Record<Exclude<ZoomAct, "actual">, string> = {
  out: "bi-zoom-out",
  in: "bi-zoom-in",
  fit: "bi-arrows-angle-contract",
};

function buildButton(doc: Document, act: ZoomAct) {
  const button = doc.createElement("button");
  button.type = "button";
  button.className = "zoom-bar-btn";
  button.dataset["zoom"] = act;
  button.title = LABELS[act];
  button.setAttribute("aria-label", LABELS[act]);
  if (act === "actual") {
    button.textContent = "100%";
  } else {
    const icon = doc.createElement("i");
    icon.className = `bi ${ICONS[act]}`;
    button.append(icon);
  }
  return button;
}

/** The percent text of a scale: 1 is "100%", 0.456 is "46%". */
export function percentText(scale: number) {
  return `${String(Math.round(scale * 100))}%`;
}

/** Build the bar of a pane. `surface` is the element the view is drawn in. */
export function buildZoomBar(
  doc: Document,
  drive: ZoomDrive,
  surface: Pick<HTMLElement, "clientWidth" | "clientHeight">,
): ZoomBar {
  const el = doc.createElement("div");
  el.className = "zoom-bar";
  const percent = doc.createElement("span");
  percent.className = "zoom-bar-percent";
  const middle = (): Point => ({ x: surface.clientWidth / 2, y: surface.clientHeight / 2 });
  const refresh = () => {
    percent.textContent = percentText(drive.scale());
  };
  const acts: Record<ZoomAct, () => void> = {
    out: () => {
      drive.zoom(1 / ZOOM_STEP, middle());
    },
    in: () => {
      drive.zoom(ZOOM_STEP, middle());
    },
    fit: () => {
      drive.fit();
    },
    actual: () => {
      drive.actual(middle());
    },
  };
  const run = (act: ZoomAct) => {
    acts[act]();
    refresh();
  };
  el.append(percent);
  for (const act of ORDER) {
    const button = buildButton(doc, act);
    button.addEventListener("click", () => {
      run(act);
    });
    el.append(button);
  }
  refresh();
  return { el, run, refresh };
}

const KEY_COMMANDS: ReadonlyArray<{ id: string; act: ZoomAct }> = [
  { id: "zoom.in", act: "in" },
  { id: "zoom.out", act: "out" },
  { id: "zoom.fit", act: "fit" },
];

/**
 * Give `+`, `-` and `0` to the zoomable pane that has the focus: `active`
 * returns its bar, or `null` when no such pane has it.
 */
export function bindZoomKeys(active: () => Pick<ZoomBar, "run"> | null) {
  for (const { id, act } of KEY_COMMANDS) {
    bindCommand(id, {
      can: () => active() !== null,
      run: () => {
        active()?.run(act);
      },
    });
  }
}
