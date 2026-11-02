//! The environment label of this daemon: the WSL distro or the OS release.

/// The human label for an environment: the WSL distro when running inside one,
/// else the OS release (`Ubuntu 24.04`, `macOS 15`) when it was read, else a
/// presentable OS name. Pure so every branch is testable on any host.
///
/// The WSL label keeps the registered distro name verbatim: the workbench maps
/// a `\\wsl.localhost\<distro>\…` path to its peer by it. Windows keeps its
/// plain name.
pub fn environment_label(wsl_distro: Option<&str>, os: &str, release: Option<&str>) -> String {
    if let Some(d) = wsl_distro {
        return format!("WSL: {d}");
    }
    match (os, release) {
        ("windows", _) => "Windows".to_string(),
        (_, Some(release)) => release.to_string(),
        ("linux", None) => "Linux".to_string(),
        ("macos", None) => "macOS".to_string(),
        (other, None) => other.to_string(),
    }
}

/// The distro and version in an `os-release` file: `NAME` and `VERSION_ID`
/// (`Ubuntu 24.04`, `Debian 12`), or `NAME` alone for a rolling release
/// (`Arch Linux`). `None` without a `NAME`.
pub fn linux_release(os_release: &str) -> Option<String> {
    let field = |key: &str| {
        os_release.lines().find_map(|line| {
            let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let name = field("NAME")?;
    // Debian's NAME is `Debian GNU/Linux`; the family is already in the icon.
    let name = name.strip_suffix(" GNU/Linux").unwrap_or(&name).to_string();
    Some(match field("VERSION_ID") {
        Some(version) => format!("{name} {version}"),
        None => name,
    })
}

/// The macOS release from `sw_vers -productVersion`: the major version only
/// (`15.3.1` → `macOS 15`), since it changes with every update otherwise.
/// Before macOS 11 the major was always `10`, so `10.x` keeps its minor.
pub fn macos_release(product_version: &str) -> Option<String> {
    let mut parts = product_version.trim().split('.');
    let major = parts.next().filter(|p| !p.is_empty())?;
    if major == "10" {
        if let Some(minor) = parts.next() {
            return Some(format!("macOS 10.{minor}"));
        }
    }
    Some(format!("macOS {major}"))
}

/// This machine's OS release for [`environment_label`], read once per process.
/// `None` when it cannot be read; the label then falls back to the OS name.
pub fn system_release() -> Option<String> {
    static RELEASE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    RELEASE.get_or_init(read_system_release).clone()
}

#[cfg(target_os = "linux")]
fn read_system_release() -> Option<String> {
    // The os-release(5) lookup order.
    ["/etc/os-release", "/usr/lib/os-release"].iter().find_map(
        |path| match std::fs::read_to_string(path) {
            Ok(text) => linux_release(&text),
            Err(e) => {
                tracing::debug!(path, error = %e, "could not read the OS release");
                None
            }
        },
    )
}

#[cfg(target_os = "macos")]
fn read_system_release() -> Option<String> {
    let mut cmd = std::process::Command::new("sw_vers");
    cmd.arg("-productVersion");
    crate::child_env::remove_secrets(&mut cmd);
    match cmd.output() {
        Ok(out) if out.status.success() => macos_release(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => {
            tracing::debug!(status = %out.status, "sw_vers did not give the macOS version");
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "could not run sw_vers");
            None
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_system_release() -> Option<String> {
    None
}

/// The WSL distro this process runs in, from `WSL_DISTRO_NAME`.
fn wsl_distro() -> Option<String> {
    std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|d| !d.is_empty())
}

/// This process's environment label, from `WSL_DISTRO_NAME`, the target OS, and
/// the OS release. Inside WSL the release is never read: the distro name wins.
pub fn detect_environment() -> String {
    let distro = wsl_distro();
    let release = if distro.is_some() {
        None
    } else {
        system_release()
    };
    environment_label(distro.as_deref(), std::env::consts::OS, release.as_deref())
}
