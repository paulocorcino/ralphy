/* ---------------------------------------------------------------------------
   The scroll probe (ADR-0084): how smooth each scroll of a console is.

   `createScrollProbe(deps)` makes the one probe of a page. It asks the daemon
   once whether the probe exists (`probe.scroll` with no records); a daemon
   built without the `scroll-probe` feature answers `unknown verb`, and then
   the probe does nothing for the rest of the load.

   `probe.watch(term, place)` gives one terminal its hooks. A scroll starts on
   the first move (touch, glide or wheel) and ends when nothing has moved for
   END_AFTER_MS. Each frame between the two is timed with
   `requestAnimationFrame`; the record goes to the daemon after the scroll
   ends, never during it. A record holds only counts and times: no text.
   --------------------------------------------------------------------------- */

// A frame longer than this missed its display refresh at 60 Hz and at 40 Hz.
export const SLOW_FRAME_MS = 25;
// A scroll has ended when nothing moved for this long, glide included.
export const END_AFTER_MS = 300;
// The most records in one command; the daemon refuses more.
export const MAX_RECORDS = 20;

export type ScrollInput = "touch" | "glide" | "wheel";
export type ScrollView = "stage" | "maximized" | "column" | "tab" | "full";
export type ScrollRenderer = "webgl" | "dom";

// One scroll, in the shape the daemon checks (`src/probe.rs`).
export type ScrollRecord = {
  input: ScrollInput;
  duration_ms: number;
  frames: number;
  slow_frames: number;
  slow_with_output: number;
  p50_ms: number;
  p95_ms: number;
  max_ms: number;
  step_ms: number;
  output_bytes: number;
  renderer: ScrollRenderer;
  rows: number;
  cols: number;
  history: number;
  view: ScrollView;
  dpr: number;
};

// Where the console is and how it draws, read when a scroll ends.
export type ScrollPlace = () => { view: ScrollView; renderer: ScrollRenderer; dpr: number };

// What the probe reads from the page, and nothing else.
export type ScrollProbeDeps = {
  // Sends `probe.scroll` with `records` and resolves with the daemon's reply.
  send: (records: ScrollRecord[]) => Promise<DaemonReply>;
  now: () => number;
  requestAnimationFrame: (callback: (time: number) => void) => number;
};

// The value at fraction `q` of `sorted`, which is in ascending order.
function quantile(sorted: number[], q: number) {
  if (sorted.length === 0) return 0;
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
}

// Rounds to a tenth of a millisecond: the record is read by a person.
const tenth = (ms: number) => Math.round(ms * 10) / 10;

// The frame figures of one scroll: `gaps` are the times between frames, and
// `output` says, per gap, whether the terminal wrote output in it.
export function frameFigures(gaps: number[], output: boolean[]) {
  const sorted = [...gaps].sort((a, b) => a - b);
  let slow = 0;
  let slowWithOutput = 0;
  gaps.forEach((gap, i) => {
    if (gap <= SLOW_FRAME_MS) return;
    slow += 1;
    if (output[i]) slowWithOutput += 1;
  });
  return {
    frames: gaps.length,
    slow_frames: slow,
    slow_with_output: slowWithOutput,
    p50_ms: tenth(quantile(sorted, 0.5)),
    p95_ms: tenth(quantile(sorted, 0.95)),
    max_ms: tenth(sorted.length ? sorted[sorted.length - 1] : 0),
  };
}

// The scroll in progress on one terminal.
type Scroll = {
  input: ScrollInput;
  startedAt: number;
  movedAt: number;
  lastFrame: number | null;
  gaps: number[];
  output: boolean[];
  outputInFrame: boolean;
  stepMs: number;
  outputBytes: number;
};

export function createScrollProbe(deps: ScrollProbeDeps) {
  // null until the daemon answered the first ask.
  let on: boolean | null = null;
  let asked = false;
  let queue: ScrollRecord[] = [];
  let sending = false;

  function ask() {
    if (asked) return;
    asked = true;
    deps.send([]).then(
      (reply) => {
        on = reply.status === "ok";
        if (on) flush();
        else queue = [];
      },
      // A daemon that could not be asked has no probe for this load.
      () => {
        on = false;
        queue = [];
      },
    );
  }

  function flush() {
    if (sending || queue.length === 0 || on !== true) return;
    const batch = queue.slice(0, MAX_RECORDS);
    queue = queue.slice(MAX_RECORDS);
    sending = true;
    const done = () => {
      sending = false;
      flush();
    };
    // A refused or lost batch is dropped: the probe never retries.
    deps.send(batch).then(done, done);
  }

  function watch(term: XtermTerminal, place: ScrollPlace) {
    ask();
    let scroll: Scroll | null = null;

    function finish(s: Scroll) {
      const where = place();
      queue.push({
        input: s.input,
        duration_ms: tenth(s.movedAt - s.startedAt),
        ...frameFigures(s.gaps, s.output),
        step_ms: tenth(s.stepMs),
        output_bytes: s.outputBytes,
        renderer: where.renderer,
        rows: term.rows,
        cols: term.cols,
        history: term.buffer.active.baseY,
        view: where.view,
        dpr: where.dpr,
      });
      flush();
    }

    function frame(time: number) {
      const s = scroll;
      if (!s) return;
      if (s.lastFrame !== null) {
        s.gaps.push(time - s.lastFrame);
        s.output.push(s.outputInFrame);
      }
      s.lastFrame = time;
      s.outputInFrame = false;
      if (deps.now() - s.movedAt < END_AFTER_MS) {
        deps.requestAnimationFrame(frame);
        return;
      }
      scroll = null;
      finish(s);
    }

    return {
      // The console moved. A glide keeps the scroll its touch started, and
      // names it a glide.
      moved(input: ScrollInput) {
        if (on === false) return;
        const now = deps.now();
        if (scroll) {
          scroll.movedAt = now;
          if (input === "glide") scroll.input = "glide";
          return;
        }
        scroll = {
          input,
          startedAt: now,
          movedAt: now,
          lastFrame: null,
          gaps: [],
          output: [],
          outputInFrame: false,
          stepMs: 0,
          outputBytes: 0,
        };
        deps.requestAnimationFrame(frame);
      },
      // The time of one scroll step of the page's own code.
      step(ms: number) {
        if (scroll) scroll.stepMs += ms;
      },
      // The terminal wrote `bytes` of output.
      wrote(bytes: number) {
        if (!scroll) return;
        scroll.outputBytes += bytes;
        scroll.outputInFrame = true;
      },
    };
  }

  return { watch };
}

// Where the console of `body` is and how it draws, for `watch`'s `place`.
export function placeOf(body: HTMLElement): ReturnType<ScrollPlace> {
  return {
    view: viewOf(body.closest(".session-window")),
    renderer: body.querySelector(".xterm-screen canvas") ? "webgl" : "dom",
    dpr: window.devicePixelRatio || 1,
  };
}

function viewOf(win: Element | null): ScrollView {
  if (!win) return "stage";
  if (document.fullscreenElement === win) return "full";
  if (win.classList.contains("in-tab")) return "tab";
  if (win.classList.contains("column")) return "column";
  return win.classList.contains("maximized") ? "maximized" : "stage";
}

export type ScrollProbe = ReturnType<typeof createScrollProbe>;
export type ScrollWatch = ReturnType<ScrollProbe["watch"]>;
