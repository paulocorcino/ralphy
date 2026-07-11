//! `ralphy daemon`: run the resident daemon in the foreground (docs/adr/0032),
//! plus `daemon setup` (interactive baptism) and `daemon status`. The CLI is the
//! composition root — it installs a plain tracing stack for readable foreground
//! logs and hands off to `ralphy-daemon`, where the async runtime lives.
//! Baptism is interactive stdin, so it lives in `setup`, never in the resident
//! foreground process which must not block on stdin. `install`/`uninstall` (OS
//! autostart, mirroring `schedule`) come in later slices.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use ralphy_core::git;
use ralphy_daemon::identity::{self, avatar_by_number, format_status_line, validate_name, AVATARS};
use ralphy_daemon::registry;
use ralphy_daemon::{auth, password, totp};

#[derive(Args)]
pub(crate) struct DaemonArgs {
    /// TCP port for the local listener.
    #[arg(long, default_value_t = ralphy_daemon::DEFAULT_PORT)]
    pub(crate) port: u16,

    /// Interface to bind. Defaults to 127.0.0.1 (loopback only). A non-localhost
    /// bind is an explicit opt-in that REQUIRES an access token minted by
    /// `ralphy daemon setup`, or the daemon refuses to start (docs/adr/0032 §4).
    #[arg(long, default_value = "127.0.0.1")]
    pub(crate) bind: std::net::IpAddr,

    #[command(subcommand)]
    pub(crate) command: Option<DaemonCommand>,
}

#[derive(Subcommand)]
pub(crate) enum DaemonCommand {
    /// Baptize the daemon: pick a name (hostname-derived default) and an avatar,
    /// minting the daemon_id on first run.
    Setup,
    /// Show the daemon's identity ("avatar name") and the listener hint.
    Status,
    /// Register a repo with the daemon by path (idempotent).
    Add {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Remove a repo from the registry by `owner/repo` slug (idempotent).
    Remove {
        #[arg(value_name = "SLUG")]
        slug: String,
    },
    /// Register the daemon for OS autostart at logon (Task Scheduler / systemd
    /// user unit).
    Install,
    /// Remove the daemon's autostart registration (idempotent).
    Uninstall,
}

pub(crate) fn run(args: &DaemonArgs) -> Result<()> {
    match &args.command {
        None => {
            init_tracing();
            ralphy_daemon::run(ralphy_daemon::DaemonConfig {
                port: args.port,
                bind: args.bind,
            })
        }
        Some(DaemonCommand::Setup) => setup(args.port),
        Some(DaemonCommand::Status) => status(args.port),
        Some(DaemonCommand::Add { path }) => {
            let repo = git::resolve_toplevel(path)?;
            let slug = git::project_slug(&repo);
            upsert_at(
                &registry::repos_toml_path()?,
                &slug,
                &repo.to_string_lossy(),
            )?;
            println!("registered {slug} → {}", repo.display());
            Ok(())
        }
        Some(DaemonCommand::Remove { slug }) => {
            let removed = remove_repo_at(&registry::repos_toml_path()?, slug)?;
            if removed {
                println!("removed {slug}");
            } else {
                println!("{slug} was not registered");
            }
            Ok(())
        }
        Some(DaemonCommand::Install) => {
            ralphy_daemon::autostart::install()?;
            println!("registered daemon autostart");
            Ok(())
        }
        Some(DaemonCommand::Uninstall) => {
            ralphy_daemon::autostart::uninstall()?;
            println!("removed daemon autostart");
            Ok(())
        }
    }
}

/// Load the registry at `registry_path`, upsert `(slug → path)`, and save it.
fn upsert_at(registry_path: &Path, slug: &str, path: &str) -> Result<()> {
    let mut store = registry::load_from(registry_path)?;
    store.upsert(slug, path);
    registry::save_to(&store, registry_path)
}

/// Load the registry at `registry_path`, remove `slug`, and save it. Returns
/// whether an entry was actually removed (idempotent for callers).
fn remove_repo_at(registry_path: &Path, slug: &str) -> Result<bool> {
    let mut store = registry::load_from(registry_path)?;
    let removed = store.remove(slug);
    registry::save_to(&store, registry_path)?;
    Ok(removed)
}

/// Resolve the slug from `repo_root` (CLI-side, since the daemon has no
/// `ralphy-core`) and upsert it into the registry at `registry_path`.
fn register_repo_at(registry_path: &Path, repo_root: &Path) -> Result<()> {
    let slug = git::project_slug(repo_root);
    upsert_at(registry_path, &slug, &repo_root.to_string_lossy())
}

/// Best-effort passive registration for the run/triage/init entry paths. AC5:
/// this MUST NEVER fail a run — the `()` return type structurally forbids
/// propagating an error; a failed write only logs a warning and the run
/// proceeds. Absent from the UI until the next successful write is acceptable.
pub(crate) fn register_repo(repo_root: &Path) {
    let result = registry::repos_toml_path().and_then(|p| register_repo_at(&p, repo_root));
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to register repo with the daemon; run proceeds");
    }
}

