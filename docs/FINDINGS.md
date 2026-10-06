# How the Paradox Launcher works

What was read, from where. Launcher **2026.12-rc** (open beta branch), Stellaris 4.5.1 on Steam, Windows 11. The code is the readable
`dist/main/index.mjs` of that version (see PROBLEMS.md D1); log and file evidence is from this machine. Names such as `L2` or `yM` are the
minifier's, the strings and service names are the launcher's own.

## 1. From Steam to `stellaris.exe`

```
Steam  →  dowser.exe  (Go, game folder)  →  bootstrapper-v2.exe  (Go, launcher folder)  →  Paradox Launcher.exe  (Electron)  →  stellaris.exe
```

| Step | What it does | Evidence |
|---|---|---|
| `dowser.exe` (`launcher-v2-dowser`, Go, 9 MB, "Dowser version 2026.2") | reads `%LOCALAPPDATA%\Paradox Interactive\launcherpath` (a one-line file: the launcher folder), finds the newest installed launcher version; if there is none it runs `launcher-installer-windows_<ver>.exe` from the game folder (`/passive`, regex over the installer's file name) | `launcher-dowser.log`; Go symbols `cfg.GetLauncherPathFile`, `GetInstallerExecutable`, `main.runInstaller` |
| `bootstrapper-v2.exe` (`launcher-v2-bootstrapper`, Go, 3 MB) | looks at every `launcher-v2…` folder under `…\Programs\Paradox Interactive\launcher\` (`.cpatch\launcher-v2_1\version` marks a valid install), picks the newest compatible version, and runs it as `Paradox Launcher.exe --pdxlLauncherInvokedTimestamp <ms> --pdxlGameDir <game folder> --gameDir <game folder>`; it also owns the update machinery (`cpatch.exe`, `xdelta3.exe`) | `launcher-bootstrapper.log` ("Found 4 compatible launcher version(s)… Latest version is `2026.12-rc` from branch `openbeta`") |
| `Paradox Launcher.exe` | Electron 232 MB: main process `dist/main/index.mjs` (3.4 MB), a worker, a preload, the renderer (`dist/renderer/assets/*.js`, React). Shows the home page, playsets, mods, settings; the **Play** button calls `LaunchExecutable` | `resources/app.asar` |
| `stellaris.exe` | spawned by the launcher (below) | `launcher-2026-09-22.log` "Starting game: ./stellaris.exe" |

## 2. The launch contract

`LaunchExecutable.launchGame` → `buildLaunchArgs` → `ProcessService.spawnExecutable`:

```
stellaris.exe  [--pdx-launcher-session-token <token> --paradox-account-userid <guid>]   # only when logged in to a Paradox account
               [-AUTH_PASSWORD=… -epicuserid=… -epicsandboxid=… -epicdeploymentid=… -EOS_REFRESH_TOKEN=…]   # Epic only
               [--continuelastsave]                                                   # the "continue" button
               <extra args from the command line>                                     # every launcher argument without the "pdxl" prefix
               <exeArgs of launcher-settings.json>                                    # ["-gdpr-compliant"]; the alternative executable adds "-nakama"
```

- `child_process.spawn(path, args, { env: {...process.env, SteamAppId}, detached: true, stdio: ["ignore", "ignore", <file>], cwd: <game folder> })`, then
  `unref()` — the launcher does not own the game process; stderr goes to a file in the launcher's app data folder.
- `launcher-settings.json` (game folder, read-only for the launcher): `exePath`, `exeArgs`, `alternativeExecutables` (the Cross-Store Multiplayer
  button), `gameDataPath` (`%USER_DOCUMENTS%/Paradox Interactive/Stellaris`), `version`/`rawVersion`, `modsCompatibilityVersion`
  ("4.5", compared with each mod's `supported_version`), `distPlatform` ("steam"), `ingameSettingsLayoutPath`.
- The game also runs without any of this: `stellaris.exe -dx11` from its folder works (what the main repo's scripts do); it then has no account
  token (no Paradox online features) and the launcher's playset is not applied to `dlc_load.json` (section 4).
- Launcher command line (yargs): `--pdxlGameDir`, `--pdxlLoglevel`, `--pdxlDebug`, `--pdxlDataSuffix` (separate launcher data folder),
  `--pdxlDisableHardwareAcceleration`, `--pdxlShowModConflictCheck` and `--pdxlShowModDirectoriesManagement` (unfinished features hidden behind
  flags), `--pdxlSteamAppIdOverride`, `--pdxlDefaultGameLibraryPath`, telemetry switches, … "Arguments without the pdxl prefix will be passed to
  the game without modification" — the way Steam launch options (`-dx11`) reach the game.

## 3. Where things live

| What | Where |
|---|---|
| launcher versions | `%LOCALAPPDATA%\Programs\Paradox Interactive\launcher\launcher-v2[--<branch>]--<version>\` (+ `bootstrapper-v2.exe`, `beta_branch`, `.cpatch\`) |
| launcher settings | `%APPDATA%\Paradox Interactive\launcher-v2\userSettings.json` (plain), `session.js` (encrypted: account session) |
| launcher logs, Chromium profile | `%LOCALAPPDATA%\Paradox Interactive\launcher-v2\logs\` and `chromium-data\` |
| game data folder | `Documents\Paradox Interactive\Stellaris\` |
| playsets, mods, DLC state | `launcher-v2_openbeta.sqlite` (the `_openbeta` suffix follows the branch; a `-backup` copy and `playsets_backup_openbeta\` next to it) |
| what the game reads | `dlc_load.json`, `dlc_signature`, `game_data.json`, `settings.txt`, `pdx_settings.txt`, `mod\*.mod`, `continue_game.json` |
| the game's own folder | `launcher-settings.json`, `settings-layout.json`, `checksum_manifest.txt` (directories hashed for multiplayer), `pdx_launcher\`, `pdx_online_assets\` |

## 4. Playsets and mods

**The database is the truth, `dlc_load.json` is an output.** Tables of `launcher-v2_openbeta.sqlite` (knex migrations, 36 so far):

- `mods` — one row per known mod: `id` (guid), `pdxId`, `steamId`, `gameRegistryId` (`mod/ugc_<steamid>.mod` — the descriptor path relative to the
  game data folder, what the game is told), `name`, `version`, `requiredVersion`, `tags`, `dirPath` (content folder; Steam workshop
  `…\workshop\content\281990\<id>`), `archivePath`, `source` (`steam` | `local`), `status` (`ready_to_play`, `unsubscribed`,
  `installation_failed`), `size`, `metadataStatus`, `keepLatest`, …
- `playsets` — `id`, `name`, `isActive`, `loadOrder`, `pdxId` (Paradox Mods sync), `state` (`private`/`public`/…), `lastUsedAt`, …
- `playsets_mods` — `(playsetId, modId, enabled, position)`: the load order of a playset.
- `playsets_dlcs` — `(playsetId, dlcId, enabled)`; `dlc` (installed DLCs), `mods_dependencies`, `metadata_relationships`, `ugc`,
  `transient_ownership`, `key_value_pairs` (`lastModsCompatibilityVersion`, `dlcPlaysetSeedDone`, `lastPlaysetBackup`).

Mod descriptors: `mod\ugc_<id>.mod` (Steam workshop), `mod\pdx_<id>.mod` (Paradox Mods), any other `mod\*.mod` is a *local* mod (the launcher
scans `./mod/!(pdx_|ugc_)*.mod`). A mod folder may contain `.metadata\metadata.json`. `mods_registry.json` caches the registry.

**`dlc_load.json`** (format "LFV0", the one Stellaris uses): `{"enabled_mods": ["mod/xxx.mod", …], "disabled_dlcs": ["dlc/….dlc", …]}`, schema-validated.
Newer games use `content_load.json` ("LFV1"/"LFV1_1": `enabledMods[{path}]`, `enabledUGC`, `disabledDLC[{paradoxAppId}]`). The writer
(`EnabledModsStorageV0.save`) turns the enabled mods of the playset into their `gameRegistryId`s and does `setAndSave("enabled_mods", …)`; the
reader maps ids back to database rows and **drops any entry it cannot match** — so a hand-edited `dlc_load.json` survives only until the
launcher saves again (PROBLEMS.md L13).

**`dlc_signature`**: MD5 over `[machine id padded to 255 bytes] + [the DLC descriptors sorted by name] + a fixed secret string`, written by the
launcher on the Steam and Paradox platforms (`NoDrm` on Epic/GOG/Humble/Microsoft/Origin). It binds the DLC state to the machine; it does not cover
mods, so changing the playset does not invalidate it. A launcher of ours should leave DLC state and the signature to the official launcher (or
only read it); forging entitlements is out of scope.

**What the game itself does with these** (strings of `stellaris.exe`, and file times): it knows the arguments `-pdx-launcher-session-token`,
`-gdpr-compliant` and `-continuelastsave` (and `-nakama`, 14 mentions), reads `dlc_load.json`, hashes the directories of
`checksum_manifest.txt` (`gfx/FX/.checksum_manifest.txt`), and **writes `dlc_signature` itself** ("Failed to create dlc signature file"; the file
on this machine was rewritten at the time of a direct start by our scripts, with no launcher involved). So a direct start needs neither the launcher
nor its signature; the launcher's job is the playset (writing `dlc_load.json`), the account token and the updates.

## 5. Updates

The launcher updates itself in place of replacing: `cpatch.exe` talks to the launcher over a socket (`127.0.0.1:11000`), downloads xdelta patches
from a CDN repository (`launcher-v2_1-openbeta-windows-64`) into a *new* version folder and keeps the old one. It does this on every start,
right after the game was spawned (it pauses patch operations while spawning). Mods from Paradox Mods are patched the same way
(`ModPatcher`, `patchService`).

## 6. Network

`api.paradox-interactive.com`, `accounts.paradoxplaza.com`, `mods.paradoxplaza.com`, `prod-telemetry.paradox-interactive.com` (telemetry; switches
`MAIN_APP_DISABLE_TELEMETRY`, `--pdxlEnableTelemetry`), Steam store/community, `raw.githubusercontent.com`/`github.com` (links), `google.com`.

## 7. What a DLL-mod manager can build on

- **Nothing in the launcher knows DLLs** (PROBLEMS.md L12): no table, no flag, no launch hook. The launch contract is a plain `spawn` with the
  game folder as the working directory, so a native plugin is loaded either by an injector running next to the spawn, or by a proxy DLL the
  game loads by itself (this repository's sibling `stellaris-live2d` ships `d3dx9_43.dll`, which the exe imports and no system DLL preloads).
- **Safe integration points**: the game folder (proxy DLLs, a plugin list file), the game data folder (`mod\*.mod` of our own; `dlc_load.json` only
  while the official launcher is not running), the launcher's database *read-only*.
- **Not safe**: writing the database or `userSettings.json` while the launcher runs; writing `dlc_signature`; relying on the minified code's
  internals (names change every release, older versions are bytecode).

## 8. Open questions

1. Replacement launcher (start the game itself, own UI) or companion (leave the official launcher, add a DLL manager that cooperates with it)?
2. What does the game do with `--pdx-launcher-session-token` when it is missing — which online features, if any, does a direct start lose?
   (The token is a game argument; the code behind it is in the exe, not the launcher. Direct starts have worked for a month; a list of what is lost
   needs a start with and without it.)
3. Does the official launcher rewrite `dlc_load.json` on Play (to be confirmed by starting it once with a marker mod in the file)?
4. Multiplayer: does the game compare plugin DLLs? (It hashes `checksum_manifest.txt` directories only — game files, not DLLs; to be confirmed.)
