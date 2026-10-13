use super::*;

fn state_with_two_published() -> InitState {
    InitState {
        created_issues: vec![101, 102],
        ..InitState::default()
    }
}

#[test]
fn publish_remaining_refuses_when_the_draft_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path());
    let mut state = state_with_two_published();

    let err = publish_remaining(dir.path(), &ws, &mut state).unwrap_err();

    assert!(err.to_string().contains("is gone"), "got: {err}");
}

#[test]
fn publish_remaining_refuses_a_draft_shorter_than_the_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path());
    std::fs::create_dir_all(ws.ralphy_dir()).unwrap();
    std::fs::write(ws.issues_draft_path(), r#"{"issues":[]}"#).unwrap();
    let mut state = state_with_two_published();

    let err = publish_remaining(dir.path(), &ws, &mut state).unwrap_err();

    assert!(
        err.to_string().contains("no longer matches the checkpoint"),
        "got: {err}"
    );
}
