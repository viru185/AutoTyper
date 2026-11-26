# Security policy

Auto Typer handles passwords, so security reports are very welcome.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through
[GitHub security advisories](https://github.com/viru185/AutoTyper/security/advisories/new).
You will get an answer within a few days.

## Supported versions

Only the latest release gets fixes.

## How Auto Typer handles your text

- The text exists only in memory. It is never written to disk, logged, or sent over the network
  (the app makes no network connections at all).
- It is cleared after a successful run and wiped from memory (zeroized) when a job ends and when
  the app exits. Typing jobs and clipboard reads use zeroizing buffers.
- Only settings are saved (`settings.toml`), never the text.
- Instances talk to each other through a per-user named pipe / local socket, with three fixed
  commands (`show`, `quit`, `reload`) and no data.

## Known limitations

- Anything that records keystrokes on the machine (or the remote machine) sees what Auto Typer
  types, exactly as if you typed it yourself.
- The clipboard is readable by other programs; turn on *Clear the clipboard after typing it* if
  you copy secrets into it.