/// Interactive baptism: derive a default name from the hostname, run the console
/// over real stdin/stdout, then persist the identity (mint-once) and echo the
/// resulting status line.
fn setup(port: u16) -> Result<()> {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suggested = identity::suggest_name(&host);

    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    let mut stdout = std::io::stdout();
    let (name, avatar) = baptize_console(&mut locked, &mut stdout, &suggested)?;

    let id = identity::baptize(&identity::daemon_toml_path()?, name, avatar)?;
    writeln!(stdout, "\nbaptized: {}", format_status_line(&id))?;

    // Mint-once access token, independent of re-baptism: shown exactly once when
    // freshly minted, never re-echoed (a re-`setup` keeps the existing token).
    let (token, minted) = auth::ensure_token_at(&auth::token_path()?)?;
    if minted {
        writeln!(stdout, "access token (shown once): {token}")?;
    } else {
        writeln!(stdout, "access token: already set (not shown)")?;
    }

    // TOTP enrolment (issue #179): mint-once, shown exactly once; then the
    // opt-in login password (blank = skip). Both are the recommended-but-opt-in
    // hardening for a network bind (ADR-0032 §4).
    enroll_totp(&mut stdout, &totp::seed_path()?, &id.name)?;
    enroll_password(&mut locked, &mut stdout, &password::password_path()?)?;

    writeln!(stdout, "listener: http://127.0.0.1:{port}")?;
    Ok(())
}

/// Enrol the TOTP seed (mint-once) and, when freshly minted, print the
/// `otpauth://` URI, its terminal QR, and the base32 secret EXACTLY once. A later
/// `setup` finds the seed and prints only "already enrolled" — the secret never
/// re-prints. Returns whether it was newly minted.
fn enroll_totp<W: Write>(out: &mut W, seed_path: &Path, account: &str) -> Result<bool> {
    let (seed, minted) = totp::ensure_seed_at(seed_path)?;
    if minted {
        let uri = seed.otpauth_uri("ralphy", account);
        writeln!(
            out,
            "\nTOTP enrolled — scan this once with your authenticator:"
        )?;
        match qrcode::QrCode::new(uri.as_bytes()) {
            Ok(code) => {
                let rendered = code
                    .render::<qrcode::render::unicode::Dense1x2>()
                    .quiet_zone(true)
                    .build();
                writeln!(out, "{rendered}")?;
            }
            Err(e) => writeln!(out, "  (QR render unavailable: {e})")?,
        }
        writeln!(out, "  otpauth: {uri}")?;
        writeln!(out, "  secret:  {}", seed.secret_base32())?;
    } else {
        writeln!(out, "TOTP: already enrolled (not shown)")?;
    }
    Ok(minted)
}

/// Prompt for the OPTIONAL login password over the console: an empty line skips;
/// a non-blank line is PBKDF2-hashed and saved owner-only, overwriting any prior
/// password. The password is the weakest, opt-in second factor (ADR-0032 §4).
fn enroll_password<R: BufRead, W: Write>(input: &mut R, out: &mut W, path: &Path) -> Result<()> {
    writeln!(out, "\nset a login password? (blank = skip):")?;
    out.flush()?;
    let line = read_line(input)?;
    let pw = line.trim();
    if pw.is_empty() {
        writeln!(out, "password: skipped")?;
    } else {
        password::save_to(&password::Hash::hash_password(pw), path)?;
        writeln!(out, "password: set")?;
    }
    Ok(())
}

