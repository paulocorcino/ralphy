//! The cognitive complexity of Rust functions, read from the source text
//! (ADR-0081, amendment 2026-10-12). The rules are those of the SonarSource
//! specification ("Cognitive Complexity", G. Ann Campbell, 2018):
//!
//! - `if`, `match`, `for`, `while`, `loop`, a `let … else` and a
//!   `select!` add 1, plus 1 for each level they are nested in;
//! - `else` and `else if` add 1, with no nesting cost;
//! - each run of one logical operator adds 1 (`a && b && c` is 1,
//!   `a && b || c` is 2);
//! - `break 'label` and `continue 'label` add 1;
//! - a closure or an `async` block adds no score but one level of nesting.
//!
//! Every `#[cfg]` branch is read, so the score is the same on every
//! platform. A function defined inside another one is scored on its own.
//! Not counted: recursion, and the tokens of a macro that are not
//! expressions separated by commas (other than `select!`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use syn::parse::{ParseStream, Parser};
use syn::visit::{self, Visit};
use syn::{BinOp, Block, Expr, Item, Pat, Token};

/// One scored function.
pub(super) struct Found {
    pub(super) name: String,
    pub(super) value: u32,
    /// The function has the attribute [`super::EXCEPTION_ATTR`].
    pub(super) marked: bool,
}

/// The score of each function in `file`, by name. Items under `#[cfg(test)]`
/// and `#[test]` functions are not production code and are left out.
pub(super) fn file_scores(src: &str) -> Result<Vec<Found>> {
    let file = syn::parse_file(src)?;
    let mut out = Vec::new();
    items(&file.items, &mut out);
    Ok(out)
}

fn items(list: &[Item], out: &mut Vec<Found>) {
    for item in list.iter().filter(|item| !is_test(item_attrs(item))) {
        match item {
            Item::Fn(f) => function(&f.sig.ident, &f.attrs, &f.block, out),
            Item::Impl(i) => impl_fns(i, out),
            Item::Trait(t) => trait_fns(t, out),
            Item::Mod(m) => m.content.iter().for_each(|(_, inner)| items(inner, out)),
            _ => {}
        }
    }
}

/// The attributes of the items that can hold a function.
fn item_attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Fn(f) => &f.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Trait(t) => &t.attrs,
        Item::Mod(m) => &m.attrs,
        _ => &[],
    }
}

fn impl_fns(block: &syn::ItemImpl, out: &mut Vec<Found>) {
    let fns = block.items.iter().filter_map(|item| match item {
        syn::ImplItem::Fn(f) if !is_test(&f.attrs) => Some(f),
        _ => None,
    });
    for f in fns {
        function(&f.sig.ident, &f.attrs, &f.block, out);
    }
}

/// The trait methods that have a default body.
fn trait_fns(t: &syn::ItemTrait, out: &mut Vec<Found>) {
    let fns = t.items.iter().filter_map(|item| match item {
        syn::TraitItem::Fn(f) if !is_test(&f.attrs) => f.default.as_ref().map(|body| (f, body)),
        _ => None,
    });
    for (f, body) in fns {
        function(&f.sig.ident, &f.attrs, body, out);
    }
}

fn function(name: &syn::Ident, attrs: &[syn::Attribute], body: &Block, out: &mut Vec<Found>) {
    let mut score = Score::default();
    score.visit_block(body);
    out.push(Found {
        name: name.to_string(),
        value: score.total,
        marked: attrs
            .iter()
            .any(|a| a.path().is_ident(super::EXCEPTION_ATTR)),
    });
    items(&score.nested_items, out);
}

/// `#[cfg(test)]` or `#[test]`.
pub(super) fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test"))
    })
}

#[derive(Default)]
struct Score {
    total: u32,
    nesting: u32,
    /// Items inside the body, scored as their own functions.
    nested_items: Vec<Item>,
}

impl Score {
    /// A structure that nests: 1, plus the depth it sits at.
    fn structure(&mut self) {
        self.total += 1 + self.nesting;
    }

    fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        self.nesting += 1;
        f(self);
        self.nesting -= 1;
    }

    fn visit_if(&mut self, e: &syn::ExprIf) {
        self.visit_expr(&e.cond);
        self.nested(|s| s.visit_block(&e.then_branch));
        if let Some((_, other)) = &e.else_branch {
            // `else` and `else if` add 1 with no nesting cost.
            self.total += 1;
            match other.as_ref() {
                Expr::If(chained) => self.visit_if(chained),
                other => self.nested(|s| s.visit_expr(other)),
            }
        }
    }

    /// One logical expression tree: 1 for each run of one operator.
    fn visit_logical(&mut self, e: &Expr) {
        let mut ops = Vec::new();
        let mut leaves = Vec::new();
        flatten(e, &mut ops, &mut leaves);
        self.total += 1 + ops.windows(2).filter(|w| w[0] != w[1]).count() as u32;
        for leaf in leaves {
            self.visit_expr(leaf);
        }
    }

    /// `select!` counts as a `match`. Its tokens are not always arms
    /// (`select!` is also a name outside tokio), so `false` when they do
    /// not parse.
    fn visit_select(&mut self, tokens: proc_macro2::TokenStream) -> bool {
        let Ok(exprs) = select_exprs.parse2(tokens) else {
            return false;
        };
        self.structure();
        self.nested(|s| exprs.iter().for_each(|e| s.visit_expr(e)));
        true
    }

    fn visit_macro_tokens(&mut self, mac: &syn::Macro) {
        let last = mac.path.segments.last().map(|s| s.ident.to_string());
        if last.as_deref() == Some("select") && self.visit_select(mac.tokens.clone()) {
            return;
        }
        // Most macros (`format!`, `vec!`, `assert!`) take expressions
        // separated by commas. Tokens of another form are not scored.
        let parser = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated;
        if let Ok(exprs) = parser.parse2(mac.tokens.clone()) {
            exprs.iter().for_each(|e| self.visit_expr(e));
        }
    }
}

/// The expressions of a `select!`: an optional `biased;`, then arms
/// `pattern = future (, if guard)? => handler` and an optional
/// `else => handler`, each followed by an optional comma.
fn select_exprs(input: ParseStream) -> syn::Result<Vec<Expr>> {
    let mut exprs = Vec::new();
    let fork = input.fork();
    if fork.parse::<syn::Ident>().is_ok_and(|i| i == "biased") && fork.peek(Token![;]) {
        input.parse::<syn::Ident>()?;
        input.parse::<Token![;]>()?;
    }
    while !input.is_empty() {
        select_arm(input, &mut exprs)?;
        input.parse::<Option<Token![,]>>()?;
    }
    Ok(exprs)
}

fn select_arm(input: ParseStream, exprs: &mut Vec<Expr>) -> syn::Result<()> {
    if input.parse::<Option<Token![else]>>()?.is_none() {
        Pat::parse_multi_with_leading_vert(input)?;
        input.parse::<Token![=]>()?;
        exprs.push(input.parse()?);
        if input.peek(Token![,]) && input.peek2(Token![if]) {
            input.parse::<Token![,]>()?;
            input.parse::<Token![if]>()?;
            exprs.push(input.parse()?);
        }
    }
    input.parse::<Token![=>]>()?;
    exprs.push(input.parse()?);
    Ok(())
}

/// The operators and the non-logical operands of a tree of `&&` and `||`,
/// left to right. Parentheses do not end a run.
fn flatten<'a>(e: &'a Expr, ops: &mut Vec<bool>, leaves: &mut Vec<&'a Expr>) {
    match e {
        Expr::Binary(b) if logical(&b.op).is_some() => {
            flatten(&b.left, ops, leaves);
            ops.extend(logical(&b.op));
            flatten(&b.right, ops, leaves);
        }
        Expr::Paren(p) if matches!(&*p.expr, Expr::Binary(b) if logical(&b.op).is_some()) => {
            flatten(&p.expr, ops, leaves);
        }
        other => leaves.push(other),
    }
}

