# stellaris-Launcher

A launcher for **Stellaris** (Windows, 4.5) that replaces the Paradox Launcher for playing: it keeps your **playsets**, manages **mods** and **DLL
plugins** (native plugins loaded into the game) in one place, and starts the game itself — without three programs, a 232 MB Electron app and a
mandatory click. About 4 MB, no installer, nothing written outside your own folders.

Status: **v0.1, working.** Verified against a real installation: playsets and mods read, official playsets imported, the game stopped, started with
the last save continued, and a DLL plugin loaded into it by the launcher.

- `stellaris-launcher.exe` — the window: playset picker, mod list (check boxes, load order, add/remove), DLL plugin switches, Play, a log.
- `stl.exe` — the same on the command line (`stl --help`).

```
stl status                              the game, the active playset, the plugins
stl import-official                     copy the playsets of the Paradox Launcher into ours (its database is only read)
stl playsets / stl playset show|new|use|remove|add|rm|enable|disable|move
stl mods                                the mods the game can load
stl plugin install <folder> [--link]    DLL plugins (see docs/PLUGINS.md); stl plugin enable <id>
stl launch [--continue] [--playset X]   write dlc_load.json, start the game, load the plugins
stl stop                                close the game
```

## How it differs from the Paradox Launcher

- Starts `stellaris.exe` directly, as the official launcher does (`-gdpr-compliant`, the game's own `launcher-settings.json`), with the working
  directory and `SteamAppId` it expects. Steam has to be running. There is no Paradox account login, so the game gets no session token (Paradox
  online features only).
- Your playsets are ours (`%APPDATA%\stellaris-launcher\playsets.json`), imported once from the official database, which is never written.
  DLC choice and `dlc_signature` are left alone.
- **DLL plugins are first class**: a manifest next to the DLL, per-playset on/off, a check that the plugin was made for the installed game build,
  loaded once the game's window exists.

## Why

What was found in the official launcher while taking it apart — how it starts the game, where its data lives, 16 problems people run into — is in
[docs/FINDINGS.md](docs/FINDINGS.md) and [docs/PROBLEMS.md](docs/PROBLEMS.md). The design is in [docs/DESIGN.md](docs/DESIGN.md), the plugin
manifest in [docs/PLUGINS.md](docs/PLUGINS.md).

## Build

Rust (stable) and a Windows toolchain. `cargo build --release` gives `target\release\stl.exe` and `stellaris-launcher.exe`; `cargo test --workspace`
runs the tests (a negative-controlled check that a started game does not inherit our pipes, a real DLL load into another process, parsers and
the official-database import against a temporary database). `.cargo/config.toml` turns off HTTP multiplexing, which crates.io needs on some networks.

`tools/asar_extract.py` unpacks the launcher's Electron archive (for reading); `tools/shot_gui.py` screenshots the window for development.
The decoded launcher code and copies of its database stay in `decoded/` and `work/`, which are not in git.

## License

MIT, see [LICENSE](LICENSE). Nothing of Paradox's is included.
