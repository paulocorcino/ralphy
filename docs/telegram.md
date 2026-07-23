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
