use serde_json::json;

use super::*;

const FP: &str = "SHA256:mIlxA9k46MmM6qdJOdMnAQpzGxF4WIVVL+fj+wZbw0g";

fn key_file() -> &'static str {
    if cfg!(windows) {
        r"C:\keys\id_ed25519"
    } else {
        "/home/me/.ssh/id_ed25519"
    }
}

fn argv(verb: Verb, payload: serde_json::Value) -> Vec<String> {
    host_argv(verb, &payload).unwrap_or_else(|e| panic!("{verb:?} {payload}: {e}"))
}

#[test]
fn host_argv_composes_each_verb() {
    assert_eq!(argv(Verb::HostAliases, json!({})), ["host", "aliases"]);
    assert_eq!(
        argv(Verb::HostKey, json!({"destination": "svrapp"})),
        ["host", "key", "svrapp"]
    );
    assert_eq!(
        argv(
            Verb::HostTrust,
            json!({"destination": "me@10.0.0.5", "fingerprint": FP})
        ),
        ["host", "trust", "me@10.0.0.5", "--fingerprint", FP]
    );
    assert_eq!(
        argv(Verb::HostCheck, json!({"destination": "svrapp"})),
        ["host", "check", "svrapp", "--json"]
    );
    assert_eq!(
        argv(
            Verb::HostAdd,
            json!({"destination": "ssh://me@10.0.0.5:2222", "identity": key_file(), "name": "vps"})
        ),
        [
            "host",
            "add",
            "ssh://me@10.0.0.5:2222",
            "--json",
            "--identity",
            key_file(),
            "--name",
            "vps"
        ]
    );
    assert_eq!(
        argv(
            Verb::HostCheck,
            json!({"destination": "svrapp", "identity": "", "name": null})
        ),
        ["host", "check", "svrapp", "--json"],
        "an empty or null optional is not given"
    );
}

#[test]
fn host_argv_remove_rotates_only_when_asked() {
    let id = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    assert_eq!(
        argv(Verb::HostRemove, json!({"host": id, "rotate_token": true})),
        ["host", "remove", id, "--json", "--rotate-token"]
    );
    for payload in [
        json!({"host": id, "rotate_token": false}),
        json!({"host": id}),
    ] {
        assert!(!argv(Verb::HostRemove, payload).contains(&"--rotate-token".to_string()));
    }
    assert_eq!(
        host_argv(
            Verb::HostRemove,
            &json!({"host": id, "rotate_token": "yes"})
        ),
        Err(ArgvError::BadParam("rotate_token"))
    );
}

#[test]
fn host_argv_refuses_an_option_or_a_broken_value() {
    for dest in ["-oProxyCommand=x", "a b", "", "x\n", "tab\there"] {
        assert_eq!(
            host_argv(Verb::HostAdd, &json!({"destination": dest})),
            Err(ArgvError::BadParam("destination")),
            "{dest:?}"
        );
    }
    assert_eq!(
        host_argv(Verb::HostCheck, &json!({})),
        Err(ArgvError::BadParam("destination"))
    );
    assert_eq!(
        host_argv(Verb::HostRemove, &json!({"host": "-x"})),
        Err(ArgvError::BadParam("host"))
    );
    assert_eq!(
        host_argv(
            Verb::HostAdd,
            &json!({"destination": "svrapp", "identity": "keys/id"})
        ),
        Err(ArgvError::BadParam("identity"))
    );
    assert_eq!(
        host_argv(
            Verb::HostAdd,
            &json!({"destination": "svrapp", "name": "-oX"})
        ),
        Err(ArgvError::BadParam("name"))
    );
    for fp in ["MD5:aa", "SHA256:short", "", &format!("{FP}=")] {
        assert_eq!(
            host_argv(
                Verb::HostTrust,
                &json!({"destination": "svrapp", "fingerprint": fp})
            ),
            Err(ArgvError::BadParam("fingerprint")),
            "{fp:?}"
        );
    }
}

#[test]
fn host_argv_refuses_a_verb_outside_the_family() {
    assert_eq!(
        host_argv(Verb::Run, &json!({})),
        Err(ArgvError::BadParam("verb"))
    );
}
