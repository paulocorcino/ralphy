use std::cell::Cell;

use ralphy_daemon::peer::PEER_PROTOCOL_VERSION;
use ralphy_release::{Asset, Release};

use super::*;
use crate::host::checks::tests::description;
use crate::host::checks::{evaluate, CheckStatus};
use crate::host::pair;
use crate::host::ssh::tests::{out, FakeHost};
use crate::host::ssh::HostOutput;

const BINARY: &[u8] = b"\x7fELF the ralphy binary \x00\xff";
const LINUX_PROBE: &str =
    "--- uid\n1000\n--- user\npaulo\n--- arch\nx86_64\n--- linger\nLinger=yes\n";

fn local<'a>(store: &'a Path, build: &str, target: &'static str) -> Local<'a> {
    Local {
        store,
        daemon_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FD0".to_string()),
        name: Some("anvil".to_string()),
        port: 7401,
        build: Build::parse(build),
        target: Some(target),
    }
}

/// A [`Fetch`] that hands out fixed bytes and records what it was asked for.
#[derive(Default)]
struct Fixed {
    asked: Vec<(Source, String)>,
}

impl Fetch for Fixed {
    fn binary(&mut self, source: &Source, target: &str) -> Result<Vec<u8>> {
        self.asked.push((source.clone(), target.to_string()));
        Ok(BINARY.to_vec())
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn json(d: &ralphy_daemon::peer::DaemonDescription) -> HostOutput {
    out(
        0,
        &serde_json::to_string(d).expect("a description serializes"),
        "",
    )
}

fn clap_refusal() -> HostOutput {
    out(2, "", "error: unrecognized subcommand 'describe'\n")
}

/// A Linux host whose first `describe` answers `first`, and whose installed
/// binary then describes `after`.
fn linux_host(probe: &str, first: HostOutput, after: HostOutput, hash: &str) -> FakeHost {
    FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, probe, ""))
        .answer("daemon describe", first)
        .answer("daemon describe", after)
        .answer(
            "cat > ",
            out(
                0,
                &format!("{hash}  /home/paulo/.ralphy/bin/ralphy.part\n"),
                "",
            ),
        )
        .answer("chmod +x", out(0, "", ""))
        .answer("daemon install", out(0, "", ""))
        .answer("daemon restart", out(0, "", ""))
}

fn no_keygen(_: &Path) -> Result<()> {
    panic!("the peer key must not be generated here")
}

fn run(fake: &mut FakeHost, local: &Local<'_>, fetch: &mut Fixed) -> (Result<()>, String) {
    let mut printed = Report::text(Vec::new());
    let result = install(fake, local, "svrapp", None, no_keygen, fetch, &mut printed);
    let printed = String::from_utf8(printed.into_inner()).expect("utf-8");
    (result, printed)
}

fn sent_a_binary(fake: &FakeHost) -> bool {
    fake.commands().iter().any(|c| c.contains("ralphy.part"))
}

#[test]
fn an_old_ralphy_gets_the_exact_bytes_and_its_daemon_moves_to_them() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = linux_host(
        LINUX_PROBE,
        clap_refusal(),
        json(&description("linux")),
        &sha(BINARY),
    );
    let mut fetch = Fixed::default();
    let (result, printed) = run(
        &mut fake,
        &local(store.path(), "v0.1.0-rc.30", "linux-x64"),
        &mut fetch,
    );
    result.expect("installed");

    assert_eq!(
        fetch.asked,
        vec![(Source::ThisComputer, "linux-x64".to_string())]
    );
    let write = fake.index_of("cat > ").expect("the binary was written");
    assert_eq!(fake.calls[write].2, BINARY, "the bytes on stdin");
    let commit = fake.index_of("chmod +x").expect("committed");
    let installed = |args: &str| {
        fake.commands()
            .iter()
            .position(|c| c.contains(&format!(r#"bin/ralphy" {args}"#)) && !c.contains("if [ -x"))
    };
    let describe = installed("daemon describe").expect("the installed binary described");
    let autostart = installed("daemon install").expect("autostart re-registered");
    let restart = installed("daemon restart").expect("the daemon restarted");
    assert!(
        write < commit && commit < describe && describe < autostart && autostart < restart,
        "{:?}",
        fake.commands()
    );
    for sent in fake.commands() {
        assert!(
            !sent.contains("require-token") && !sent.contains("setup"),
            "{sent}"
        );
    }
    assert!(printed.contains("still the old version"), "{printed}");
}

#[test]
fn a_missing_ralphy_starts_nothing() {
    let store = tempfile::tempdir().unwrap();
    let mut after = description("linux");
    after.running = false;
    after.autostart = false;
    let mut fake = linux_host(
        LINUX_PROBE,
        out(127, "", "sh: 1: ralphy: not found\n"),
        json(&after),
        &sha(BINARY),
    );
    let (result, printed) = run(
        &mut fake,
        &local(store.path(), "v0.1.0-rc.30", "linux-x64"),
        &mut Fixed::default(),
    );
    result.expect("installed");
    assert!(
        fake.index_of("daemon install").is_none(),
        "{:?}",
        fake.commands()
    );
    assert!(
        fake.index_of("daemon restart").is_none(),
        "{:?}",
        fake.commands()
    );
    assert!(printed.contains("add ~/.ralphy/bin to PATH"), "{printed}");
}

#[test]
fn a_copy_with_another_hash_replaces_nothing() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = linux_host(
        LINUX_PROBE,
        clap_refusal(),
        json(&description("linux")),
        &sha(b"a truncated copy"),
    );
    let (result, _) = run(
        &mut fake,
        &local(store.path(), "v0.1.0-rc.30", "linux-x64"),
        &mut Fixed::default(),
    );
    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("nothing was replaced"), "{err}");
    assert!(fake.index_of("chmod +x").is_none(), "{:?}", fake.commands());
}

