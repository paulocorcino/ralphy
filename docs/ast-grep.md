# ast-grep

ast-grep reads code as a syntax tree. `npx -y -p @ast-grep/cli@0.51.1 ast-grep`
runs it at the version the repo pins (below, `sg`). It sees syntax only: no
types, no imports, nothing across files. A match is a candidate; the compiler
and the linters decide what it means.

## Look before you read

Before reading a whole source file, outline it, then read only the range you
need:

    sg outline <file>                    # items and their members, with line numbers
    sg outline <dir> --items exports     # the public surface of a folder
    sg outline <file> --items imports    # what a file depends on

## Search by shape

Search with ast-grep when the question is about the shape of code: every call
of a function with a literal argument, every `catch` with an empty body, every
`impl` of a trait. Search with grep when it is about text: a name, a message, a
comment, a file.

    sg run -p '<pattern>' -l <language> <paths>
    sg run -p '<pattern>' -l <language> <paths> --json=stream   # one match per line, to count

The pattern is code of the language: `$X` matches one node, `$$$ARGS` any
number of them. `-l` takes `rust` or `ts` here.

A pattern that matches nothing is not proof the code is absent:

- A path is part of the pattern: `Command::new($P)` does not match
  `std::process::Command::new(...)`. Write the path as a metavariable
  (`$C::new($P)`) and read the matches.
- `--debug-query=pattern` prints how ast-grep parsed the pattern; test it on a
  snippet you know contains the code.

## Rewrite

One shape changed at many sites:

    sg run -p '<old>' -r '<new>' -l <language> <paths>      # prints the diff, changes nothing
    sg run -p '<old>' -r '<new>' -l <language> <paths> -U   # applies it

Read the diff before `-U`. The rewrite drops the line breaks of the code it
replaces: run `cargo fmt --all` (Rust) or the oxfmt step of the gate
(TypeScript) after it. A rewrite goes in a commit of its own, with the command
in its message, and the gate passes after it.

## The gate's rules

The rules are in `ast-grep/rules`, one per file, and their tests in
`ast-grep/tests`. `cargo run -q -p xtask -- gate` runs `sg scan` and `sg test`.

- A rule's message says what to do at the site: do it.
- A new rule copies the form of one here, with `valid` cases (each false
  positive found) and `invalid` cases (each form of the failure), and
  `sg test --skip-snapshot-tests` passes. For rule syntax, read the page you
  need from `https://ast-grep.github.io/llms.txt`.
- An exception is `// ast-grep-ignore: <rule-id> -- <why>` on the line above,
  the reason saying why the rule is wrong there. Every ignore names its rule;
  the gate fails on one that suppresses nothing. A comment that only mentions
  the directive starts with another word: one whose first word is
  `ast-grep-ignore`, in any comment form, is a directive.
