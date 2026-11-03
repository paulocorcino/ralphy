//! Which fence holds a console or a note card. Membership is derived from the
//! geometry, never stored (ADR-0051 §6, amended by #743): a record belongs to
//! the first fence that holds the middle of its title bar. The workbench owns
//! this rule (`fenceOf` in `assets/ui/wb-geometry.ts`); this is its second
//! spelling, for the desk history only, and the shared fixture
//! `ui-tests/fixtures/api-desk--fence-membership-cases.json` keeps the two the same.

use super::{DeskFence, DeskRect};

/// Half of the title bar height (`--row-h`, 33 px), as `TITLE_MID` in the
/// workbench.
const TITLE_MID: f64 = 16.0;

/// The middle of the title bar: the horizontal centre, and the top plus half
/// the title bar, at most half the height.
fn title_bar_point(rect: &DeskRect) -> (f64, f64) {
    (
        rect.left + rect.width / 2.0,
        rect.top + TITLE_MID.min(rect.height / 2.0),
    )
}

/// Half-open, so two fences that touch never both hold a point on the edge
/// they share.
fn holds(fence: &DeskRect, (x, y): (f64, f64)) -> bool {
    x >= fence.left
        && x < fence.left + fence.width
        && y >= fence.top
        && y < fence.top + fence.height
}

/// The id of the first fence that holds `rect`, or `None`.
pub fn fence_of<'a>(fences: &'a [DeskFence], rect: &DeskRect) -> Option<&'a str> {
    let point = title_bar_point(rect);
    fences
        .iter()
        .find(|f| holds(&f.rect, point))
        .map(|f| f.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        fences: Vec<DeskFence>,
        rect: DeskRect,
        fence: Option<String>,
    }

    #[test]
    fn every_shared_case_finds_the_fence_the_workbench_finds() {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../ui-tests/fixtures/api-desk--fence-membership-cases.json"
        ))
        .expect("the shared fixture is valid JSON");
        assert!(cases.len() >= 8, "the table lost cases: {}", cases.len());
        for c in &cases {
            assert_eq!(
                fence_of(&c.fences, &c.rect),
                c.fence.as_deref(),
                "{}",
                c.name
            );
        }
    }
}
