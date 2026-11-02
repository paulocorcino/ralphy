//! The extraction rules and the known misses that the `ui-copy` report prints.

pub(super) const RULES: &[&str] = &[
    "HTML: static `title`, `aria-label` and `placeholder` values; text nodes outside `<script>` \
      and `<style>` (this includes `<title>`), whitespace squeezed, entities decoded, skipped when \
      they hold no letter.",
    "Alpine: `:title`, `:aria-label`, `:placeholder` (or `x-bind:`) and `x-text`, read as a \
      JavaScript expression.",
    "An expression becomes one row per thing it can show: a ternary gives its two branches (never \
      its condition), `||`/`??` give each side, `&&` gives its right side. Inside a row, a \
      non-literal part is written `{expr}` and the row is flagged concatenated.",
    "JavaScript sinks: the `title`/`message`/`text`/`label`/`confirmLabel`/`cancelLabel`/\
      `placeholder`/`hint`/`caption`/`tooltip`/`ariaLabel`/`help`/`blurb` keys of an object \
      literal (`js:toast` inside `toast(…)`, `js:confirm` inside `askConfirm`/`askNotice`/\
      `askPrompt`/`askInShell`, else `js:property`); `window.confirm`/`prompt`/`alert`; \
      `.textContent`/`.innerText`/`.innerHTML =` (an `innerHTML` value that holds elements is read \
      as HTML, one row per element and attribute; otherwise tags are stripped); `.title`/\
      `.placeholder`/`.ariaLabel =` and `setAttribute(\"title\"|\"aria-label\"|\"placeholder\", \
      …)`; `term.write(…)`; the key bar's `key(name, html, title)`; `this.x = …` when the text has \
      a space (`js:state`); `const NAME = …` with a SCREAMING_CASE name when the text has a space \
      and does not start with `(` or `[` (`js:const`); `return` inside a function whose name ends \
      in Title/Label/Text/Hint/Tooltip/Message/Caption, or is listed in `copy_helpers` of `docs/\
      ui-copy-rules.json` (`js:helper`); the argument of a call that `copy_calls` names, such as \
      the text of `paintState(el, text)`, or of a method when the name is `Owner.method` \
      (`WBFail.failed(reply, text)`, also as `window.WBFail.failed`) (`js:call`).",
    "A literal with no space that reads as code (kebab-case, a dotted name, a path, a selector) \
      is not copy.",
    "Pinned: a daemon test claim on the same asset (or on no named asset) with a literal that \
      holds the text or one of its literal parts. Text of two words or more counts when it is \
      equal, quoted (`\"…\"`, `'…'`, backticks, `>…<`), or (four words or more) anywhere in the \
      literal. One word counts only next to its sink: `title: \"…\"`, `title=\"…\"`, `textContent \
      = \"…\"`, `>…<`, or with its Alpine attribute in the same literal.",
];

pub(super) const MISSES: &[&str] = &[
    "Text built in a variable before it reaches a sink.",
    "Default values in a function signature (`confirmLabel = \"Confirm\"`).",
    "Helpers written as `const x = () => …`, and label maps indexed by a key.",
    "`x-html`, `alt`, CSS `content:`, and the icon names of `x-icon`.",
    "Text the daemon (Rust) produces and the UI shows verbatim. Out of scope for PRD #422.",
];
