// Unit tests for assets/ui/wb-console-session.ts — the console's session folds
// (wire codec, reconnect, resume and dormancy rules, peer rules). Pure
// functions: each test calls the function directly, with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CONNECT_TIMEOUT_MS,
  TAG_COMMAND,
  TAG_TERMINAL,
  birthDecision,
  consoleCommand,
  dormancyDecision,
  encodeCommand,
  encodeDetach,
  encodeResize,
  encodeTerminal,
  endNotice,
  heldReturnDecision,
  peerGate,
  peerHeld,
  peerHost,
  peerOfflineView,
  peerReturnDecision,
  reconnectDecision,
  relaunchRequest,
  resumeDecision,
  sessionRowFor,
  unheardRef,
} from "../assets/ui/wb-console-session.ts";

// --- resumeDecision: the resume rule, shared with wb-daemon.ts -------------
// A tablet suspends the tab: no JS runs while the link is torn down, so the
// sockets come back reporting OPEN with nothing ever arriving on them again.
// The fixed 3s retry only helps the ones that actually heard their close.

test("resumeDecision reconnects exactly the sockets the resume must replace", () => {
  const T = CONNECT_TIMEOUT_MS;
  // [case, socket, the stale verdicts it is asked under, expected]
  const rows = [
    // A socket that is gone reconnects, whatever the verdict.
    ["gone (null)", { readyState: null }, [true, false], "reconnect"],
    ["gone (undefined)", { readyState: undefined }, [true, false], "reconnect"],
    ["CLOSING", { readyState: 2 }, [true, false], "reconnect"],
    ["CLOSED", { readyState: 3 }, [true, false], "reconnect"],
    // A young CONNECTING socket is left alone — it IS the reconnect.
    ["young CONNECTING", { readyState: 0 }, [true, false], "none"],
    ["CONNECTING for 0 ms", { readyState: 0, connectingMs: 0 }, [true, false], "none"],
    [
      "CONNECTING just before the deadline",
      { readyState: 0, connectingMs: T - 1 },
      [true, false],
      "none",
    ],
    // Opened before the suspend, or onto a link that was not up yet: its
    // deadline timer froze with the tab, so the resume is what retires it.
    ["CONNECTING at the deadline", { readyState: 0, connectingMs: T }, [true, false], "reconnect"],
    // An OPEN socket churns only when the caller says it is stale.
    ["OPEN and stale", { readyState: 1 }, [true], "reconnect"],
    ["OPEN and not stale", { readyState: 1 }, [false], "none"],
  ];
  for (const [name, socket, stales, want] of rows) {
    for (const stale of stales) {
      assert.equal(resumeDecision({ ...socket, stale }), want, `${name}, stale=${stale}`);
    }
  }
});

// #411: the launch request a desk record relaunches with. An agent record asks
// for its vendor AND the worktree it was recorded in; a record without one
// (a pre-#411 desk, or a console on the primary) asks for `null`, the primary;
// a shell record is the shell request and never carries a checkout.
test("relaunchRequest carries the record's checkout on an agent record only", () => {
  const agent = { id: "a", repo: "owner/repo", agent: "claude", kind: "agent" };
  assert.deepEqual(relaunchRequest({ ...agent, checkout: "wt-a" }), {
    repo: "owner/repo",
    agent: "claude",
    checkout: "wt-a",
  });
  assert.deepEqual(relaunchRequest(agent), {
    repo: "owner/repo",
    agent: "claude",
    checkout: null,
  });
  assert.deepEqual(
    relaunchRequest({ id: "s", repo: "owner/repo", agent: "console", kind: "console", checkout: "wt-a" }),
    { console: true, repo: "owner/repo", command: undefined },
  );
  // "~" is the daemon's label for a repo-less console, never a slug to send back.
  assert.deepEqual(relaunchRequest({ id: "h", repo: "~", agent: "console", kind: "console" }), {
    console: true,
    repo: undefined,
    command: undefined,
  });
  // A console record whose label is not the literal `console` is a
  // startup-command console: the label IS the command it relaunches with.
  assert.deepEqual(relaunchRequest({ id: "m", repo: "owner/repo", agent: "htop", kind: "console" }), {
    console: true,
    repo: "owner/repo",
    command: "htop",
  });
});

