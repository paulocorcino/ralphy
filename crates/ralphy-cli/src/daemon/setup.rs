//! The interactive `ralphy daemon setup`: four numbered steps (name, avatar,
//! authenticator app, login password), then a summary.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use ralphy_daemon::identity::{self, avatar_by_number, format_status_line, validate_name, AVATARS};
use ralphy_daemon::{auth, password, totp};

/// Wrong codes accepted before the authenticator check is skipped.
const CODE_ATTEMPTS: usize = 3;
/// Time steps of clock skew accepted, as the daemon's sign-in does.
const CODE_SKEW_STEPS: i64 = 1;

/// The outcome of step 3, shown in the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Authenticator {
    Confirmed,
    Skipped,
    AlreadySet,
}

impl Authenticator {
    fn label(self) -> &'static str {
        match self {
            Authenticator::Confirmed => "confirmed",
            Authenticator::Skipped => "not confirmed (the same QR code stays valid)",
            Authenticator::AlreadySet => "already set",
        }
    }
}

/// Interactive setup over the real console: read the password without echo
/// when stdin is a terminal, and as plain lines otherwise.
pub(super) fn setup(port: u16) -> Result<()> {
    let host = hostname::get().map_or_default(|h| h.to_string_lossy().into_owned());
    let suggested = identity::suggest_name(&host);

    let stdin = std::io::stdin();
    let hidden = stdin.is_terminal();
    let mut locked = stdin.lock();
    let mut stdout = std::io::stdout();
    let (name, avatar) = baptize_console(&mut locked, &mut stdout, &suggested)?;

    let id = identity::baptize(&identity::daemon_toml_path()?, name, avatar)?;
    writeln!(stdout, "\nsaved: {}", format_status_line(&id))?;

    // Mint-once access token, independent of re-baptism: shown exactly once.
    let (token, minted) = auth::ensure_token_at(&auth::token_path()?)?;
    let token_line = if minted {
        format!("access token (shown once): {token}")
    } else {
        "access token: already set (not shown)".to_owned()
    };
    writeln!(stdout, "{token_line}")?;

    let authenticator = enroll_totp(&mut locked, &mut stdout, &totp::seed_path()?, &id.name)?;
    let password_set = if hidden {
        let term = console::Term::stdout();
        let mut read = || term.read_secure_line().context("reading the password");
        enroll_password(&mut read, &mut stdout, &password::password_path()?)?
    } else {
        let mut read = || read_line(&mut locked);
        enroll_password(&mut read, &mut stdout, &password::password_path()?)?
    };

    writeln!(stdout, "\nSetup is done.")?;
    writeln!(stdout, "  name:          {}", format_status_line(&id))?;
    writeln!(stdout, "  {token_line}")?;
    writeln!(stdout, "  authenticator: {}", authenticator.label())?;
    let pw = if password_set { "set" } else { "skipped" };
    writeln!(stdout, "  password:      {pw}")?;
    writeln!(stdout, "\nNext: open http://127.0.0.1:{port}")?;
    Ok(())
}

/// Steps 1 and 2: prompt for a name (an empty line takes `suggested`), then
/// an avatar number; both re-prompt until valid. Returns `(name, avatar)`.
pub(super) fn baptize_console<R: BufRead, W: Write>(
    mut input: R,
    out: &mut W,
    suggested: &str,
) -> Result<(String, String)> {
    writeln!(out, "Step 1 of 4: Name")?;
    writeln!(
        out,
        "The name tells this daemon apart from your other machines."
    )?;
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

    writeln!(out, "\nStep 2 of 4: Avatar")?;
    writeln!(out, "The avatar marks this daemon in the workbench.")?;
    for (i, emoji) in AVATARS.iter().enumerate() {
        writeln!(out, "  {} {}", i + 1, emoji)?;
    }
    let avatar = loop {
        writeln!(out, "avatar number:")?;
        out.flush()?;
        let line = read_line(&mut input)?;
        match line.trim().parse::<usize>().ok().and_then(avatar_by_number) {
            Some(emoji) => break emoji.to_owned(),
            None => writeln!(out, "  pick a number from 1 to {}", AVATARS.len())?,
        }
    };

    Ok((name, avatar))
}