/// Print the daemon's identity and listener hint, or a setup hint when the
/// daemon has not been baptized yet.
fn status(port: u16) -> Result<()> {
    match identity::load_current()? {
        Some(id) => println!("{}", format_status_line(&id)),
        None => println!("not set up — run `ralphy daemon setup`"),
    }
    println!(
        "access token: {}",
        if auth::load_token()?.is_some() {
            "set"
        } else {
            "not set"
        }
    );
    println!(
        "totp: {}",
        if totp::load_seed()?.is_some() {
            "enrolled"
        } else {
            "not enrolled"
        }
    );
    println!(
        "password: {}",
        if password::load()?.is_some() {
            "set"
        } else {
            "not set"
        }
    );
    println!("listener: http://127.0.0.1:{port}");
    match ralphy_daemon::autostart::status() {
        Ok(st) if st.registered => println!("autostart: registered"),
        Ok(_) => println!("autostart: not registered"),
        Err(e) => {
            tracing::warn!(error = %e, "failed to probe daemon autostart status");
            println!("autostart: unknown");
        }
    }
    Ok(())
}

/// Drive the interactive baptism over `input`/`out`: prompt for a name
/// (defaulting to `suggested` on an empty line), re-prompting on any
/// [`validate_name`] error; then present the numbered avatar list and read a
/// number until [`avatar_by_number`] resolves. Returns the `(name, avatar)`.
fn baptize_console<R: BufRead, W: Write>(
    mut input: R,
    out: &mut W,
    suggested: &str,
) -> Result<(String, String)> {
    let name = loop {
        writeln!(out, "daemon name [{suggested}]:")?;
        out.flush()?;
        let line = read_line(&mut input)?;
        let raw = if line.trim().is_empty() {
            suggested
        } else {
            line.trim()
        };
        match validate_name(raw) {
            Ok(name) => break name,
            Err(e) => writeln!(out, "  {e}")?,
        }
    };

    writeln!(out, "\npick an avatar by number:")?;
    for (i, emoji) in AVATARS.iter().enumerate() {
        writeln!(out, "  {} {}", i + 1, emoji)?;
    }
    let avatar = loop {
        writeln!(out, "avatar number:")?;
        out.flush()?;
        let line = read_line(&mut input)?;
        match line.trim().parse::<usize>().ok().and_then(avatar_by_number) {
            Some(emoji) => break emoji.to_string(),
            None => writeln!(out, "  pick a number from 1 to {}", AVATARS.len())?,
        }
    };

    Ok((name, avatar))
}

/// Read one line. EOF (0 bytes) is an error, not an empty line, so an exhausted
/// script cannot spin the prompt loop forever waiting for input that will never
/// come.
fn read_line<R: BufRead>(input: &mut R) -> Result<String> {
    let mut buf = String::new();
    let n = input.read_line(&mut buf).context("reading console input")?;
    if n == 0 {
        anyhow::bail!("unexpected end of input during baptism");
    }
    Ok(buf)
}

