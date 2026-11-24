use super::*;

fn spec() -> AutostartSpec {
    AutostartSpec {
        program: PathBuf::from("/usr/local/bin/ralphy"),
        log_path: PathBuf::from("/home/me/.ralphy/daemon.log"),
        wsl_distro: None,
        path: None,
        uid: Some(501),
        plist_path: PathBuf::from("/Users/me/Library/LaunchAgents/dev.ralphy.daemon.plist"),
    }
}

fn path_spec(path: &str) -> AutostartSpec {
    AutostartSpec {
        path: Some(path.to_owned()),
        ..spec()
    }
}

fn wsl_spec(distro: &str) -> AutostartSpec {
    AutostartSpec {
        wsl_distro: Some(distro.to_owned()),
        ..spec()
    }
}

/// The shortcut opens the launcher beside `ralphy.exe`, with the log file and
/// `daemon`; no shell, no `reg` and no script is anywhere in it.
#[test]
fn the_windows_shortcut_opens_the_launcher_with_the_log_and_daemon() {
    // Built with `join`, so the paths are this host's: the rule is the same.
    let home = Path::new("home").join("me");
    let bin = home.join(".local").join("bin");
    let log = home.join(".ralphy").join("daemon.log");
    let startup = home.join("Startup");
    let shortcut = windows_shortcut(&bin.join("ralphy.exe"), &log, &startup);
    assert_eq!(shortcut.link, startup.join(SHORTCUT_NAME));
    assert_eq!(shortcut.target, bin.join(LAUNCHER_NAME));
    assert_eq!(
        shortcut.arguments,
        format!("--log \"{}\" daemon", log.display())
    );
    assert_eq!(shortcut.working_dir, bin);
    let all = format!("{shortcut:?}").to_lowercase();
    for absent in ["pwsh", "powershell", "reg ", "conhost", "-command"] {
        assert!(!all.contains(absent), "{absent:?} in {all}");
    }
}

/// Task Manager stores 2 for an entry that is on and 3 for one that is off;
/// an entry it never stored is on.
#[test]
fn a_startup_entry_is_off_only_when_its_first_byte_is_odd() {
    assert!(!turned_off(None));
    assert!(!turned_off(Some(&[])));
    assert!(!turned_off(Some(&[2, 0, 0, 0])));
    assert!(turned_off(Some(&[3, 0, 0, 0, 0x9a, 0x13])));
}

#[test]
fn render_install_launchd_bootstraps_the_gui_domain() {
    let argv = render_install(Platform::Launchd, &spec());
    assert_eq!(
        argv,
        vec![
            "launchctl",
            "bootstrap",
            "gui/501",
            "/Users/me/Library/LaunchAgents/dev.ralphy.daemon.plist",
        ],
        "{argv:?}"
    );
}

#[test]
fn render_uninstall_and_query_launchd_name_the_service_target() {
    let bootout = render_uninstall(Platform::Launchd, &spec());
    assert_eq!(
        bootout,
        vec!["launchctl", "bootout", "gui/501/dev.ralphy.daemon"],
        "{bootout:?}"
    );
    let print = render_query(Platform::Launchd, &spec());
    assert_eq!(
        print,
        vec!["launchctl", "print", "gui/501/dev.ralphy.daemon"],
        "{print:?}"
    );
}

