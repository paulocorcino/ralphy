use super::*;

const LOGGED_OUT: &str = include_str!("../../tests/fixtures/logged_out.jsonl");

#[test]
fn the_auth_line_is_an_early_kill_marker() {
    assert!(is_auth_line(
        "Authentication required. Please visit the URL to log in:"
    ));
    for other in [
        "  https://accounts.google.com/o/oauth2/auth?access_type=offline",
        "Waiting for authentication (timeout 60s)...",
        "Or, paste the authorization code here and press Enter:",
        "root agent idle; waiting up to 30s for 1 background task(s)",
        "",
    ] {
        assert!(!is_auth_line(other), "{other}");
    }
}

#[test]
fn the_failed_sign_in_result_is_the_second_marker() {
    assert!(is_auth_error(LOGGED_OUT));
    assert!(!is_auth_error("error: print timeout after 30s"));
}

#[test]
fn the_auth_message_names_the_owned_root() {
    let root = Path::new("/home/op/.ralphy/antigravity-root");
    let message = auth_message(root);
    assert!(message.contains("agy --gemini_dir="), "{message}");
    assert!(message.contains(&root.display().to_string()), "{message}");
}

#[test]
fn the_login_probe_reads_sign_in_from_agy_models() {
    let stderr = concat!(
        "Fetching available models...\n",
        "Error: Please sign in to view available models. ",
        "Launch the CLI without arguments to sign in.\n",
    );
    assert!(models_reads_logged_out(Some(1), stderr));
    assert!(!models_reads_logged_out(
        Some(0),
        "Fetching available models...\n"
    ));
    assert!(
        !models_reads_logged_out(Some(1), "Error: connection refused\n"),
        "another failure is not an answer about the sign-in"
    );
}