// ADR-0059 §5: a window's row on `/api/sessions` is the daemon's id AND the
// repo ref — a restarted daemon reuses ids and a peer's `1` is not ours.
test("sessionRowFor matches a window's session by id and repo ref", () => {
  const win = (sessionId, repo) => ({ _term: { sessionId }, _deskRepo: repo });
  const sessions = [
    { id: 1, repo: "owner/a", agent_state: { state: "working", since: "t" } },
    { id: 1, repo: "01PEER/owner/a", agent_state: { state: "waiting", since: "t" } },
    { id: 2, repo: "~" },
  ];
  assert.equal(sessionRowFor(win(1, "owner/a"), sessions).agent_state.state, "working");
  assert.equal(sessionRowFor(win(1, "01PEER/owner/a"), sessions).agent_state.state, "waiting");
  assert.equal(sessionRowFor(win(2, "~"), sessions).id, 2);
  assert.equal(sessionRowFor(win(3, "owner/a"), sessions), null);
  // A window still waiting for its socket has `_wantsSession` and no term.
  assert.equal(
    sessionRowFor({ _wantsSession: 1, _deskRepo: "owner/a" }, sessions).agent_state.state,
    "working",
  );
  assert.equal(sessionRowFor({ _deskRepo: "owner/a" }, sessions), null);
});

// --- endNotice: the last line of a console that gave up -------------------
// A refused launch never had a session, so this line is the only place the
// browser can show why. Every other end keeps the line it always printed.

test("endNotice names the reason of a refused launch, and only of one", () => {
  assert.equal(endNotice("refused", "unknown repo"), "[could not start: unknown repo]");
  assert.equal(endNotice("refused", "  unknown repo \n"), "[could not start: unknown repo]");
  // A refusal with no words still says it did not start, never "undefined".
  assert.equal(endNotice("refused", null), "[could not start]");
  assert.equal(endNotice("refused", ""), "[could not start]");
  assert.equal(endNotice("refused", 42), "[could not start]");
  for (const reason of ["child-exited", "daemon-shutdown", "taken-over", null]) {
    assert.equal(endNotice(reason, "ignored"), "[session closed]", String(reason));
  }
});

// --- peer placeholders: a console whose peer cannot serve it ---------------
// The box words the peer's fleet state, offers only the action that can fix
// it, and comes back by itself only as a shell.

const PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const peerGroup = (state, extra = {}) => ({
  daemon: PEER,
  environment: "macOS 15",
  name: "corcino-mac",
  tunnel: true,
  state,
  diagnosis: `diagnosis of ${state}`,
  nudgeable: false,
  local: false,
  ...extra,
});

test("peerOfflineView words each peer state and offers only the action that fixes it", () => {
  const wsl = { tunnel: false, name: "", environment: "WSL: Ubuntu", nudgeable: true };
  const rows = [
    [peerGroup("asleep", wsl), "WSL: Ubuntu is asleep.", "wake"],
    [peerGroup("unreachable", wsl), "Ralphy is not running on WSL: Ubuntu.", "wake"],
    [peerGroup("unreachable"), "Ralphy is not running on corcino-mac.", "retry"],
    [peerGroup("tunnel-closed"), "Reconnecting to corcino-mac…", "wait"],
    [
      peerGroup("tunnel-silent"),
      "corcino-mac does not answer.",
      "retry",
    ],
    [peerGroup("unauthorized"), "corcino-mac cannot open this console.", null],
    [peerGroup("version-mismatch"), "corcino-mac cannot open this console.", null],
    [peerGroup("refused"), "corcino-mac cannot open this console.", null],
    [peerGroup("malformed"), "corcino-mac cannot open this console.", null],
    // The fleet has not seen what the refused launch saw.
    [peerGroup("reachable"), "corcino-mac did not start this console.", "retry"],
  ];
  for (const [group, text, action] of rows) {
    const got = peerOfflineView(group, "refused words", "fallback");
    assert.deepEqual(
      got,
      { text, detail: `diagnosis of ${group.state}`, action },
      group.state,
    );
  }
});

