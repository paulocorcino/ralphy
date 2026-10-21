//! The act bar of the changes and projects panels: refusals, the lock and the remove action.

use super::*;

/// A refusal of an act dispatched from the Changes panel must render IN that
/// panel. It did not: every one of those handlers reported through
/// `_flashAction`, whose only renderer is inside `aside.runs`
/// (`x-show="runsOpen"`, closed by default) — so `sync push` could answer
/// "cannot push: this branch has no remote to push to" and the operator saw
/// a button that did nothing. These are the CI-visible pins over that fix,
/// on the same bargain as the #331 gate above: no Playwright in CI.
#[test]
fn a_refused_change_act_reports_in_the_changes_panel() {
    let html = include_str!("../../assets/ui/index.html");
    for pin in [
        r#"class="chg-error""#,
        r#"x-show="changesError""#,
        r#"x-text="changesError""#,
        r#"data-act="chg-error-dismiss""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // The note is pinned BELOW the compose box and ABOVE the remote bar, in
    // the strip of the panel that never scrolls: a refusal parked behind a
    // long change list is the defect again, wearing a different hat.
    let compose = html.find(r#"class="chg-compose""#).expect("compose box");
    let err = html.find(r#"class="chg-error""#).expect("error note");
    let bar = html.find(r#"class="chg-bar""#).expect("remote bar");
    assert!(
        compose < err && err < bar,
        "the refusal note sits between the compose box and the remote bar"
    );

    // That every act in the panel lands its refusal in `changesError` and
    // still flashes it is driven by `ui-tests/app.test.mjs`.

    let css = served_css();
    assert!(
        css.contains(".chg-error {") && css.contains(".chg-error span {"),
        "the note must be styled and its text bounded, like .runs-verb-error"
    );
    assert!(
        css.contains("color: var(--danger)"),
        "the note carries the token palette's danger colour, not a literal"
    );
}

/// A remote act in flight must SAY so. It did not: a click on Push sent
/// the verb and the button sat unchanged for the whole round trip, so an
/// operator read it as dead and clicked again — racing a second push
/// against the first. One `syncBusy` slot locks the whole bar and swaps
/// the busy act's icon for a ring. Pinned here on the #331 bargain: no
/// Playwright in CI.
#[test]
fn a_remote_act_in_flight_locks_the_bar_and_shows_a_ring() {
    let html = include_str!("../../assets/ui/index.html");
    for pin in [
        r#"data-act="fetch" :disabled="$store.connection.writeLocked() || !!syncBusy""#,
        r#"data-act="pull" :disabled="$store.connection.writeLocked() || !!syncBusy || !!pullBlocked()""#,
        r#"data-act="push" :disabled="$store.connection.writeLocked() || !!syncBusy""#,
        r#":class="{ busy: syncBusy === 'fetch' }""#,
        r#":class="{ busy: syncBusy === 'pull' }""#,
        r#":class="{ busy: syncBusy === 'push' }""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // One ring per act, a sibling of the icon that the CSS swaps in.
    assert_eq!(
        html.matches(r#"<span class="bar-spinner" aria-hidden="true"></span>"#)
            .count(),
        3,
        "each of the three remote acts carries its own ring"
    );

    // That each act takes the slot, refuses while another holds it and frees
    // it on every exit is driven by `ui-tests/app.test.mjs`.

    let css = served_css();
    for pin in [
        ".bar-act .bar-spinner {",
        ".bar-act.busy svg {",
        ".bar-act.busy .bar-spinner {",
        ".bar-act.busy:disabled {",
    ] {
        assert!(css.contains(pin), "the stylesheet must keep the pin {pin}");
    }
}

/// On a branch with no upstream, Pull has nothing to pull from until the
/// branch is published, so it leaves the bar and Publish takes its width.
/// "Publish branch" did not fit in a third of the bar, and the centred
/// label was cut on both sides.
#[test]
fn pull_leaves_the_bar_while_the_branch_has_no_upstream() {
    let html = include_str!("../../assets/ui/index.html");
    let pull = html
        .split(r#"data-act="pull""#)
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .expect("index.html has a Pull button in the remote bar");
    assert!(
        pull.contains(r#"x-show="!pullBlocked()""#),
        "Pull must be hidden while pullBlocked() names a reason: {pull}"
    );
    assert_eq!(
        html.matches(r#"<span class="bar-label""#).count(),
        3,
        "each of the three remote acts wraps its label for the ellipsis"
    );

    let css = served_css();
    let label = css
        .split(".bar-act .bar-label {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("the stylesheet styles .bar-act .bar-label");
    assert!(
        label.contains("text-overflow: ellipsis"),
        "a label that does not fit ends in an ellipsis: {label}"
    );
}

/// The same defect on the branch chip, which lives in the PROJECTS panel:
/// `_mutateBranch` reverted the optimistic chip and sent its reason to
/// `_flashAction`, so a refused switch was a chip that snapped back saying
/// nothing. Its own state and its own gutter — dressing a branch refusal as
/// `treeError` would make a working project read as an unreadable one.
#[test]
fn a_refused_branch_change_reports_in_the_projects_panel() {
    let html = include_str!("../../assets/ui/index.html");
    for pin in [
        r#"class="files-error branch-error""#,
        r#"class="files-error branch-error" x-show="$store.checkouts.branchError""#,
        r#"x-text="$store.checkouts.branchError""#,
    ] {
        assert!(html.contains(pin), "index.html must keep the pin {pin}");
    }
    // Under the Files bar, above the tree: the answer sits beside the row
    // whose chip asked, not below rows that can run off screen.
    let head = html
        .find(r#"class="side-head files-sec""#)
        .expect("files bar");
    let err = html
        .find(r#"class="files-error branch-error""#)
        .expect("note");
    let host = html.find(r#"class="wb-host""#).expect("tree host");
    assert!(
        head < err && err < host,
        "the branch refusal sits between the files bar and the tree"
    );

    let app_js = shell_family();
    for pin in [
        "branchError: \"\"",
        "_branchRefused(msg: string) {",
        "this.branchError = msg || \"\";",
    ] {
        assert!(app_js.contains(pin), "the shell must keep the pin {pin}");
    }
    // Both `_mutateBranch` arms report: the refusal AND, in daemon mode, the
    // transport throw. The throw is the arm that used to be deliberately
    // silent, and it is the one that leaves the optimistic chip standing —
    // silence there is the chip claiming a switch nobody confirmed.
    // Both arms are pinned below, inside `_mutateBranch`. There is NO
    // client-side refusal under a selected worktree any more (#407): the
    // act is SENT with the checkout.
    let mutate = squeeze(js_method_body(
        &app_js,
        "async _mutateBranch(verb: string, slug: string | null, name: string, revert: () => void) {",
    ));
    assert!(
        mutate.contains(
            r#"this._branchRefused("Couldnotreachthedaemon.Checkwhetherthebranchchanged.")"#
        ),
        "the daemon-mode throw arm must report; an unanswered branch change must not read as a completed one"
    );
    assert!(
        app_js.contains("WBDaemon.withCheckout({ repo: slug, name }, this.$store.checkouts.checkoutOf(slug))"),
        "a branch act under a selected worktree is sent WITH the checkout — the worktree's HEAD moves, never the primary's"
    );
    assert!(
        !app_js.contains("pick primary before switching branches"),
        "the #406 client-side refusal is gone"
    );
    // The create lives in the console's prompt (wb-console-checkout.ts): an
    // unanswered add re-opens it with the same honest line.
    assert!(
        console_family().contains(
            r#"error = "Could not reach the daemon. Check whether the worktree was created.";"#
        ),
        "an unanswered worktree create must not read as a completed one"
    );
    assert!(
        app_js.contains(
            r#"failure = "Could not reach the daemon. Check whether the worktree was deleted.";"#
        ),
        "an unanswered worktree remove must not read as a completed one"
    );
    // The revert is on the REFUSAL arm only: a throw may have landed, and
    // reverting a switch that happened would put a lie in the chip.
    assert!(
        mutate.contains("revert();this._branchRefused(WBFail.failed(reply,"),
        "only a refusal reverts the optimistic chip, and it reports"
    );

    assert!(
        served_css().contains(".branch-error {"),
        "the branch note bounds its own text: it is the CLI's prose, above the tree"
    );
}

/// The picker row's remove action (#409, ADR-0063 §4): the `.stop` is
/// load-bearing — the row's own click selects the checkout and closes the
/// modal, so without it a remove is also a select-and-dismiss.
#[test]
fn the_worktree_row_remove_action_stops_the_selecting_click() {
    // The rows are the checkout menu's (wb-console-checkout.ts `checkoutMenu`),
    // opened from the Files bar's chip with `onRemove` (ADR-0063
    // amendment 2026-09-16 b): the trash is its own element whose click
    // stops before the row's own pick, and the primary row never has one.
    let console_js = console_family();
    assert!(console_js.contains(r#"trash.className = "session-checkout-remove";"#));
    assert!(
        console_js.contains("if (onRemove && !row.primary) {"),
        "the primary tree has no remove action"
    );
    let trash_click = console_js
        .find("trash.addEventListener(\"click\", (e) => {")
        .expect("the trash has a click handler");
    assert!(
        console_js[trash_click..trash_click + 200].contains("e.stopPropagation();"),
        "the remove action must stop the row's selecting click"
    );
    let html = include_str!("../../assets/ui/index.html");
    assert!(
        html.contains(r#"onRemove: (row) => this.removeWorktree(ref, row),"#)
            || shell_family().contains("onRemove: (row) => this.removeWorktree(ref, row),"),
        "the Files chip's menu wires the remove action"
    );
    let app_js = shell_family();
    assert!(
        app_js.contains("WBProject.checkoutAfterListing(ck, listing) === null"),
        "the selection resets from the re-read listing, never from the reply's status"
    );
    let remove = app_js
        .find("async removeWorktree(slug: string, w: CheckoutRow) {")
        .expect("removeWorktree exists");
    let body = &app_js[remove..];
    let ask = body
        .find("await this.$store.modals.askConfirm({")
        .expect("a worktree remove asks the operator first");
    let call = body
        .find(r#"observe("worktree.remove""#)
        .expect("removeWorktree sends worktree.remove");
    assert!(ask < call, "the confirm comes before the daemon call");
    assert!(
        body[ask..call].contains("if (!ok || this._removing(slug, w.name)) return;"),
        "a cancel makes no daemon call"
    );
    assert!(
        served_css().contains(".session-checkout-remove {"),
        "the remove action is styled in the served CSS"
    );
}
