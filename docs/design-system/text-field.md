# Text field

A one-line input or a native `<select>`. There are two classes today:
`.set-input` and `.host-input`.

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

## Variants

| Class                | Ground     | Size      | Notes                      |
| -------------------- | ---------- | --------- | -------------------------- |
| `.set-input`         | `--log-bg` | `0.82rem` | `min-width: 220px`         |
| `.set-input.set-num` | `--log-bg` | `0.82rem` | `100px` wide, for a number |
| `.set-input.mono`    | `--log-bg` | `0.82rem` | Monospace, full width      |
| `.host-input`        | `--bg`     | `0.8rem`  | Width from its grid        |

The two classes have the same job and different values. This is a
difference found in the audit, not a decision. Do not add a third class.

## States

| State     | What the code does                                                                     |
| --------- | -------------------------------------------------------------------------------------- |
| Default   | `--border` hairline, radius `--radius`                                                 |
| Focus     | `.set-input`: outline off, border `--border-focus`. `.host-input`: the browser outline |
| Disabled  | `.set-input`: `opacity: 0.6`, `not-allowed`. `.host-input`: `opacity: 0.6`             |
| Read-only | `.host-input[readonly]`: text in `--text-muted`                                        |
| Empty     | A `placeholder` attribute, for example "Current password"                              |
| Error     | No style on the field. A `.side-error` line shows the message                          |
| Loading   | Not used on the field                                                                  |

## Files

- `styles/08-modals.css`: `.set-row`, `.set-main`, `.set-label`,
  `.set-input`, `.set-help`
- `styles/03-runs.css`: `.host-input`, `.host-field`, `.host-grid`
- `wb-settings-dialog.part.html`, `wb-security-dialog.part.html`,
  `wb-release-dialogs.part.html`, `wb-hosts-dialog.part.html`,
  `wb-add-project-dialog.part.html`

## Keyboard

See Keyboard section.