test("peerOfflineView with no fleet group yet uses the refusal and the recorded environment", () => {
  assert.deepEqual(peerOfflineView(null, "  tunnel open, daemon silent \n", "macOS 15"), {
    text: "macOS 15 did not start this console.",
    detail: "tunnel open, daemon silent",
    action: "retry",
  });
  // Nothing known at all still names someone, and shows no empty detail.
  assert.deepEqual(peerOfflineView(null, undefined, ""), {
    text: "The other computer did not start this console.",
    detail: "",
    action: "retry",
  });
});

test("peerReturnDecision relaunches only a shell, only after the box saw its peer offline", () => {
  const back = { available: true, wasOffline: true };
  assert.equal(peerReturnDecision({ kind: "console", canLaunch: true, ...back }), "relaunch");
  // A vendor CLI never starts without a click.
  assert.equal(peerReturnDecision({ kind: "agent", canLaunch: true, ...back }), "offer");
  // The popup authors no session at all.
  assert.equal(peerReturnDecision({ kind: "console", canLaunch: false, ...back }), "offer");
  // A refusal on a peer the fleet still calls reachable: no relaunch loop.
  assert.equal(
    peerReturnDecision({ kind: "console", canLaunch: true, available: true, wasOffline: false }),
    "stay",
  );
  assert.equal(
    peerReturnDecision({ kind: "console", canLaunch: true, available: false, wasOffline: true }),
    "stay",
  );
});

test("heldReturnDecision launches only when the list heard from the peer that nothing runs", () => {
  const row = { id: 3, repo: "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/repo" };
  // Not known (the list did not hear from the peer): never a launch.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: undefined }), "stay");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: true, session: undefined }), "stay");
  // It still runs there: attach, whatever the kind.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: row }), "attach");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: false, session: row }), "attach");
  // Heard, and not running: a shell opens again, a vendor CLI waits for a click.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: null }), "relaunch");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: true, session: null }), "offer");
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: false, session: null }), "offer");
});

test("peerHeld holds a peer project only while its known peer cannot serve it", () => {
  const ref = `${PEER}/owner/repo`;
  const groups = (state) => new Map([[PEER, peerGroup(state)]]);
  assert.equal(peerHeld(ref, groups("tunnel-silent"))?.state, "tunnel-silent");
  assert.equal(peerHeld(ref, groups("asleep"))?.state, "asleep");
  assert.equal(peerHeld(ref, groups("reachable")), null);
  // No state yet counts as available, as in the sidebar.
  assert.equal(peerHeld(ref, groups("")), null);
  assert.equal(peerHeld(ref, new Map()), null);
  assert.equal(peerHeld("owner/repo", groups("tunnel-silent")), null);
});

test("peerGate holds a reattach only while its known peer cannot serve it", () => {
  const opening = ["connect", "reconnect", "park-as-watcher"];
  const down = ["asleep", "unreachable", "tunnel-closed", "tunnel-silent", "unauthorized", "version-mismatch", "refused", "malformed"];
  for (const decision of opening) {
    for (const state of down) {
      assert.equal(peerGate({ decision, group: peerGroup(state), id: 7 }), "hold", `${decision} on ${state}`);
    }
    // Reachable, a state not heard yet, and no group: the socket opens.
    assert.equal(peerGate({ decision, group: peerGroup("reachable"), id: 7 }), decision);
    assert.equal(peerGate({ decision, group: peerGroup(""), id: 7 }), decision);
    assert.equal(peerGate({ decision, group: null, id: 7 }), decision);
    // A launch has no session to hold for: its placeholder says why.
    assert.equal(peerGate({ decision, group: peerGroup("asleep"), id: null }), decision);
  }
  // A session that ended stays ended, whatever the peer does.
  assert.equal(peerGate({ decision: "give-up", group: peerGroup("asleep"), id: 7 }), "give-up");
});

