# The workbench keyboard is one list of commands with direct Alt+Shift chords

Status: proposed
Kind: structural
Protects: consistency of the workbench; constraint portability

## Context

The workbench has about 46 shortcuts, listed in the Keyboard section of
`docs/DESIGN-SYSTEM.md` (read on 2026-11-14). The page shortcuts use
Alt+Shift, matched on `e.code`. Each one is its own document listener, in
four files (`wb-wire.ts:447`, `wb-consoles-tab-menus.ts:309`,
`wb-consoles-tab.ts:448`, `wb-console-press.ts:31`). There is no command
palette and no help screen.

A console runs programs that need almost every key: Escape, Tab, Ctrl with
any letter, the F keys. A browser check on 2026-11-14 (Chromium, scratch
daemon) found four places where the page and the terminal disagree about a
key: #953, #954, #955. The terminal gives a chord to the page before it
knows whether the page will act, and some Escape listeners do not stop the
key.

Ralphy ships on Windows, Linux and macOS. On a Mac, Alt is Option,
Option+Shift with a key types a symbol, Cmd belongs to the browser and the
system, and F1 to F12 control the screen and the sound unless Fn is held.

## Decision

**D1.** There is no prefix key. Every page shortcut is one direct chord:
Alt+Shift (Option+Shift on a Mac) with one more key, matched on `e.code`.
Only `/`, `?` and Ctrl+Shift+F (Cmd+Shift+F on a Mac) are outside this
form; `/` and `?` count only while the focus is outside a text field, an
editor and a terminal.

**D2.** Every page shortcut is a command in one list, in one workbench
module. A command has an id, a label, its keys (or none), a check "can it
run now", and its action. One document listener runs the list. No other
code adds a page shortcut.

**D3.** In a terminal every key goes to the child program, except a chord
of the list whose check says the command can run now. The terminal asks
the list before it gives a chord to the page. When the command cannot run,
the key goes to the child program.

**D4.** There is no key that only leaves the terminal. The focus leaves a
terminal through a command that opens or focuses something: the palette, a
menu, a fence, another column.

**D5.** What a command opens from a terminal gives the focus back to that
terminal when it closes. When the command moves to another console, the
focus goes to the terminal of that console.

**D6.** A command palette opens on Alt+Shift+P, also from a terminal. It
lists the commands that can run now, each with its keys. Typing filters,
the arrow keys move, Enter runs, Escape closes. The palette lists commands,
not files or projects: `/` stays the search.

**D7.** A help screen opens on `?` (with the focus outside a text field, an
editor and a terminal) and on Alt+Shift+/ from anywhere. The palette has a
row for it too. It is a modal that only shows text. It is made from the
list of D2, plus the keys inside components, plus one line that says every
other key in a terminal goes to the program. On a touch screen it is
reached from the palette and the account menu.

**D8.** The list refuses these keys:

- any key without Alt+Shift, except the three of D1;
- any chord with Ctrl or Cmd (Meta); Ctrl goes to the child program and Cmd
  to the browser and the system;
- Escape, Tab, and F1 to F12 alone;
- the keys a browser keeps for itself: Ctrl or Cmd with W, T, N or Q,
  Ctrl+Shift+N, P, T, C, I or J, Cmd+Option+I, F11, F12, Alt+F4.

Two Ctrl keys stay, as they are today. Ctrl+Shift+F is the one command
with Ctrl (Cmd on a Mac): xterm sends nothing for it, so the child program
loses nothing. Ctrl+Insert is not a command: the terminal copies its
selection with it when it has one (Windows and Linux; on a Mac, Cmd+C from
the browser).

**D9.** The keys inside a component (Escape in a dialog, Enter in a field,
the arrow keys in a menu) stay in the component and are not commands. A
component that takes Escape or Enter from a terminal's focus stops the
event.

**D10.** The keys are fixed. A person cannot change them.

**D11.** Every key shown to a person (a menu hint, a `title`, the palette,
the help screen) is written for the person's system: "⌥⇧3" on a Mac,
"Alt+Shift+3" elsewhere.

## Consequences

The four page listeners of today move into the list. D3 fixes #953,
and #954 becomes one more row of the list (F1 to F12 with their fence). D9
fixes #955.

On a Mac, the fence keys need Fn (Fn+⌥⇧F1). The palette reaches a fence by
its name, so a Mac does not depend on those keys.

On Windows, Left Alt+Shift alone can change the input language when two or
more layouts are installed. Every chord here has a third key; this is a
known risk, not measured.

The status becomes accepted when the list ships and a check on a Mac, in
Safari and in Chrome, has tried the chords inside and outside a terminal,
Cmd+C in a terminal, and the help screen.

## Considered options

- **A prefix key, as in tmux (for example Ctrl+B).** Any prefix takes a key
  from the child program, and Ctrl+B is the prefix of tmux itself. It is
  also a mode, a hidden state between two keys, which the workbench refuses
  elsewhere.
- **A key that only leaves the terminal (a double Escape, Ctrl+Shift+Escape).**
  A double Escape delays every Escape of vim. Ctrl+Shift+Escape opens the
  Task Manager on Windows, and a page cannot take it.
- **Ctrl+Shift+P for the palette.** Edge and Firefox open a private window
  on it, and a page cannot take it.
- **One field for the palette and the search, as with `>` in some editors.**
  It mixes two jobs, and it changes `/`, which people already use.
- **Keys a person can change.** It can come later through the list of D2,
  as a setting kept per browser and checked by D8. It is not needed now,
  and fixed keys keep the help and the docs true for everyone.

## Compliance

- D1, D8: checked by `crates/ralphy-daemon/ui-tests/wb-commands.test.mjs`
  (the list holds no refused key and no key twice).
- D2: checked by `ast-grep/rules/page-shortcut-in-the-list.yml`: an
  Alt+Shift test or a typed `/` or `?` outside `wb-commands.ts` fails, with
  no exception. The four listeners of the Context moved into the list.
- D3: checked by `crates/ralphy-daemon/ui-tests/wb-console-press.test.mjs`
  (a terminal gives a chord to the page only when its command can run now).
- D4, D5, D6, D7, D9, D11: not checked by code: manual, reviewed in the PR
  and by a browser check in `tests/browser`.
- D10: not checked by code: manual, reviewed in the PR.
