# Text field

A one-line input or a native `<select>`. Class `.text-field`; `.set-input`
and `.host-input` are older names for the same rule.

Layer: element (atom). Made of: no smaller part.

## When to use it

- Use `.set-input` in a settings row (`.set-row`) and in the security and
  release dialogs.
- Use `.host-input` in a form with a label above each field: the Hosts
  dialog and the Add a project dialog.
- Use a [Segmented control](./segmented-control.md) instead of a `<select>`
  when there are two to four values and they fit on screen.
- Use a plain checkbox for a yes or no value. `01-base.css` draws every
  checkbox.

## Reference markup

A settings row, from `wb-settings-dialog.part.html`:

```html
<div class="set-row">
  <div class="set-main">
    <label class="set-label" x-text="it.label"></label>
    <select class="set-input" @change="saveSetting(it.key, $event.target.value)">
      …
    </select>
  </div>
  <p class="set-help" x-text="…"></p>
</div>
```

A form field, from `wb-hosts-dialog.part.html`:

```html
<input type="text" id="host-user" class="host-input" :value="addHost.user"
       :readonly="!!addHost.alias" …>
```

## Spacing

Measured in Chromium on 2026-11-14. Padding 5.6px 8px, 28px high, for every name. Radius 4px. A settings row pads 11.2px top and bottom.

## Modes

27 to 28px high in every mode.

## Variants

| Class                | Notes                                                                |
| -------------------- | -------------------------------------------------------------------- |
| `.text-field`        | The shared look: `--log-bg`, `0.82rem`, padding 5.6px 8px, 28px high |
| `.set-input`         | Also `min-width: 220px`, for a settings row                          |
| `.set-input.set-num` | `100px` wide, for a number                                           |
| `.set-input.mono`    | Monospace, full width                                                |
| `.host-input`        | Width from its grid in the Hosts dialog                              |

## States

| State     | What the code does                                                        |
| --------- | ------------------------------------------------------------------------- |
| Default   | `--border-field` edge (3:1 on the field and the modal), radius `--radius` |
| Focus     | The focus ring: 1px `--focus-ring`, offset 1px; the edge stays            |
| Disabled  | `opacity: 0.6`, cursor `not-allowed`                                      |
| Read-only | `.host-input[readonly]`: text in `--text-muted`                           |
| Empty     | A `placeholder` attribute, for example "Current password"                 |
| Error     | No style on the field. A `.side-error` line shows the message             |
| Loading   | Not used on the field                                                     |

## Files

- `styles/08-modals.css`: `.set-row`, `.set-main`, `.set-label`,
  `.set-input`, `.set-help`
- `styles/03-runs.css`: `.host-input`, `.host-field`, `.host-grid`
- `wb-settings-dialog.part.html`, `wb-security-dialog.part.html`,
  `wb-release-dialogs.part.html`, `wb-hosts-dialog.part.html`,
  `wb-add-project-dialog.part.html`

## Keyboard

See [Keyboard](./keyboard.md).
