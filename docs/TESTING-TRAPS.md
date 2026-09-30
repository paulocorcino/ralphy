# Testing traps

Each trap was measured in this repo. A test or a check that ignores one passes
vacuously or fails for no reason. The words *vacuous* and *red* are defined in
[TESTING.md](./TESTING.md#words).

## File watcher

**One Windows filesystem action can produce more than one settled watcher
nudge.** `notify` can split one create into several debounced batches, each
mapped to the same watched directory. Assert at least one correctly stamped
nudge, and assert that every received nudge names the expected repo and path.
Leave the count of nudges for one create unasserted.

## PTY child and Ctrl+C

**On Windows ConPTY, a helper that reads `BufRead::lines()` sees a raw ETX
(`0x03`) only after a later newline.** An interrupt test child consumes bytes
and exits on ETX. The test then proves that the daemon delivered a raw Ctrl+C
to the native child, and not a kill from the server.

## In-process Axum server with WebSockets

**Aborting an in-process Axum `serve` task leaves running the WebSocket upgrade
tasks it already spawned.** A proxy-restart test fires the router shutdown
watch first, then aborts the serve task. In production, process death does
this; task cancellation alone leaves the old attachment busy.

## The workbench page in a browser

**A geometry assertion proves the element was visible when it measured.** An
Alpine `x-show` change is not visible to the next `evaluate`, and a hidden
element reports every dimension as `0`. So `scrollWidth <= clientWidth` passes
vacuously on a box that never rendered (measured in #331: `0 <= 0`). Gate the
`wait_for_function` on `offsetParent !== null && clientWidth > 0`, and repeat
the `clientWidth > 0` guard inside the assertion. The wait proves when; the
guard proves what.

**Tree selection after a write settles over time.** The daemon's own
`tree.dirty` for the directory arrives after the byte-op. Its reconcile pass
re-applies a selection it saved at its own start, so the node that a create or
duplicate just showed is moved away for a moment and then selected again.
Measured in #362: a read right after the new row appeared saw the previous
selection, and a bounded wait saw the right one every time. Assert the settled
state with `wait_for_function`.

**Wunderbaum paints `wb-active` a frame or more after `setActive()`.** Reading
`classList.contains('wb-active')` right after the row appears gives a false
red. Assert the tree's `getActiveNode()`, or wait for the class.

**A terminal's scroll position is `term.buffer.active.viewportY`.** The vendored
xterm renders through a monaco-style `.xterm-scrollable-element` that scrolls
by transform, so `.xterm-viewport` itself never scrolls. Measured in #337 with
400 lines written: `buffer.active.baseY == 389`, while
`scrollHeight === clientHeight === 342` and `scrollTop` stays `0`. A
`scrollHeight > clientHeight` precondition never becomes true, and a
`scrollTop` oracle reads `0` in both directions, so "it scrolled" and "it did
not scroll" both pass vacuously. Gate the precondition on `baseY` and assert
on `viewportY`.

**A Monaco diff editor does not report its own layout.** It has no
`getOption`, and in 0.56 both `getModifiedEditor().getOption(
EditorOption.renderSideBySide)` and `getRawOptions().renderSideBySide` return
`null`, so an options check reads as "the flag was ignored". Assert geometry
instead: `.editor.original` and `.editor.modified` have equal width, and the
modified pane starts where the original ends. Assert it at a narrow viewport
too. Below 900px Monaco swaps to the inline view unless
`useInlineViewWhenSpaceIsLimited` is `false`, and a wide viewport cannot see
that swap. `monaco.editor.getDiffEditors()` reaches the models.

**The diff's collapsed-lines ruler appears after the diff computation
settles.** `.diff-hidden-lines` paints later than the first `.view-lines`. A
test gated on the panes counts zero collapse widgets on a diff that does
collapse. Gate on `.diff-hidden-lines` itself.

**A leaked Monaco model is invisible to a reopen.** Disposing a diff editor
does not dispose its two models, and each model URI carries the viewer's
per-open `uid`, so a reopened tab never collides with a leaked model. The only
oracle is `monaco.editor.getModels().length` returning to a baseline taken
before the first open (`tests/wb_diff_311.py`).

**On WebKit, the computed `touch-action` is not the behaviour.** WebKit parses
every value and computes back exactly what the CSS says, but it honours only
`auto`, `none` and `manipulation`; `pan-x`, `pan-y` and `pinch-zoom` act as
`auto` ([WebKit #133112](https://bugs.webkit.org/show_bug.cgi?id=133112)).
Measured on an iPad: a console body with `pinch-zoom` panned the page, while
the title bar with `none` dragged correctly. A `getComputedStyle` assertion
passes on both.

**A synthetic `TouchEvent` cannot drive native scrolling.** Assert the touch
handler and the `touch-action` declaration separately.

## Python script reading a Rust child's output

**On Windows, decode the child's stdout as UTF-8 explicitly.**
`subprocess.run(..., text=True)` decodes with the console codepage (cp1252 on a
pt-BR or en-US default install). A non-ASCII byte from the Rust side, such as
the `→` in `ralphy daemon add`'s "registered X → path", comes back changed with
no exception, and a later `str.split` or `in` match fails without a message.
Pass `encoding="utf-8"` to `subprocess.run`. If the script itself prints a
non-ASCII string, call `sys.stdout.reconfigure(encoding="utf-8")` once at the
top; the default stdout write raises `UnicodeEncodeError`.
