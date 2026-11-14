# Keyboard

The keyboard shortcuts of the workbench and the keys each component
answers. Read from the code on 2026-11-14. A new shortcut gets a row here
in the same change as its code.

## The chord rule

A page shortcut uses **Alt+Shift** with no Ctrl and no Meta. The check is
`altShiftOnly` in `wb-console-press.ts`. Keys are matched on `e.code`, so
the keyboard layout does not change them. A terminal (xterm) gives these
chords to the page instead of to the child program (`pageTakesChord`).
Every other key in a terminal goes to the child program.

A shortcut does nothing when `$store.modals.consoleShortcutsBlocked()` is
true: no login, a dialog is open, or a text field has focus
(`wb-modals-store.ts`). A key that finds nothing to do is not swallowed.

## Page shortcuts

| Keys                        | Act                                                               | Code                       |
| --------------------------- | ----------------------------------------------------------------- | -------------------------- |
| `/`                         | Open the search: files when a project is open, projects otherwise | `wb-wire.ts`               |
| Ctrl+Shift+F (Cmd on macOS) | Open the files search, also from an editor                        | `wb-wire.ts`               |
| Alt+Shift+0 to 9            | The row with that digit in the Consoles menu                      | `wb-consoles-tab-menus.ts` |
| Alt+Shift+R                 | Open the Consoles menu on the command field                       | `wb-consoles-tab-menus.ts` |
| Alt+Shift+F1 to F12         | Go to fence 1 to 12                                               | `wb-consoles-tab-menus.ts` |
| Alt+Shift+arrows            | Walk the columns and fences                                       | `wb-consoles-tab.ts`       |
| Ctrl+Insert                 | Copy the selection of a terminal                                  | `wb-console-press.ts`      |
| Shift+wheel                 | Scroll the stage sideways                                         | `wb-stage-view.ts`         |

Ctrl+Shift+C is not used: in Chrome and Edge it opens the developer
tools, and a page cannot take it. Ctrl+C belongs to the child program.

## Keys inside components

| Component                                   | Keys                                                                                                                                       |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| [Modal](./modal.md)                         | Escape closes the top modal. Enter confirms a confirm dialog when no button has focus. Tab and Shift+Tab stay inside the top modal         |
| [Dropdown menu](./dropdown.md)              | Up and Down move between the enabled rows and wrap at the ends; Home and End go to the first and last row; Escape closes the checkout menu |
| [Text field](./text-field.md)               | Enter saves and Escape cancels in the title, fence name and note rename fields                                                             |
| [Segmented control](./segmented-control.md) | Tab reaches each segment. In a tab list (`role="tablist"`), Left and Right move between the tabs                                           |
| [Button](./button.md)                       | Enter and Space, from the browser                                                                                                          |
| [Tab](./tab.md)                             | Tab reaches the tab bar; Left and Right move between tabs, Home and End to the ends; Enter or Space opens a tab                            |
| Files tree                                  | F2 renames. Alt+Enter shows the properties                                                                                                 |
| Note editor                                 | Ctrl+S (Cmd+S) saves. Escape leaves the editor                                                                                             |
| Markdown search                             | Enter: next match. Shift+Enter: the one before. Escape closes                                                                              |
| Add a project dialog                        | Up and Down move in the list. Enter or Tab picks                                                                                           |

## Where the keys are shown

A menu row shows its shortcut at the right in a [kbd hint](./kbd.md). A
button shows it in its `title`, for example "Search files (Ctrl+Shift+F)".

The arrow keys of every menu (`.dropdown`, `.ctxmenu`) and every tab list
are one document listener, in `wb-arrow-keys.ts`. A text field inside a
menu keeps its own arrow keys.