#[test]
fn nothing_is_sent_when_the_install_must_not_happen() {
    let mut newer = description("linux");
    newer.protocol_version = PEER_PROTOCOL_VERSION + 1;
    let arm_probe = LINUX_PROBE.replace("x86_64", "aarch64");
    let missing = || out(127, "", "sh: 1: ralphy: not found\n");
    let cases: [(&str, HostOutput, &str, &str, &'static str); 4] = [
        (
            "a newer host",
            json(&newer),
            LINUX_PROBE,
            "v0.1.0-rc.30",
            "linux-x64",
        ),
        (
            "no build for the target",
            missing(),
            &arm_probe,
            "v0.1.0-rc.30",
            "linux-x64",
        ),
        (
            "a development build for another target",
            missing(),
            LINUX_PROBE,
            "v0.1.0-rc.30-4-gabc1234",
            "windows-x64",
        ),
        (
            "a host that already connects",
            json(&description("linux")),
            LINUX_PROBE,
            "v0.1.0-rc.30",
            "linux-x64",
        ),
    ];
    for (why, first, probe, build, target) in cases {
        let store = tempfile::tempdir().unwrap();
        let mut fake = linux_host(probe, first, json(&description("linux")), &sha(BINARY));
        let mut fetch = Fixed::default();
        let (result, printed) = run(&mut fake, &local(store.path(), build, target), &mut fetch);
        assert!(!sent_a_binary(&fake), "{why}: {:?}", fake.commands());
        assert!(fetch.asked.is_empty(), "{why}");
        match why {
            "a host that already connects" => {
                assert!(
                    result.is_ok() && printed.contains("Nothing was installed"),
                    "{why}"
                )
            }
            _ => assert!(result.is_err(), "{why}"),
        }
    }
}

#[test]
fn each_host_maps_to_its_release_target() {
    assert_eq!(target_of(HostOs::Linux, Some("x86_64")), Ok("linux-x64"));
    assert_eq!(target_of(HostOs::MacOs, Some("arm64")), Ok("macos-arm64"));
    assert_eq!(target_of(HostOs::MacOs, Some("x86_64")), Ok("macos-x64"));
    assert_eq!(target_of(HostOs::Windows, Some("AMD64")), Ok("windows-x64"));
    let arm = target_of(HostOs::Linux, Some("aarch64")).unwrap_err();
    assert!(arm.contains("cargo"), "{arm}");
    assert!(
        target_of(HostOs::Windows, Some("ARM64")).is_err(),
        "not measured"
    );
    assert!(target_of(HostOs::Linux, None).is_err());
}

#[test]
fn a_development_build_offers_only_its_own_target() {
    let store = tempfile::tempdir().unwrap();
    let facts = parse_facts(HostOs::Linux, LINUX_PROBE);
    let dev = local(store.path(), "v0.1.0-rc.30-4-gabc1234", "windows-x64");
    let ralphy = RalphyOnHost::Missing;
    let mut checks = evaluate(&facts, &ralphy, &[], "svrapp", None);
    assert_eq!(offer_for(&mut checks, &facts, &ralphy, &dev), None);
    let row = checks.iter().find(|c| c.id == CheckId::Ralphy).unwrap();
    assert_eq!(
        row.status,
        CheckStatus::Warn,
        "no command installs Ralphy by hand"
    );
    assert!(
        row.text.starts_with(
            "Ralphy is not installed on the host. This computer runs a development build"
        ),
        "{row:?}"
    );
    assert!(row.text.contains("from source"), "{row:?}");

    let same = local(store.path(), "v0.1.0-rc.30-4-gabc1234", "linux-x64");
    let own = offer(&facts, &same.build, same.target).expect("its own target");
    assert_eq!(own.source, Source::ThisComputer);
    let release = local(store.path(), "v0.1.0-rc.30", "windows-x64");
    let other = offer(&facts, &release.build, release.target).expect("a release");
    assert_eq!(other.source, Source::Release("v0.1.0-rc.30".to_string()));
    assert_eq!(other.target, "linux-x64");
}

#[test]
fn check_announces_what_install_would_send() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_PROBE, ""))
        .answer(
            "daemon describe",
            out(127, "", "sh: 1: ralphy: not found\n"),
        );
    let mut printed = Report::json(Vec::new());
    pair::check(
        &mut fake,
        &local(store.path(), "v0.1.0-rc.30", "windows-x64"),
        "svrapp",
        None,
        None,
        no_keygen,
        &mut printed,
    )
    .expect("checked");
    let text = String::from_utf8(printed.into_inner()).unwrap();
    let event: serde_json::Value = text
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|e| e["event"] == "install")
        .unwrap_or_else(|| panic!("no install event in {text}"));
    assert_eq!(
        event,
        serde_json::json!({
            "event": "install",
            "version": "v0.1.0-rc.30",
            "target": "linux-x64",
            "source": "release",
            "folder": "~/.ralphy/bin",
        })
    );
}

