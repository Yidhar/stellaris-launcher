# Problems log

Everything that went wrong, or is wrong with the Paradox Launcher, found while taking it apart. Two kinds:

- **L** — problems of the launcher itself (what people complain about; evidence from this machine's files and logs, launcher 2026.12-rc open beta
  unless said otherwise).
- **D** — problems met while decoding it (so the next person does not repeat them).

Status: `open` (still true / not dealt with), `verified` (checked in more than one place), `guess` (inferred, to be checked).

## Launcher (L)

### L1. Three programs to start one game — `verified`
Steam runs `dowser.exe` (9 MB, in the game folder) → it reads `%LOCALAPPDATA%\Paradox Interactive\launcherpath`, finds the newest launcher version
folder → `bootstrapper-v2.exe` (3 MB) chooses among the installed versions and branches → `Paradox Launcher.exe` (232 MB, a whole Electron
application) → the user must press Play → only then `stellaris.exe` is spawned. Nothing can be scripted: the click is mandatory (the main repo's
AGENTS.md has to say "never start the game through dowser or the launcher"). Evidence: `launcher-dowser.log`, `launcher-bootstrapper.log`
("Executing command `Paradox Launcher.exe --pdxlLauncherInvokedTimestamp … --pdxlGameDir … --gameDir …`").

