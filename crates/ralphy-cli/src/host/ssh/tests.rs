use std::path::{Path, PathBuf};

use anyhow::Result;

use super::*;

/// A host that answers from a table: the first entry whose needle the command
/// contains wins. It records every call, so a test can assert what was sent
/// and in which order.
#[derive(Default)]
pub(crate) struct FakeHost {
    pub answers: Vec<(String, HostOutput)>,
    pub calls: Vec<(Option<PathBuf>, String, Vec<u8>)>,
}

impl FakeHost {
    pub(crate) fn answer(mut self, needle: &str, out: HostOutput) -> Self {
        self.answers.push((needle.to_string(), out));
        self
    }

    pub(crate) fn commands(&self) -> Vec<&str> {
        self.calls.iter().map(|c| c.1.as_str()).collect()
    }

    /// The index of the first recorded command that contains `needle`.
    pub(crate) fn index_of(&self, needle: &str) -> Option<usize> {
        self.calls.iter().position(|c| c.1.contains(needle))
    }
}

impl HostShell for FakeHost {
    fn run(&mut self, identity: Option<&Path>, command: &str, stdin: &[u8]) -> Result<HostOutput> {
        self.calls.push((
            identity.map(Path::to_path_buf),
            command.to_string(),
            stdin.to_vec(),
        ));
        let pos = self
            .answers
            .iter()
            .position(|(needle, _)| command.contains(needle.as_str()))
            .unwrap_or_else(|| panic!("the fake host has no answer for {command:?}"));
        // A needle answers once when a later entry has the same needle, so a
        // test can script "refused, then accepted".
        let later = self.answers[pos + 1..]
            .iter()
            .any(|(n, _)| *n == self.answers[pos].0);
        if later {
            Ok(self.answers.remove(pos).1)
        } else {
            Ok(self.answers[pos].1.clone())
        }
    }
}

pub(crate) fn out(code: i32, stdout: &str, stderr: &str) -> HostOutput {
    HostOutput {
        code: Some(code),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    }
}

#[test]
fn ssh_argv_is_strict_and_batch() {
    let argv = ssh_argv(Path::new("ssh"), "svrapp", None, "uname -s");
    assert!(
        argv.iter().any(|a| a == "StrictHostKeyChecking=yes"),
        "{argv:?}"
    );
    assert!(argv.iter().any(|a| a == "BatchMode=yes"), "{argv:?}");
    let dash = argv.iter().position(|a| a == "--").expect("a -- separator");
    assert_eq!(argv[dash + 1], "svrapp");
    assert_eq!(argv.last().map(String::as_str), Some("uname -s"));
    assert!(!argv.iter().any(|a| a == "-i"), "{argv:?}");

    let argv = ssh_argv(
        Path::new("ssh"),
        "svrapp",
        Some(Path::new("peer_ed25519")),
        "uname -s",
    );
    let i = argv.iter().position(|a| a == "-i").expect("-i");
    assert_eq!(argv[i + 1], "peer_ed25519");
    assert!(argv.iter().any(|a| a == "IdentitiesOnly=yes"), "{argv:?}");
    let dash = argv.iter().position(|a| a == "--").expect("a -- separator");
    assert!(i < dash);
}

#[test]
fn classify_host_key_unknown() {
    // format of OpenSSH 9 with StrictHostKeyChecking=yes
    let o = out(
        255,
        "",
        "No ED25519 host key is known for svrapp and you have requested strict checking.\r\nHost key verification failed.\r\n",
    );
    assert_eq!(classify(&o), Some(SshFailure::HostKeyUnknown));
}

#[test]
fn classify_host_key_changed() {
    let o = out(
        255,
        "",
        "@@@@@@@@\r\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\r\n@@@@@@@@\r\nHost key verification failed.\r\n",
    );
    assert_eq!(classify(&o), Some(SshFailure::HostKeyChanged));
}

#[test]
fn classify_auth_refused() {
    let o = out(255, "", "paulo@svrapp: Permission denied (publickey).\r\n");
    assert_eq!(classify(&o), Some(SshFailure::AuthRefused));
}

#[test]
fn classify_unreachable() {
    for err in [
        "ssh: connect to host svrapp port 22: Connection refused",
        "ssh: connect to host 10.0.0.9 port 22: Connection timed out",
        "ssh: Could not resolve hostname svrapp: No such host is known.",
        "ssh: connect to host 10.0.0.9 port 22: No route to host",
    ] {
        assert_eq!(
            classify(&out(255, "", err)),
            Some(SshFailure::Unreachable),
            "{err}"
        );
    }
}

#[test]
fn classify_other_and_the_remote_commands_own_error() {
    assert_eq!(
        classify(&out(
            255,
            "",
            "kex_exchange_identification: read: Connection reset"
        )),
        Some(SshFailure::Other)
    );
    assert_eq!(classify(&out(1, "", "Permission denied")), None);
    assert_eq!(classify(&out(0, "Linux\n", "")), None);
}
