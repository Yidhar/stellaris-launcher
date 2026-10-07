# stellaris-Launcher

A launcher for **Stellaris** (Windows, 4.5) that replaces the Paradox Launcher for playing: it keeps your **playsets**, manages **mods** and **DLL
plugins** (native plugins loaded into the game) in one place, and starts the game itself — without three programs, a 232 MB Electron app and a
mandatory click. About 4 MB, no installer, nothing written outside your own folders.

Status: **v0.1, working.** Verified against a real installation: playsets and mods read, official playsets imported, the game stopped, started with
the last save continued, and a DLL plugin loaded into it by the launcher.

- `stellaris-launcher.exe` — the window, in the manner of iOS: the game's artwork behind frosted-glass cards and a tab bar. **Play** (the playset
  and two large buttons, Play and Continue; the news cards of the Paradox Launcher's home page as a small strip you page through),
  **Playsets** (mods in load order, DLC on/off, plugins), **Mods** (the whole
  mod folder, add/remove with one tap, make a new mod, upload your own to the Steam Workshop), **Plugins** (install, link, remove), **Settings** (launch options, nine languages, background, game folder, log).
- `stl.exe` — the same on the command line (`stl --help`).

```
stl status                              the game, the active playset, the plugins
stl import-official                     copy the playsets of the Paradox Launcher into ours (its database is only read)
stl playsets / stl playset show|new|use|remove|add|rm|enable|disable|move
stl mods                                the mods the game can load
stl mod new <name> [--tags a,b]         make a local mod (folder + both descriptors)
stl mod upload <mod> [--yes]            check a local mod, then upload it to the Steam Workshop (through the running Steam client);
                                        .git and source files stay out, and a failure says why and what to do
stl dlc [enable|disable <name>…]        the installed DLC, switched on/off in the active playset
stl news [--refresh]                    the news cards of the official launcher's home page
stl plugin install <folder> [--link]    DLL plugins (see docs/PLUGINS.md); stl plugin enable <id>
stl launch [--continue] [--playset X]   write dlc_load.json, start the game, load the plugins
stl stop                                close the game
stl check [--playset X] [--mod Y]       problems of a playset's mods, and what overrides what (docs/CONFLICTS.md)
stl sort [--playset X] [--apply]        a load order where each mod comes after what it needs and what it patches
stl self-update [--check]               update the launcher from its GitHub releases (with the window closed)
```

**Updates.** The window looks for a newer release of the launcher when it starts (and every six hours while it runs), downloads it in the
background, checks it against the `.sha256` published beside the zip and keeps it in `%APPDATA%\stellaris-launcher\update`. A button in the title
bar then shows the release notes and restarts into the new version; or the next start installs it before the window opens. The running exe is
renamed to `.old` (Windows allows that, not overwriting it) and removed on the following start. Switch it off in Settings → Launcher updates.

## Install

From [the releases](https://github.com/Yidhar/stellaris-launcher/releases):

- **`stellaris-launcher-setup-v….exe`** installs it for your user only (no administrator rights) into
  `%LOCALAPPDATA%\Programs\Stellaris Launcher`, with a Start menu entry (a desktop one if you tick it) and an uninstaller in
  *Apps & features*, which asks whether to keep your playsets and settings.
- **`stellaris-launcher-v….zip`** runs from wherever you unpack it. Its first start adds it to *Apps & features* (and to Win+R as
  `stellaris-launcher`); uninstalling it there removes only the files the zip brought (`stellaris-launcher.exe --uninstall`).

Either way it then updates itself from the releases (below). Your playsets and settings are in `%APPDATA%\stellaris-launcher`.

## How it differs from the Paradox Launcher

- Starts `stellaris.exe` directly, as the official launcher does (`-gdpr-compliant`, the game's own `launcher-settings.json`), with the working
  directory and `SteamAppId` it expects. Steam has to be running. There is no Paradox account login (sign-in is left to the Paradox Launcher), so
  the game gets no session token (Paradox online features only).
- The **news cards** ("ads") of the official home page are shown too: the same public, anonymous feed, fetched without sending anything about you
  (switch it off in Settings); animated GIFs play. The background is the game's own artwork, read from the Paradox Launcher's cache or Steam's
  library cache, never copied or shipped.
- Your playsets are ours (`%APPDATA%\stellaris-launcher\playsets.json`), imported once from the official database, which is never written.
  Each playset can also switch DLC off (`disabled_dlcs` of `dlc_load.json`, which is what the official launcher edits); a playset that never set
  it leaves the file's list alone. `dlc_signature` is the game's own and is never touched.
- **It updates itself** from its GitHub releases (above); the Paradox Launcher's own updater is not involved.
- **DLL plugins are first class**: a manifest next to the DLL, per-playset on/off, a check that the plugin was made for the installed game build,
  loaded once the game's window exists.

## Why

What was found in the official launcher while taking it apart — how it starts the game, where its data lives, 16 problems people run into — is in
[docs/FINDINGS.md](docs/FINDINGS.md) and [docs/PROBLEMS.md](docs/PROBLEMS.md). The design is in [docs/DESIGN.md](docs/DESIGN.md), the plugin
manifest in [docs/PLUGINS.md](docs/PLUGINS.md), and how to write a plugin in [docs/PLUGIN_HANDBOOK.md](docs/PLUGIN_HANDBOOK.md).

## Build

Rust (stable) and a Windows toolchain. `cargo build --release` gives `target\release\stl.exe` and `stellaris-launcher.exe`; `cargo test --workspace`
runs the tests (a negative-controlled check that a started game does not inherit our pipes, a real DLL load into another process, parsers and
the official-database import against a temporary database). `.cargo/config.toml` turns off HTTP multiplexing, which crates.io needs on some networks.

`tools/asar_extract.py` unpacks the launcher's Electron archive (for reading); `tools/shot_gui.py` screenshots the window for development.
The decoded launcher code and copies of its database stay in `decoded/` and `work/`, which are not in git.

## License

MIT, see [LICENSE](LICENSE). Nothing of Paradox's is included.
