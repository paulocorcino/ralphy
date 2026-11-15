# Keyboard

How the workbench shows its keys. The list of every shortcut, the rules
they follow and their conflicts with the terminal are in
[DESIGN-SYSTEM.md → Keyboard](../DESIGN-SYSTEM.md#keyboard). A new shortcut
gets a row there in the same change as its code.

## Where the keys are shown

A menu row shows its shortcut at the right in a [kbd hint](./kbd.md). A
button shows it in its `title`, for example "Search files (Ctrl+Shift+F)".

The arrow keys of every menu (`.dropdown`, `.ctxmenu`) and every tab list
are one document listener, in `wb-arrow-keys.ts`. A text field inside a
menu keeps its own arrow keys.

## Keys by component

| Component                                   | Where its keys are                                    |
| ------------------------------------------- | ----------------------------------------------------- |
| [Modal](./modal.md)                         | Escape, Tab and Shift+Tab; Enter in a confirm dialog  |
| [Dropdown menu](./dropdown.md)              | Up, Down, Home and End; Escape closes                 |
| [Context menu](./context-menu.md)           | Up, Down, Home and End; Escape closes                 |
| [Text field](./text-field.md)               | Enter saves and Escape cancels in the rename fields   |
| [Segmented control](./segmented-control.md) | Tab; Left and Right inside a tab list                 |
| [Tab](./tab.md)                             | Left, Right, Home and End; Enter or Space opens a tab |
| [Button](./button.md)                       | Enter and Space, from the browser                     |