/// Step 3: mint the seed once; when fresh, print the QR code, `otpauth://` URI
/// and secret exactly once, then ask for a code from the app to confirm it.
pub(super) fn enroll_totp<R: BufRead, W: Write>(
    input: &mut R,
    out: &mut W,
    seed_path: &Path,
    account: &str,
) -> Result<Authenticator> {
    writeln!(out, "\nStep 3 of 4: Authenticator app")?;
    writeln!(
        out,
        "A code from your phone protects the sign-in from other machines."
    )?;
    let (seed, minted) = totp::ensure_seed_at(seed_path)?;
    if !minted {
        writeln!(out, "authenticator: already set (not shown)")?;
        return Ok(Authenticator::AlreadySet);
    }
    let uri = seed.otpauth_uri("ralphy", account);
    writeln!(out, "Scan this QR code with your authenticator app:")?;
    match qrcode::QrCode::new(uri.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            writeln!(out, "{rendered}")?;
        }
        Err(e) => writeln!(out, "  (QR code not available: {e})")?,
    }
    writeln!(out, "  otpauth: {uri}")?;
    writeln!(out, "  secret:  {}", seed.secret_base32())?;
    confirm_code(input, out, &seed, unix_now)
}

/// Ask for the app's code until it verifies, an empty line skips, or the
/// attempts run out. A skip keeps the seed: the QR code above stays valid.
pub(super) fn confirm_code<R: BufRead, W: Write>(
    input: &mut R,
    out: &mut W,
    seed: &totp::Seed,
    now: impl Fn() -> u64,
) -> Result<Authenticator> {
    for _ in 0..CODE_ATTEMPTS {
        writeln!(out, "Type the 6-digit code your app shows (blank = skip):")?;
        out.flush()?;
        let line = read_line(input)?;
        let code = line.trim();
        if code.is_empty() {
            break;
        }
        if seed.verify(code, now(), CODE_SKEW_STEPS) {
            writeln!(out, "authenticator: confirmed")?;
            return Ok(Authenticator::Confirmed);
        }
        writeln!(
            out,
            "  That code is not right. Check the app and try again."
        )?;
    }
    writeln!(
        out,
        "authenticator: not confirmed. The app setup is saved and the QR code above \
         stays valid; sign in with a code from the app."
    )?;
    Ok(Authenticator::Skipped)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Step 4: the optional login password. `read` returns one entry (hidden on a
/// terminal); a blank entry skips, and the password is asked twice and must
/// match. Saved hashed and owner-only, over any earlier password. Returns
/// whether a password was set.
pub(super) fn enroll_password<W: Write>(
    read: &mut dyn FnMut() -> Result<String>,
    out: &mut W,
    path: &Path,
) -> Result<bool> {
    writeln!(out, "\nStep 4 of 4: Login password (optional)")?;
    writeln!(out, "A password adds one more check at sign-in.")?;
    loop {
        writeln!(out, "password (blank = skip):")?;
        out.flush()?;
        let first = read()?;
        let pw = first.trim();
        if pw.is_empty() {
            writeln!(out, "password: skipped")?;
            return Ok(false);
        }
        writeln!(out, "type the password again:")?;
        out.flush()?;
        if read()?.trim() == pw {
            password::save_to(&password::Hash::hash_password(pw), path)?;
            writeln!(out, "password: set")?;
            return Ok(true);
        }
        writeln!(out, "  The two passwords are not the same. Try again.")?;
    }
}

/// Read one line. EOF (0 bytes) is an error, not an empty line, so an exhausted
/// script cannot spin a prompt loop forever.
pub(super) fn read_line<R: BufRead>(input: &mut R) -> Result<String> {
    let mut buf = String::new();
    let n = input.read_line(&mut buf).context("reading console input")?;
    if n == 0 {
        anyhow::bail!("unexpected end of input during setup");
    }
    Ok(buf)
}

#[cfg(test)]
mod tests;