/// Foreground logs to stderr: raw INFO `fmt` lines with local timestamps (the
/// same shape `run --verbose` prints), overridable via `RUST_LOG`/`RALPHY_LOG`.
/// No presenter — a resident process wants a scrollable log, not animation.
fn init_tracing() {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string()))
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_daemon::identity::Identity;
    use std::io::Cursor;
    use ulid::Ulid;

    #[test]
    fn baptism_refuses_reserved_then_accepts() {
        // "run" is reserved → refused; "anvil" accepted; avatar #3 → AVATARS[2].
        let input = Cursor::new(b"run\nanvil\n3\n".to_vec());
        let mut out: Vec<u8> = Vec::new();
        let (name, avatar) = baptize_console(input, &mut out, "suggested").unwrap();

        let printed = String::from_utf8(out).unwrap();
        assert!(
            printed.contains("run") && printed.to_lowercase().contains("reserved"),
            "the console must show a refusal naming `run`; got: {printed}"
        );
        assert_eq!(name, "anvil");
        assert_eq!(avatar, AVATARS[2].to_string());
    }

    #[test]
    fn empty_name_accepts_suggestion() {
        let input = Cursor::new(b"\n1\n".to_vec());
        let mut out: Vec<u8> = Vec::new();
        let (name, avatar) = baptize_console(input, &mut out, "anvil").unwrap();
        assert_eq!(name, "anvil");
        assert_eq!(avatar, AVATARS[0].to_string());
    }

    #[test]
    fn status_line_shows_avatar_then_name() {
        let id = Identity {
            id: Ulid::nil(),
            avatar: "🐙".into(),
            name: "anvil".into(),
        };
        assert_eq!(format_status_line(&id), "🐙 anvil");
    }

    #[test]
    fn register_repo_at_writes_entry() {
        // Path-explicit: a temp registry path + a temp repo dir, no env mutation.
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let repo_dir = tempfile::tempdir().unwrap();

        register_repo_at(&registry_path, repo_dir.path()).unwrap();

        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1, "exactly one entry written");
        let entry = store.repos.values().next().unwrap();
        assert_eq!(entry.path, repo_dir.path().to_string_lossy());
    }

    #[test]
    fn totp_enrolment_is_mint_once_and_shows_secret_once() {
        // Path-explicit (temp seed path, no env mutation), mirroring
        // `register_repo_at`. The secret is printed on the first mint and NEVER on
        // a later setup — the mint-once display invariant (AC1).
        let dir = tempfile::tempdir().unwrap();
        let seed_path = dir.path().join("daemon-totp");

        let mut out1: Vec<u8> = Vec::new();
        assert!(
            enroll_totp(&mut out1, &seed_path, "anvil").unwrap(),
            "first enrol mints"
        );
        let printed1 = String::from_utf8(out1).unwrap();
        let seed = totp::load_seed_from(&seed_path).unwrap().unwrap();
        assert!(
            printed1.contains(&seed.secret_base32()),
            "first mint prints the base32 secret; got: {printed1}"
        );
        assert!(
            printed1.contains("otpauth://"),
            "first mint prints the otpauth URI; got: {printed1}"
        );

        let mut out2: Vec<u8> = Vec::new();
        assert!(
            !enroll_totp(&mut out2, &seed_path, "anvil").unwrap(),
            "second enrol does not re-mint"
        );
        let printed2 = String::from_utf8(out2).unwrap();
        assert!(
            !printed2.contains(&seed.secret_base32()),
            "the secret must NOT re-print on a later setup; got: {printed2}"
        );
        assert!(
            printed2.contains("already enrolled"),
            "a re-enrol reports already enrolled; got: {printed2}"
        );
    }

    #[test]
    fn password_enrolment_skips_on_blank_and_saves_on_input() {
        use std::io::Cursor;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon-password");

        // Blank line → skipped, nothing written.
        let mut blank = Cursor::new(b"\n".to_vec());
        let mut out: Vec<u8> = Vec::new();
        enroll_password(&mut blank, &mut out, &path).unwrap();
        assert!(password::load_from(&path).unwrap().is_none(), "blank skips");
        assert!(String::from_utf8(out).unwrap().contains("skipped"));

        // A non-blank line → hashed and saved; the hash verifies.
        let mut typed = Cursor::new(b"hunter2\n".to_vec());
        let mut out: Vec<u8> = Vec::new();
        enroll_password(&mut typed, &mut out, &path).unwrap();
        let saved = password::load_from(&path)
            .unwrap()
            .expect("a password was saved");
        assert!(saved.verify("hunter2"), "the saved hash verifies the input");
        assert!(String::from_utf8(out).unwrap().contains("set"));
    }

    #[test]
    fn add_remove_idempotent() {
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");

        upsert_at(&registry_path, "owner/repo", "/some/path").unwrap();
        upsert_at(&registry_path, "owner/repo", "/some/path").unwrap();
        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1, "repeated upsert keeps one entry");

        assert!(
            remove_repo_at(&registry_path, "owner/repo").unwrap(),
            "first remove reports true"
        );
        assert!(
            !remove_repo_at(&registry_path, "owner/repo").unwrap(),
            "second remove reports false"
        );
    }
}