/// The agent runs `<exe> daemon` at login and is relaunched only after an
/// unsuccessful exit — `KeepAlive.SuccessfulExit = false` is the systemd
/// `Restart=on-failure`. A clean exit (what `ralphy daemon restart` asks for)
/// must NOT be resurrected, or the supervisor fights the restart for the port.
#[test]
fn launchd_plist_runs_at_load_and_keeps_alive_on_failure_only() {
    let plist = launchd_plist(&spec());
    for needle in [
        "<key>Label</key>\n  <string>dev.ralphy.daemon</string>",
        "<key>ProgramArguments</key>\n  <array>\n    \
         <string>/usr/local/bin/ralphy</string>\n    <string>daemon</string>\n  </array>",
        "<key>RunAtLoad</key>\n  <true/>",
        "<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>",
        "<key>StandardOutPath</key>\n  <string>/home/me/.ralphy/daemon.log</string>",
        "<key>StandardErrorPath</key>\n  <string>/home/me/.ralphy/daemon.log</string>",
        "<plist version=\"1.0\">",
    ] {
        assert!(plist.contains(needle), "missing {needle:?} in:\n{plist}");
    }
    assert!(plist.starts_with("<?xml version=\"1.0\""), "{plist}");
    assert!(plist.trim_end().ends_with("</plist>"), "{plist}");
}

/// The agent pins the installer's PATH, for the reason the systemd unit does:
/// a launch agent inherits the login session's minimal PATH, not the shell's,
/// so the daemon's children (`gh`, `git`, the agent CLIs under `~/.local/bin`
/// or Homebrew) would resolve wrong. No PATH given, no pin written.
#[test]
fn launchd_plist_pins_the_installers_path() {
    let path = "/Users/me/.local/bin:/opt/homebrew/bin:/usr/bin:/bin";
    let plist = launchd_plist(&path_spec(path));
    assert!(
        plist.contains(&format!(
            "<key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    \
             <string>{path}</string>\n  </dict>"
        )),
        "{plist}"
    );
    let bare = launchd_plist(&spec());
    assert!(!bare.contains("EnvironmentVariables"), "{bare}");
    assert!(
        !launchd_plist(&path_spec("")).contains("EnvironmentVariables"),
        "an empty PATH is no PATH"
    );
}

/// A plist is XML, so a value with `&` or `<` is escaped — lossless — rather
/// than dropped the way the systemd unit drops an unquotable value.
#[test]
fn launchd_plist_escapes_xml_in_paths_and_env() {
    let plist = launchd_plist(&AutostartSpec {
        program: PathBuf::from("/Users/me/tools & bins/<ralphy>"),
        path: Some("/a&b:/c<d>".to_owned()),
        ..spec()
    });
    assert!(
        plist.contains("<string>/Users/me/tools &amp; bins/&lt;ralphy&gt;</string>"),
        "{plist}"
    );
    assert!(
        plist.contains("<string>/a&amp;b:/c&lt;d&gt;</string>"),
        "{plist}"
    );
    assert!(
        !plist.contains("tools & bins"),
        "raw ampersand leaked: {plist}"
    );
}

/// A spec the executor never resolved renders `gui/` with no uid, so
/// `launchctl` fails loudly instead of targeting the wrong domain.
#[test]
fn render_launchctl_without_a_uid_does_not_invent_one() {
    let argv = render_install(
        Platform::Launchd,
        &AutostartSpec {
            uid: None,
            ..spec()
        },
    );
    assert_eq!(argv[2], "gui/", "{argv:?}");
}

#[test]
fn systemd_unit_has_execstart_and_wantedby() {
    let unit = systemd_unit(&spec());
    for needle in [
        "[Unit]",
        "[Service]",
        "[Install]",
        "Description=Ralphy daemon",
        "ExecStart=/usr/local/bin/ralphy daemon\n",
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
        wsl_distro: Some("Ubuntu-22.04".to_owned()),
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
    let disable = render_uninstall(Platform::Systemd, &spec()).join(" ");
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

#[test]
fn a_loaded_agent_with_the_same_plist_is_not_loaded_again() {
    let plist = "<plist/>";
    assert!(!launchd_reload_needed(Some(plist), plist, true));
    assert!(
        launchd_reload_needed(Some("<old/>"), plist, true),
        "a changed plist reloads"
    );
    assert!(
        launchd_reload_needed(Some(plist), plist, false),
        "an agent not loaded is loaded"
    );
    assert!(launchd_reload_needed(None, plist, false));
}
