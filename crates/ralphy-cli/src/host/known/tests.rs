use super::*;

// `ssh-keygen -q -t ed25519 -N "" -f k` then `ssh-keygen -lf k.pub`
// (OpenSSH for Windows via Git for Windows, 2026-09-29).
const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILWYuPYh+dmW8NM2aN7U13V46utUcbNLC84XS9ufQuy6";
const ED25519_FP: &str = "SHA256:cZGgFfiHAUCQuwJMFDKA5dXZxpWqAnRw39n/LNiUxxY";
const OTHER: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIHd69OUvOLWs4RXfBTXly96xze2F+N42g5AIGx51I79C";
const OTHER_FP: &str = "SHA256:/dPGCF75br6+caJT/spOJHGBFDog//mR2xZTZi5tVPw";

#[test]
fn fingerprint_matches_ssh_keygen() {
    assert_eq!(fingerprint(ED25519).unwrap(), ED25519_FP);
    assert_eq!(fingerprint(OTHER).unwrap(), OTHER_FP);
    let changed = ED25519.replacen("LWY", "LWZ", 1);
    assert_ne!(fingerprint(&changed).unwrap(), ED25519_FP);
    assert!(fingerprint("not base64!").is_err());
}

fn scan_text() -> String {
    format!(
        "# svrapp:22 SSH-2.0-OpenSSH_9.6\n\
         svrapp ssh-ed25519 {ED25519}\n\
         svrapp ssh-ed25519 {OTHER}\n"
    )
}

#[test]
fn parse_scan_skips_comments() {
    let scan = parse_scan(&scan_text());
    assert_eq!(scan.len(), 2);
    assert_eq!(scan[0].host_field, "svrapp");
    assert_eq!(scan[0].kind, "ssh-ed25519");
    assert_eq!(scan[0].blob, ED25519);
}

#[test]
fn lines_matching_keeps_only_the_key_shown() {
    let scan = parse_scan(&scan_text());
    assert_eq!(
        lines_matching(&scan, OTHER_FP, "svrapp").unwrap(),
        [format!("svrapp ssh-ed25519 {OTHER}")]
    );
    let err = lines_matching(&scan, "SHA256:nope", "svrapp").unwrap_err();
    assert!(err.to_string().contains("nothing was written"), "{err}");
}

#[test]
fn known_name_brackets_a_port_that_is_not_22() {
    assert_eq!(known_name("h", 22), "h");
    assert_eq!(known_name("h", 2222), "[h]:2222");
}

#[test]
fn append_separates_a_file_with_no_final_newline() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".ssh").join("known_hosts");
    append(&file, &["a k1".to_string()]).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "a k1\n");
    std::fs::write(&file, "old").unwrap();
    append(&file, &["b k2".to_string()]).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\nb k2\n");
}

#[test]
fn trust_scanned_writes_only_the_key_shown() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known_hosts");
    std::fs::write(&file, "old k0\n").unwrap();
    let scan = parse_scan(&scan_text());
    assert!(trust_scanned(&scan, "SHA256:nope", "svrapp", &file).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old k0\n");
    trust_scanned(&scan, OTHER_FP, "svrapp", &file).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("old k0\nsvrapp ssh-ed25519 {OTHER}\n")
    );
}

#[test]
fn unknown_reply_is_the_host_key_reply() {
    let v = unknown_reply("10.0.0.5", 2222, &parse_scan(&scan_text())).unwrap();
    assert_eq!(v["state"], "unknown");
    assert_eq!(v["host"], "10.0.0.5");
    assert_eq!(v["port"], 2222);
    assert_eq!(v["keys"][0]["type"], "ssh-ed25519");
    assert_eq!(v["keys"][0]["fingerprint"], ED25519_FP);
    assert_eq!(v["keys"][1]["fingerprint"], OTHER_FP);
    crate::golden::check("host.key", json!({ "status": "ok", "key": v }), &[]);
}
