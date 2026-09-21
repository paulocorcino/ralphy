use super::*;

#[test]
fn generate_token_is_64_hex_chars() {
    let token = generate_token();
    assert_eq!(token.len(), 64, "256 bits hex-encoded is 64 chars");
    assert!(
        token.chars().all(|c| c.is_ascii_hexdigit()),
        "token must be lowercase hex; got {token}"
    );
    assert_ne!(token, generate_token(), "two mints must differ");
}

#[test]
fn ensure_token_is_mint_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("daemon-token");
    let (first, minted) = ensure_token_at(&path).unwrap();
    assert!(minted, "first call mints");
    let (second, minted_again) = ensure_token_at(&path).unwrap();
    assert!(!minted_again, "second call does not re-mint");
    assert_eq!(first, second, "the same token is returned");
}

#[test]
fn load_token_from_missing_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load_token_from(&dir.path().join("absent")).unwrap(), None);
}

#[test]
fn save_then_load_trims_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon-token");
    save_token_to("abc123\n", &path).unwrap();
    assert_eq!(load_token_from(&path).unwrap(), Some("abc123".into()));
}
