use super::*;

fn spec() -> AutostartSpec {
    AutostartSpec {
        program: PathBuf::from("/usr/local/bin/ralphy"),
        log_path: PathBuf::from("/home/me/.ralphy/daemon.log"),
        wsl_distro: None,
        path: None,
    }
}

fn path_spec(path: &str) -> AutostartSpec {
    AutostartSpec {
        path: Some(path.to_string()),
        ..spec()
    }
}

fn wsl_spec(distro: &str) -> AutostartSpec {
    AutostartSpec {
        wsl_distro: Some(distro.to_string()),
        ..spec()
    }
}

#[test]
fn render_install_windows_runkey() {
    let joined = render_install(Platform::Windows, &spec()).join(" ");
    for needle in [
        "reg",
        "add",
        RUN_KEY,
        "/v",
        TASK_NAME,
        "REG_SZ",
        "-WindowStyle Hidden",
        "daemon",
        "*>>",
    ] {
        assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
    }
    assert!(!joined.contains("schtasks"), "{joined:?}");
    assert!(!joined.contains("ONLOGON"), "{joined:?}");
}

#[test]
fn render_uninstall_windows() {
    let joined = render_uninstall(Platform::Windows).join(" ");
    for needle in ["reg", "delete", RUN_KEY, "/v", TASK_NAME, "/f"] {
        assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
    }
}

#[test]
fn render_query_windows() {
    let joined = render_query(Platform::Windows).join(" ");
    for needle in ["reg", "query", RUN_KEY, "/v", TASK_NAME] {
        assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
    }
}

#[test]
fn uninstall_targets_the_installed_task() {
    let install_joined = render_install(Platform::Windows, &spec()).join(" ");
    let uninstall_joined = render_uninstall(Platform::Windows).join(" ");
    assert!(install_joined.contains(TASK_NAME));
    assert!(install_joined.contains(RUN_KEY));
    assert!(uninstall_joined.contains("delete"));
    assert!(uninstall_joined.contains(TASK_NAME));
    assert!(uninstall_joined.contains(RUN_KEY));

    let disable = render_uninstall(Platform::Systemd).join(" ");
    assert!(disable.contains("disable"), "{disable:?}");
    assert!(disable.contains(UNIT_NAME), "{disable:?}");
}

#[test]
fn systemd_unit_has_execstart_and_wantedby() {
    let unit = systemd_unit(&spec());
    for needle in [
        "[Unit]",
        "[Service]",
        "[Install]",
        "Description=Ralphy daemon",
        "ExecStart=",
        "daemon",
        "WantedBy=default.target",
    ] {
        assert!(unit.contains(needle), "missing {needle:?} in {unit:?}");
    }
}

/// The unit pins the distro, because the daemon it starts cannot find it.
///
/// `systemd --user` does not inherit WSL's login-session `WSL_DISTRO_NAME`,
/// and nothing inside the distro names itself, so an un-pinned unit yields
/// a peer descriptor with no `NudgeSpec` — a sleeping peer that can never
/// be woken (ADR-0052 §4). Measured live on Ubuntu-22.04 in the #353
/// capstone: absent from both `systemctl --user show-environment` and the
/// daemon's own `/proc/<pid>/environ`.
#[test]
fn systemd_unit_pins_the_wsl_distro() {
    let unit = systemd_unit(&wsl_spec("Ubuntu-22.04"));
    assert!(
        unit.contains("Environment=\"WSL_DISTRO_NAME=Ubuntu-22.04\""),
        "{unit:?}"
    );
    // In the [Service] section, where an Environment= is honoured.
    let service = unit
        .split("[Service]")
        .nth(1)
        .expect("the unit always has a [Service] section");
    assert!(service.contains("WSL_DISTRO_NAME"), "{unit:?}");
}

/// Off WSL there is no variable to pin, and pinning an empty one would tell
/// the daemon it is somewhere it is not.
#[test]
fn systemd_unit_omits_the_distro_off_wsl() {
    assert!(!systemd_unit(&spec()).contains("Environment="));
    assert!(!systemd_unit(&wsl_spec("")).contains("Environment="));
}

/// The unit pins the installer's PATH, because the daemon's children cannot
/// find the operator's tools without it.
///
/// `systemd --user` does not source the login profile, so a unit without the
/// pin resolves `gh` against the distro's stock PATH. Measured live on
/// Ubuntu-22.04: `/usr/bin/gh` 2.4 (2022) answered the board fold with
/// `Unknown JSON field: "stateReason"` while `~/.local/bin/gh` 2.89 sat
/// unreachable, and every peer board query failed. The pin is a separate
/// `Environment=` line so a missing distro never drops it (and vice versa);
/// an unsafe value is dropped rather than written malformed, like the distro.
#[test]
fn systemd_unit_pins_the_installers_path() {
    let path = "/home/me/.local/bin:/home/me/.cargo/bin:/usr/local/bin:/usr/bin:/bin";
    let unit = systemd_unit(&path_spec(path));
    let service = unit
        .split("[Service]")
        .nth(1)
        .expect("the unit always has a [Service] section");
    assert!(
        service.contains(&format!("Environment=\"PATH={path}\"\n")),
        "{unit:?}"
    );
    assert!(
        !unit.contains("WSL_DISTRO_NAME"),
        "no distro was given: {unit:?}"
    );

    let both = systemd_unit(&AutostartSpec {
        wsl_distro: Some("Ubuntu-22.04".to_string()),
        ..path_spec(path)
    });
    assert!(
        both.contains("Environment=\"WSL_DISTRO_NAME=Ubuntu-22.04\"\n"),
        "{both:?}"
    );
    assert!(
        both.contains(&format!("Environment=\"PATH={path}\"\n")),
        "{both:?}"
    );

    for hostile in ["", "has\"quote", "has\\backslash", "has\nnewline"] {
        assert!(
            !systemd_unit(&path_spec(hostile)).contains("PATH="),
            "{hostile:?} should not reach the unit"
        );
    }
}

/// A distro name may contain spaces — `Ubuntu 22.04` is a real default — so
/// the value is quoted. A name that would break the quoting is dropped
/// rather than written malformed: a unit that fails to parse takes the
/// whole daemon down, while a missing pin only costs the nudge.
#[test]
fn systemd_unit_quotes_a_spaced_distro_and_drops_an_unsafe_one() {
    let spaced = systemd_unit(&wsl_spec("Ubuntu 22.04"));
    assert!(
        spaced.contains("Environment=\"WSL_DISTRO_NAME=Ubuntu 22.04\""),
        "{spaced:?}"
    );
    for hostile in ["has\"quote", "has\\backslash", "has\nnewline"] {
        assert!(
            !systemd_unit(&wsl_spec(hostile)).contains("Environment="),
            "{hostile:?} should not reach the unit"
        );
    }
}

#[test]
fn render_enable_disable_systemd() {
    let enable = render_install(Platform::Systemd, &spec()).join(" ");
    let disable = render_uninstall(Platform::Systemd).join(" ");
    for joined in [&enable, &disable] {
        assert!(joined.contains("systemctl"), "{joined:?}");
        assert!(joined.contains("--user"), "{joined:?}");
        assert!(joined.contains(UNIT_NAME), "{joined:?}");
    }
    assert!(enable.contains("enable"), "{enable:?}");
    assert!(disable.contains("disable"), "{disable:?}");
}

#[test]
fn systemd_is_enabled_parses_literal() {
    assert!(systemd_is_enabled("enabled\n"));
    assert!(!systemd_is_enabled("disabled\n"));
    assert!(!systemd_is_enabled(""));
}