test("reconnectDecision reattaches any unannounced close and gives up only on an announced end", () => {
  const base = { everOpened: true, announced: null, idKnown: true, failedReopens: 0 };
  const rows = [
    ["no id: nothing to reattach to", { idKnown: false }, "give-up"],
    ["taken over: watch", { announced: "taken-over" }, "park-as-watcher"],
    ["the daemon said why", { announced: "child-exited" }, "give-up"],
    ["too many failed opens", { failedReopens: 11 }, "give-up"],
    ["a drop of a held session", {}, "reconnect"],
    ["never opened, first tries", { everOpened: false, failedReopens: 2 }, "reconnect"],
    ["never opened, then watch", { everOpened: false, failedReopens: 3 }, "park-as-watcher"],
  ];
  for (const [name, change, want] of rows) {
    assert.equal(reconnectDecision({ ...base, ...change }), want, name);
  }
  // A proxy closes cleanly for a socket the daemon dropped (ADR-0051 §9): the
  // close itself says nothing, so an unannounced clean close reconnects.
  for (const close of [{ code: 1000, wasClean: true }, { code: 1001, wasClean: true }, { code: 1005, wasClean: true }]) {
    assert.equal(reconnectDecision({ ...base, opened: true, ...close }), "reconnect", JSON.stringify(close));
  }
});

// --- encodeDetach: why the page closes a console socket ----------------------
// The daemon logs the reason, so a dormancy reattach is told apart from a
// network drop. The daemon's `Leave::from_command` test reads this same JSON.
const textOf = (frame) => new TextDecoder().decode(frame.subarray(1));

test("encodeDetach is a command frame that names the reason", () => {
  const frame = encodeDetach("dormant");
  assert.equal(frame[0], 0x02, "the command tag");
  assert.equal(textOf(frame), '{"id":0,"verb":"detach","payload":{"reason":"dormant"}}');
});

test("encodeResize keeps its frame", () => {
  const frame = encodeResize(24, 80);
  assert.equal(frame[0], 0x02, "the command tag");
  assert.equal(textOf(frame), '{"id":0,"verb":"resize","payload":{"rows":24,"cols":80}}');
});

// --- dormancyDecision: a console off the viewport gives its renderer back ---
// Chrome caps a document at ~16 live WebGL contexts. Past that the xterm addon
// disposes itself and EVERY terminal falls back to the DOM renderer, so a desk
// gets slower the more consoles are open. A window scrolled well off the stage
// viewport therefore disposes its terminal and closes its socket, and rebuilds
// when it returns — the session is the daemon's, and the reattach replays it.
//
// The rule is a pure fold: the IntersectionObserver supplies `intersecting` and
// the caller owns the fifteen-second grace period. `live` is a window that is
// allowed to sleep, so each test below changes exactly one thing about it.
const live = {
  intersecting: false,
  covered: false,
  dormant: false,
  maximized: false,
  fullscreen: false,
  focused: false,
  hasTerminal: true,
  ended: false,
  sessionId: 7,
};

test("dormancyDecision sleeps only a live console nobody can see", () => {
  const asleep = { dormant: true, hasTerminal: false };
  // [case, what changes about `live`, expected decision]
  const rows = [
    ["a live console off the viewport sleeps", {}, "sleep"],
    ["a visible console holds", { intersecting: true }, "hold"],
    // Visible outranks every other reading: a dormant window that comes back
    // wakes even while it is also maximized or focused.
    ["a dormant console that comes back wakes", { intersecting: true, ...asleep }, "wake"],
    [
      "a dormant console that comes back wakes even maximized and focused",
      { intersecting: true, ...asleep, maximized: true, focused: true },
      "wake",
    ],
    // Columns, a maximize or the physical screen fill the viewport. The
    // windows under them are inside it, so the observer calls them visible,
    // but nobody sees them: they sleep, and they do not wake until uncovered.
    ["a console under a full bleed sleeps", { intersecting: true, covered: true }, "sleep"],
    [
      "a dormant console under a full bleed stays asleep",
      { intersecting: true, covered: true, ...asleep },
      "hold",
    ],
    // Already asleep and still away: nothing to do. Without this the caller
    // would re-arm its timer on every observer callback for the life of the page.
    ["a window is never slept twice", { ...asleep, sessionId: null }, "hold"],
    // Both fill the viewport, so "outside" is a lie the observer can still tell
    // in the frame between the class landing and the layout that follows it.
    ["a maximized console holds", { maximized: true }, "hold"],
    ["a fullscreen console holds", { fullscreen: true }, "hold"],
    // Every drag and every resize begins with a `pointerdown` that calls
    // `focusWin`, so this one guard covers a window being hauled across the
    // plane without a second "dragging" flag nothing else in the module keeps.
    ["the focused (and dragged) console holds", { focused: true }, "hold"],
    ["a placeholder holds: there is no terminal to dispose", { hasTerminal: false }, "hold"],
    // No daemon-side session means no replay: sleeping would throw away the
    // last thing the agent said, permanently.
    ["an ended session holds", { ended: true }, "hold"],
    // The R1 hazard, in this direction: `WBSessionRoute.url` composes a LAUNCH
    // url when `id` is absent, so waking such a window would spawn a SECOND
    // vendor CLI rather than reattaching to the first.
    ["a null session id holds", { sessionId: null }, "hold"],
    ["an undefined session id holds", { sessionId: undefined }, "hold"],
    // Zero is a real session id, not an absent one.
    ["session id zero sleeps", { sessionId: 0 }, "sleep"],
  ];
  for (const [name, change, want] of rows) {
    assert.equal(dormancyDecision({ ...live, ...change }), want, name);
  }
});