/// A release that carries a Windows archive holding `payload`, and its hash.
fn windows_release(payload: &[u8]) -> (Release, Vec<u8>, String) {
    let mut zip_bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        zip.start_file::<_, ()>(
            "ralphy-v0.1.0-rc.30-windows-x64/ralphy.exe",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("entry");
        std::io::Write::write_all(&mut zip, payload).expect("write");
        zip.finish().expect("finish");
    }
    let name = "ralphy-v0.1.0-rc.30-windows-x64.zip";
    let asset = |name: &str, size: u64| Asset {
        name: name.to_string(),
        browser_download_url: format!("https://example.invalid/{name}"),
        size,
    };
    let release = Release {
        tag_name: "v0.1.0-rc.30".to_string(),
        name: None,
        published_at: None,
        html_url: String::new(),
        prerelease: true,
        draft: false,
        body: None,
        assets: vec![
            asset(name, zip_bytes.len() as u64),
            asset(&format!("{name}.sha256"), 90),
        ],
    };
    let hash = sha(&zip_bytes);
    (release, zip_bytes, hash)
}

#[test]
fn a_release_archive_is_downloaded_once_and_checked_each_time() {
    let store = tempfile::tempdir().unwrap();
    let (release, zip_bytes, hash) = windows_release(b"the windows binary");
    let archive_downloads = Cell::new(0);
    let fetch = |url: &str, _: Option<u64>| -> Result<Vec<u8>> {
        if url.ends_with(".sha256") {
            return Ok(format!("{hash}  ralphy.zip\n").into_bytes());
        }
        archive_downloads.set(archive_downloads.get() + 1);
        Ok(zip_bytes.clone())
    };
    let get = || {
        release_binary(
            store.path(),
            "v0.1.0-rc.30",
            "windows-x64",
            |_| Ok(release.clone()),
            fetch,
        )
    };
    assert_eq!(get().expect("first"), b"the windows binary");
    assert_eq!(get().expect("second"), b"the windows binary");
    assert_eq!(archive_downloads.get(), 1, "the second read uses the cache");

    let cached = store
        .path()
        .join("host-install")
        .join("ralphy-v0.1.0-rc.30-windows-x64.zip");
    std::fs::write(&cached, b"a damaged archive").unwrap();
    assert_eq!(get().expect("third"), b"the windows binary");
    assert_eq!(
        archive_downloads.get(),
        2,
        "a damaged cache is downloaded again"
    );
}
