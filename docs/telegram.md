# Telegram run monitor (optional)

Since a run is unattended, Ralphy can post a live **status card** to a Telegram chat and
keep it updated through the whole run — planning, execution, usage-limit waits, and the
final summary — with a quick ping at the moments that matter. It's read-only; the bot
just reports. Once set up it's on by default for real runs; mute one run with
`--no-telegram`.

```powershell
ralphy telegram setup    # store the bot token, then send /start to capture your chat
ralphy telegram test     # send a ping to confirm it works
ralphy telegram status   # show the configured chat and a masked token
ralphy telegram disable  # remove the stored config
```

The bot token and the chat are saved owner-only in the daemon store, in
`~/.ralphy/telegram.toml` (or in the folder that `RALPHY_DAEMON_DIR` names).
The daemon and every run on this machine read that file. An older version kept
them in `~/.config/ralphy/config.toml` (`%APPDATA%\ralphy\config\config.toml`
on Windows); Ralphy copies that file into `~/.ralphy` once, and
`ralphy telegram disable` removes both. `ralphy doctor` says whether this
machine sends, and why not.