### L2. Old launcher versions are never removed: 1.5 GB — `verified`
`%LOCALAPPDATA%\Programs\Paradox Interactive\launcher` holds 15 version folders (`launcher-v2.2023.2` … `launcher-v2--openbeta--2026.12-rc`,
~430 MB each for the recent ones), plus `.cpatch` state folders. The bootstrapper even re-checks all of them on every start ("Checking
launcher-v2.2024.6 … Could not find valid installation"). Only the newest is used.

### L3. Two installers shipped inside the game folder (273 MB) — `verified`
`launcher-installer-windows_2024.14.exe` (147 MB) and `launcher-installer-windows-sandbox_2024.6-rc.msi` (126 MB) sit next to `stellaris.exe`
(the MSI is from 2025, never used on a normal install). `dowser.exe` falls back to the installer when `launcherpath` is missing.

### L4. The Windows package carries other platforms' SDKs and debug libraries — `verified`
`resources/app.asar.unpacked/node_modules/launcher-v2-eos-bindings/epic-online-services-sdk` contains the Mac and Linux binaries
(`libEOSSDK-Mac-Shipping.dylib` 47 MB, Linux `.so` ×2 ≈ 49 MB), the Win32 DLL, debug `DirectXTK.lib` / `SDL2-staticd.lib` files and a Samples
folder (≈ 150 MB of build-time files in a runtime install). `greenworks` ships Steam's `steamcmd` for osx/linux and a `.iobj` linker file.

### L5. No active playset → errors on every Play — `verified`
The database on this machine has three playsets and **none has `isActive = 1`**; two are both named "Initial playset". Starting the game then logs
`[GameHandler] Failed to get ongoing ops filter function … Cannot read properties of null (reading 'mods')` and `Failed to send PLAYSET_LOADED
telemetry event: Cannot read properties of null (reading 'id')`. The game still starts, but the launcher has no defined behaviour for "no playset".

### L6. Mod sizes are stored in a 32-bit signed column — `verified`
`ModService` logs `size: 4017489272` for a mod; the database row of another large mod holds `-1791174991`. Sizes over 2 GiB (or 4 GiB) overflow
and are shown wrong.

### L7. Every game start also starts a launcher self-update — `verified`
Right after spawning `stellaris.exe` the launcher checks the CDN, decides "Update 2026.11-rc.2 is not installed, installing" and begins patching
itself (`cpatch.exe` + `xdelta3.exe`, a socket server on `127.0.0.1:11000`) while the game is loading. That is where the 1.5 GB of L2 grows from.

### L8. Noise and crashes in the log of an ordinary start — `verified`
`Failed to initialize GeforceNow runtime: GFN_RESULT_INIT_SUCCESS_CLIENT_ONLY` (a success code reported as a failure) on every start, stack
traces from `index.mjs` / `index.jsc`, megabytes of logs per day (`launcher-2025-10-24.log` is 2.9 MB).

### L9. `dowser.exe` loses its installation after a profile change — `verified`
First start in the log (2024-04-21): "The `launcherpath` file does not exists … Trying to fix existing installation in "/passive" mode … Fixing
existing installation failed: exit status 1605 … Running regular installation" — a full reinstall of the launcher from the installer in the game
folder. (The Windows profile folder of that run was a non-ASCII name, `C:\Users\芸`; a later log shows a different profile name.) `guess`: path
encoding is the reason.

### L10. The launcher marks its own start as "third party" — `guess`
`userSettings.json` has `"launchedByThirdParty": true` once the game was started by something else; what changes because of it is not yet read
from the code.

### L11. The game is started with an account session token on the command line — `verified`
`stellaris.exe --pdx-launcher-session-token <token> --paradox-account-userid <id> -gdpr-compliant [--continuelastsave]`: the token is visible to
every process on the machine (command lines are not private on Windows). The log redacts it; the process list does not.

### L12. There is no notion of native plugins — `verified`
Mods are `.mod` descriptors (`ugc_<id>.mod` Steam workshop, `pdx_<id>.mod` Paradox Mods, local `*.mod`) plus the content folder; nothing in the
database, the playsets or the launch code knows about DLLs. Anyone who wants a native plugin has to put a DLL next to the exe by hand (and
remember it when the game is verified or updated), or inject it from outside. This is the feature gap this project is for.

### L13. A hand-edited `dlc_load.json` is silently overwritten — `verified` (code), not yet seen live
The database is the source of truth: when the launcher saves the playset it writes `enabled_mods` from its own rows, and when it reads the file
it drops every entry that matches no row. Anything that is not a mod the launcher knows (a local mod added by a tool, a hand edit) is lost the next
time the launcher saves. Evidence: `EnabledModsStorageV0` (`mapModFilePathsToModId`, `save`).

### L14. The launcher runs the open-beta branch although the setting says it is off — `guess`
`…\launchereta_branch` contains `openbeta` and the bootstrapper logs "Found `openbeta` branch", while `userSettings.json` has
`"openBetaOptIn": false`. The data files carry the branch in their names (`launcher-v2_openbeta.sqlite`, `playsets_backup_openbeta`), so the
state of two branches can silently diverge.

### L15. Unfinished features are behind command-line flags — `verified`
`--pdxlShowModConflictCheck` ("until the feature is fully released"), `--pdxlShowModDirectoriesManagement`: what mod users ask for most (conflict
detection, choosing where mods live) exists but is hidden.

### L16. Telemetry is on unless an environment variable says otherwise — `guess`
Events such as `PLAYSET_LOADED` go to `prod-telemetry.paradox-interactive.com`; switches are `MAIN_APP_DISABLE_TELEMETRY` and `--pdxlEnableTelemetry`.
Not yet checked: what the first-run dialog promises.

## Decoding (D)

### D1. Only the newest launcher version is readable
`2026.12-rc` ships `dist/main/index.mjs` (minified but plain ES modules, 3.4 MB). Older versions (`2025.8-rc` in the log) ship V8 bytecode
(`index.jsc`, made with bytenode), which cannot be read as text. Decode the newest version and note the version in every finding.

### D2. Minified single-file bundles
`index.mjs` is one 3.4 MB line; reading needs a beautifier (`jsbeautifier`, 9 s to turn it into 140 000 lines). Names are mangled
(`yM`, `q9`, `Zt`), but strings, service names (`LaunchExecutable`, `ProcessService`, `ModService`) and log messages survive and are the way in.

### D3. `app.asar.unpacked` has to be next to the archive when extracting
Files flagged `unpacked` in the asar header are not in the archive; `tools/asar_extract.py` copies them from `app.asar.unpacked`. Extract only
`dist/` and `package.json`: `node_modules` is about 1 GB of native SDKs.

### D4. Tools used on this machine need care
Git Bash `cygpath`/heredocs mangle backslashes in Windows paths — write Python helpers to files instead of inline; `pip install jsbeautifier` is
the only dependency.

### D5. `cargo` cannot reach crates.io with HTTP/2 multiplexing
`index.crates.io` answers `curl` (6 s) but cargo times out ("Failed to connect … after 21042 ms"); `[http] multiplexing = false` and a longer timeout in
`.cargo/config.toml` fix it. The same machine also fails CMake's FetchContent downloads of GitHub release assets ("Error in the HTTP2 framing
layer") and `gh run download` from the artifact blob store: anything that speaks HTTP/2 to those hosts is unreliable here; `curl --http1.1` works.

### D6. A started game inherited our pipes (found by the first live run)
`stl launch` did not return when its output was captured by a script: the game, started detached, had inherited the parent's inheritable
standard handles (Windows gives a child every inheritable handle when `bInheritHandles` is true), so the pipe's write end stayed open for as long as the
game ran. The official launcher cannot have this problem (it passes its own stdio). Fixed in `process::spawn` by clearing the inherit flag of
our standard handles around the spawn; `crates/stl-core/tests/spawn_pipes.rs` checks it, and by running the old behaviour as a negative control.

### D7. The idle gate can starve a test
The live tests close and restart the game, which is only polite when the user is away (`desktop_guard`, 45 s idle). A test that needs the machine
for minutes will simply wait: keep every step that does not need the game (unit tests, dry runs) independent of it.
