use super::*;

fn argv(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn flags(model: Option<&'static str>, effort: Option<&'static str>) -> RunFlags<'static> {
    RunFlags {
        root: Path::new("/data/antigravity-root"),
        model,
        effort,
    }
}

#[test]
fn the_run_argv_states_every_flag() {
    let cmd = build_agy_command(
        Path::new("/repo"),
        flags(Some("claude-sonnet-5-5"), Some("high")),
    );
    let root_flag = format!(
        "--gemini_dir={}",
        Path::new("/data/antigravity-root").display()
    );
    assert_eq!(
        argv(&cmd),
        [
            root_flag.as_str(),
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--dangerously-skip-permissions",
            "--model",
            "claude-sonnet-5-5",
            "--effort",
            "high",
        ]
    );
    assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));

    let bare = argv(&build_agy_command(
        Path::new("/repo"),
        flags(None, Some("high")),
    ));
    for refused in [
        "-p",
        "--print",
        "--mode",
        "--sandbox",
        "--print-timeout",
        "--remote-control",
        "--model",
        "--effort",
    ] {
        assert!(
            !bare
                .iter()
                .any(|a| a == refused || a.starts_with(&format!("{refused}="))),
            "{refused} reached argv: {bare:?}"
        );
    }
}

#[test]
fn the_child_environment_drops_google_and_gemini_keys() {
    let parent = [
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "AGY_ADC_AUTH",
        "GOOGLE_CLOUD_QUOTA_PROJECT",
        "google_genai_use_vertexai",
        "PATH",
        "SystemRoot",
    ]
    .map(OsString::from);
    let cmd = build_with(
        NAME.into(),
        parent.into_iter(),
        Path::new("/repo"),
        flags(None, None),
    );
    let removed: Vec<String> = cmd
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in [
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "AGY_ADC_AUTH",
        "GOOGLE_CLOUD_QUOTA_PROJECT",
        "google_genai_use_vertexai",
    ] {
        assert!(
            removed.iter().any(|r| r.eq_ignore_ascii_case(name)),
            "{name} reached the child: {removed:?}"
        );
    }
    for kept in ["PATH", "SystemRoot"] {
        assert!(
            !removed.iter().any(|r| r.eq_ignore_ascii_case(kept)),
            "{kept} was removed"
        );
    }
    assert!(
        cmd.get_envs().all(|(_, value)| value.is_none()),
        "the child gets no variable set by Ralphy"
    );
}

#[test]
fn the_user_message_is_one_ndjson_line() {
    let prompt = "line one\nline \"two\" ação ✓";
    let line = user_message(prompt);
    assert!(line.ends_with('\n') && !line.ends_with("\n\n"), "{line:?}");
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "event": "user",
            "message": { "role": "user", "content": prompt },
        })
    );
}