// A restored console that reattaches starts asleep, so a console off the
// viewport never replays its text; the observer's first report wakes the
// visible ones.
test("birthDecision starts asleep only a reattach the observer can wake", () => {
  const rows = [
    ["a reattach starts asleep", { id: 7, observed: true }, "dormant"],
    ["session id zero is a reattach", { id: 0, observed: true }, "dormant"],
    // A launch has no id to wake to: it would spawn a second CLI.
    ["a launch attaches", { id: null, observed: true }, "attach"],
    ["an undefined id attaches", { id: undefined, observed: true }, "attach"],
    // Without an IntersectionObserver nothing would ever wake the window.
    ["no observer: a reattach attaches", { id: 7, observed: false }, "attach"],
  ];
  for (const [name, inputs, want] of rows) {
    assert.equal(birthDecision(inputs), want, name);
  }
});

// --- encodeTerminal, encodeCommand: the frames src/protocol.rs reads ---------

test("encodeTerminal is the terminal tag, the session id as a u64, then the bytes", () => {
  const frame = encodeTerminal("hé");
  assert.equal(frame[0], TAG_TERMINAL);
  assert.deepEqual([...frame.subarray(1, 9)], [0, 0, 0, 0, 0, 0, 0, 1], "session 1, big endian");
  assert.equal(new TextDecoder().decode(frame.subarray(9)), "hé");
  assert.equal(encodeTerminal("").length, 9, "an empty write is the header alone");
});

test("encodeCommand is the command tag and one JSON body with id 0", () => {
  const frame = encodeCommand("resize", { rows: 1, cols: 2 });
  assert.equal(frame[0], TAG_COMMAND);
  assert.deepEqual(JSON.parse(new TextDecoder().decode(frame.subarray(1))), {
    id: 0,
    verb: "resize",
    payload: { rows: 1, cols: 2 },
  });
});

// --- peerHost, unheardRef, consoleCommand ------------------------------------

test("peerHost names the machine, else the recorded environment, else a plain phrase", () => {
  assert.equal(peerHost(peerGroup("asleep"), "WSL: Ubuntu"), "corcino-mac");
  assert.equal(peerHost(peerGroup("asleep", { name: "" }), "WSL: Ubuntu"), "macOS 15");
  assert.equal(peerHost(null, "WSL: Ubuntu"), "WSL: Ubuntu");
  assert.equal(peerHost(null, ""), "The other computer");
  assert.equal(peerHost(undefined, undefined), "The other computer");
});

test("unheardRef is true only for a peer ref whose daemon the list did not hear from", () => {
  const ref = `${PEER}/owner/repo`;
  assert.equal(unheardRef(ref, new Set([PEER])), true);
  assert.equal(unheardRef(ref, new Set(["OTHER"])), false);
  assert.equal(unheardRef(ref, undefined), false, "no list of unheard peers");
  assert.equal(unheardRef("owner/repo", new Set([PEER])), false, "a local ref has no peer");
  assert.equal(unheardRef("~", new Set([PEER])), false);
  assert.equal(unheardRef("owner/repo", new Set([""])), false, "no daemon id is never an unheard peer");
});

test("consoleCommand is the label of a startup-command console, and nothing for the bare shell", () => {
  assert.equal(consoleCommand("htop"), "htop");
  assert.equal(consoleCommand("console"), undefined);
  assert.equal(consoleCommand(""), undefined);
  assert.equal(consoleCommand(undefined), undefined);
});
