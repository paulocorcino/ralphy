// The Hosts dialog's help: how to turn on the SSH server, and where to read an address.
//
// PURE data, as `wb-hosts.ts`, which re-exports it.

export type HelpTab = {
  id: string;
  name: string;
  steps: { text: string | null; command: string | null }[];
};

// How to turn on the SSH server, per system, when the connection failed
// before the dialog could learn the host's system.
export function helpTabs(): HelpTab[] {
  return [
    {
      id: "windows",
      name: "Windows",
      steps: [
        {
          text: "In PowerShell as administrator, install the SSH server and start it.",
          command: "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
        },
        { text: null, command: "Start-Service sshd" },
        { text: null, command: "Set-Service -Name sshd -StartupType Automatic" },
        {
          text: "Check that the firewall lets SSH in.",
          command: "Get-NetFirewallRule -Name OpenSSH-Server-In-TCP",
        },
        {
          text: "When the rule is missing, create it.",
          command:
            "New-NetFirewallRule -Name OpenSSH-Server-In-TCP " +
            "-DisplayName 'OpenSSH Server (sshd)' -Enabled True " +
            "-Direction Inbound -Protocol TCP -Action Allow -LocalPort 22",
        },
        {
          text: "Sign in with the account password, not the “Windows Hello” PIN.",
          command: null,
        },
        {
          text: "With a Microsoft account, the user name is the local one. This command shows it.",
          command: "whoami",
        },
      ],
    },
    {
      id: "macos",
      name: "macOS",
      steps: [
        {
          text:
            "On macOS 13 and later, turn on “Remote Login” in " +
            "“System Settings › General › Sharing”.",
          command: null,
        },
        {
          text: "On macOS 12, turn on “Remote Login” in “System Preferences › Sharing”.",
          command: null,
        },
      ],
    },
    {
      id: "linux",
      name: "Linux",
      steps: [
        {
          text: "Install the SSH server and start it. On Debian and Ubuntu:",
          command: "sudo apt install openssh-server && sudo systemctl enable --now ssh",
        },
        {
          text: "On Fedora:",
          command: "sudo dnf install openssh-server && sudo systemctl enable --now sshd",
        },
      ],
    },
  ];
}

export const WRONG_ADDRESS = {
  text: "A refused connection can also mean a wrong address. Read the address on the host itself.",
  commands: [
    { name: "macOS", command: "ipconfig getifaddr en0" },
    { name: "Linux", command: "hostname -I" },
    { name: "Windows", command: "ipconfig" },
  ],
};