/// `Some(true)` for `&&`, `Some(false)` for `||`.
fn logical(op: &BinOp) -> Option<bool> {
    match op {
        BinOp::And(_) => Some(true),
        BinOp::Or(_) => Some(false),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for Score {
    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::If(i) => {
                self.structure();
                self.visit_if(i);
            }
            Expr::Match(m) => {
                self.structure();
                self.visit_expr(&m.expr);
                self.nested(|s| m.arms.iter().for_each(|a| s.visit_arm(a)));
            }
            Expr::ForLoop(f) => {
                self.structure();
                self.visit_expr(&f.expr);
                self.nested(|s| s.visit_block(&f.body));
            }
            Expr::While(w) => {
                self.structure();
                self.visit_expr(&w.cond);
                self.nested(|s| s.visit_block(&w.body));
            }
            Expr::Loop(l) => {
                self.structure();
                self.nested(|s| s.visit_block(&l.body));
            }
            Expr::Closure(c) => self.nested(|s| s.visit_expr(&c.body)),
            Expr::Async(a) => self.nested(|s| s.visit_block(&a.block)),
            Expr::Binary(b) if logical(&b.op).is_some() => self.visit_logical(e),
            Expr::Break(b) => {
                self.total += u32::from(b.label.is_some());
                visit::visit_expr_break(self, b);
            }
            Expr::Continue(c) => self.total += u32::from(c.label.is_some()),
            Expr::Macro(m) => self.visit_macro_tokens(&m.mac),
            other => visit::visit_expr(self, other),
        }
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        let Some(init) = &l.init else {
            return;
        };
        self.visit_expr(&init.expr);
        if let Some((_, diverge)) = &init.diverge {
            // `let … else` is an `if` whose `else` must leave.
            self.structure();
            self.nested(|s| s.visit_expr(diverge));
        }
    }

    fn visit_stmt_macro(&mut self, m: &'ast syn::StmtMacro) {
        self.visit_macro_tokens(&m.mac);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        self.nested_items.push(item.clone());
    }
}

/// The production `.rs` files of the crate at `crate_dir`: every module its
/// roots declare, followed through `mod x;` and `#[path]`, except a module
/// under `#[cfg(test)]`. The roots are `src/lib.rs`, `src/main.rs` and each
/// `src/bin/*.rs` that is not a test helper child.
pub(super) fn production_files(crate_dir: &Path) -> Result<Vec<PathBuf>> {
    let src = crate_dir.join("src");
    let mut roots: Vec<PathBuf> = ["lib.rs", "main.rs"]
        .iter()
        .map(|r| src.join(r))
        .filter(|p| p.is_file())
        .collect();
    let bin = src.join("bin");
    if bin.is_dir() {
        for entry in
            std::fs::read_dir(&bin).with_context(|| format!("reading {}", bin.display()))?
        {
            let path = entry.context("reading a bin entry")?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".rs") && !name.ends_with("_test_child.rs") {
                roots.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for root in roots {
        let dir = root.parent().map(Path::to_path_buf).unwrap_or_default();
        walk(&root, &dir, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Adds `file` and the files of the modules it declares. `dir` is where its
/// child modules live.
fn walk(file: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let parsed = syn::parse_file(&text).with_context(|| format!("parsing {}", file.display()))?;
    out.push(file.to_path_buf());
    declared(&parsed.items, file, dir, out)
}

fn declared(items: &[Item], file: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mods = items.iter().filter_map(|item| match item {
        Item::Mod(m) if !is_test(&m.attrs) => Some(m),
        _ => None,
    });
    for m in mods {
        let name = m.ident.to_string();
        match &m.content {
            Some((_, inner)) => declared(inner, file, &dir.join(&name), out)?,
            None => {
                let child = module_file(m, &name, file, dir);
                walk(&child, &module_dir(&child), out)?;
            }
        }
    }
    Ok(())
}

/// The file of `mod name;`: its `#[path]`, relative to the directory of the
/// declaring file, or `name.rs`, or `name/mod.rs`.
fn module_file(m: &syn::ItemMod, name: &str, file: &Path, dir: &Path) -> PathBuf {
    let base = file.parent().unwrap_or(dir);
    match path_attr(&m.attrs) {
        Some(rel) => base.join(rel),
        None if dir.join(format!("{name}.rs")).is_file() => dir.join(format!("{name}.rs")),
        None => dir.join(name).join("mod.rs"),
    }
}

/// Where the child modules of `file` live: its own directory for a
/// `mod.rs`, else a directory named like the file.
fn module_dir(file: &Path) -> PathBuf {
    match (file.file_name().and_then(|n| n.to_str()), file.parent()) {
        (Some("mod.rs"), Some(parent)) => parent.to_path_buf(),
        _ => file.with_extension(""),
    }
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("path"))
        .and_then(|a| {
            let syn::Meta::NameValue(nv) = &a.meta else {
                return None;
            };
            let Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            else {
                return None;
            };
            Some(s.value())
        })
}

#[cfg(test)]
mod tests;
