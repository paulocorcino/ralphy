use super::*;

const CONFIG: &str = "# my hosts
Host svrapp
    HostName 10.0.0.5
    User deploy
Host *
    ServerAliveInterval 30
Host web db !bastion
Host=svrapp
host   gpu?
Match host x
    User nobody
";

#[test]
fn parse_config_hosts_keeps_only_concrete_names() {
    assert_eq!(parse_config_hosts(CONFIG), ["svrapp", "web", "db"]);
    assert!(parse_config_hosts("").is_empty());
}

// format of `ssh -G svrapp`, OpenSSH 9
const SSH_G: &str = "host svrapp
user deploy
hostname 10.0.0.5
port 2222
addressfamily any
userknownhostsfile ~/.ssh/known_hosts ~/.ssh/known_hosts2
proxyjump none
";

#[test]
fn parse_ssh_g_reads_the_connection() {
    let home = Path::new("/home/me");
    let r = parse_ssh_g(SSH_G, Some(home)).unwrap();
    assert_eq!(r.hostname, "10.0.0.5");
    assert_eq!(r.user, "deploy");
    assert_eq!(r.port, 2222);
    assert_eq!(
        r.known_hosts,
        [
            home.join(".ssh/known_hosts").display().to_string(),
            home.join(".ssh/known_hosts2").display().to_string(),
        ]
    );
    assert!(!r.proxied);

    let jumped = SSH_G.replace("proxyjump none", "proxyjump bastion");
    assert!(parse_ssh_g(&jumped, Some(home)).unwrap().proxied);
    let command = format!("{SSH_G}proxycommand nc %h %p\n");
    assert!(parse_ssh_g(&command, Some(home)).unwrap().proxied);
}

#[test]
fn parse_ssh_g_needs_a_hostname_and_a_port() {
    assert!(parse_ssh_g("user deploy\n", None).is_err());
}

#[test]
fn expand_home_leaves_an_absolute_path() {
    assert_eq!(
        expand_home("/etc/ssh/known", Some(Path::new("/h"))),
        "/etc/ssh/known"
    );
    assert_eq!(expand_home("~/.ssh/k", None), "~/.ssh/k");
}

#[test]
fn split_known_hosts_keeps_a_path_with_a_space_whole() {
    // `ssh -G` of OpenSSH_for_Windows 9.5p2 for
    // `UserKnownHostsFile "C:/Temp/sp ace/kh" ~/.ssh/known_hosts`
    let value = r"C:/Users/PICHAU/AppData/Local/Temp/sp ace/kh C:\Users\PICHAU/.ssh/known_hosts";
    assert_eq!(
        split_known_hosts(value),
        [
            "C:/Users/PICHAU/AppData/Local/Temp/sp ace/kh",
            r"C:\Users\PICHAU/.ssh/known_hosts"
        ]
    );
    assert_eq!(
        split_known_hosts("/home/me/.ssh/known_hosts ~/.ssh/known_hosts2"),
        ["/home/me/.ssh/known_hosts", "~/.ssh/known_hosts2"]
    );
}
