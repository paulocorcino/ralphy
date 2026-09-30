use super::super::ssh::tests::{out, FakeHost};
use super::super::ssh::SshError;
use super::*;

const LINE: &str = "ssh-ed25519 AAAAC3NzaBODY ralphy-peer@anvil";

fn denied() -> HostOutput {
    out(
        255,
        "",
        "root@10.1.1.4: Permission denied (publickey,password).\r\n",
    )
}

fn kind(e: &anyhow::Error) -> Option<SshFailure> {
    e.chain()
        .find_map(|c| c.downcast_ref::<SshError>())
        .map(|s| s.kind)
}

#[test]
fn read_drops_one_final_line_break_only() {
    let pw = Password::read(" s3cret \r\n".as_bytes()).unwrap();
    assert_eq!(pw.expose(), " s3cret ");
    let pw = Password::read("a\n\n".as_bytes()).unwrap();
    assert_eq!(pw.expose(), "a\n");
    assert!(Password::read("\n".as_bytes()).is_err());
    assert_eq!(format!("{pw:?}"), "Password(<hidden>)");
}

#[test]
fn decide_answers_the_password_prompt_once() {
    let mut a = Answers::default();
    assert!(a.decide("root@10.1.1.4's password:"));
    // The host asks again after a wrong password; the same answer is not sent
    // twice.
    assert!(!a.decide("root@10.1.1.4's password:"));
    assert_eq!(a.unhandled, None);

    let mut a = Answers::default();
    assert!(!a.decide("Verification code:"));
    assert_eq!(a.unhandled.as_deref(), Some("Verification code:"));
    assert!(a.decide("(user@mac) Password:"));

    let mut a = Answers::default();
    assert!(a.decide("Password:"));
    assert!(!a.decide("New password:"));
    assert_eq!(a.unhandled.as_deref(), Some("New password:"));
}

#[test]
fn serve_answers_only_the_askpass_that_has_the_nonce() {
    let pw = Password::read("s3cret".as_bytes()).unwrap();
    let (replies, unhandled) = serve(&pw, |env| {
        assert!(
            env.iter().all(|(_, v)| !v.contains("s3cret")),
            "the password is in the environment: {env:?}"
        );
        assert!(env
            .iter()
            .any(|(k, v)| k == "SSH_ASKPASS_REQUIRE" && v == "force"));
        let (_, value) = env
            .iter()
            .find(|(k, _)| k == ASKPASS_ENV)
            .expect("the askpass variable");
        let (port, _) = value.split_once(':').expect("port:nonce");
        let wrong = format!("{port}:{}", "0".repeat(64));
        Ok([
            request(&wrong, "Password:").map(|r| r.to_string()).ok(),
            request(value, "Verification code:")
                .map(|r| r.to_string())
                .ok(),
            request(value, "Password:").map(|r| r.to_string()).ok(),
        ])
    })
    .unwrap();
    assert_eq!(replies, [None, None, Some("s3cret".to_string())]);
    assert_eq!(unhandled.as_deref(), Some("Verification code:"));
}

#[test]
fn add_peer_key_signs_in_with_the_password_and_appends_the_line() {
    let mut fake = FakeHost::default()
        .with_password()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("authorized_keys", out(0, "", ""));
    add_peer_key(&mut fake, "root@10.1.1.4", LINE).unwrap();
    assert_eq!(fake.password_commands().len(), 2, "{:?}", fake.commands());
    let append = &fake.calls[1];
    assert!(append.1.contains("grep -qF AAAAC3NzaBODY"), "{}", append.1);
    assert_eq!(append.2, format!("{LINE}\n").into_bytes());
}

#[test]
fn add_peer_key_on_a_windows_administrator_writes_the_shared_file() {
    let mut fake = FakeHost::default()
        .with_password()
        .answer("uname -s", out(1, "", "'uname' is not recognized"))
        .answer(
            "cmd /c ver",
            out(0, "Microsoft Windows [Version 10.0.26200]", ""),
        )
        .answer(
            "--- groups",
            out(0, "BUILTIN\\Administrators S-1-5-32-544", ""),
        )
        .answer("authorized_keys", out(0, "", ""));
    add_peer_key(&mut fake, "admin@win", LINE).unwrap();
    let append = fake.calls.last().expect("an append");
    assert!(
        append.1.contains("administrators_authorized_keys") && append.1.contains("icacls"),
        "{}",
        append.1
    );
}

#[test]
fn a_refused_password_stops_after_one_attempt() {
    let mut fake = FakeHost::default()
        .with_password()
        .answer("uname -s", denied());
    let err = add_peer_key(&mut fake, "root@10.1.1.4", LINE).unwrap_err();
    assert_eq!(kind(&err), Some(SshFailure::PasswordRefused), "{err:#}");
    assert!(
        format!("{err:#}").contains("refused the password"),
        "{err:#}"
    );
    assert_eq!(fake.calls.len(), 1, "{:?}", fake.commands());
}

#[test]
fn a_second_factor_prompt_is_reported_as_a_prompt() {
    let mut fake = FakeHost {
        prompt: Some("Verification code:".to_string()),
        ..FakeHost::default()
    }
    .with_password();
    let err = add_peer_key(&mut fake, "root@10.1.1.4", LINE).unwrap_err();
    assert_eq!(kind(&err), Some(SshFailure::Prompt), "{err:#}");
}
