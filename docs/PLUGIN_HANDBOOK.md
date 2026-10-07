# Stellaris Launcher DLL Plugin Developer Handbook

**English** | [简体中文](PLUGIN_HANDBOOK.zh-CN.md)

> Spec version: **plugin spec v2** (manifest `"schema": 2`), for Stellaris Launcher 0.1 and later and Stellaris 4.5 (Windows x64).
> This handbook is the complete guide for plugin authors; the compact field reference is [PLUGINS.md](PLUGINS.md).
>
> **MUST** marks a requirement of the spec: a plugin that breaks it is refused by the launcher or damages the user's setup. **SHOULD** is a
> strong recommendation. **MAY** is optional.

---

## Contents

1. [What a plugin is](#1-what-a-plugin-is)
2. [Ten-minute start](#2-ten-minute-start)
3. [The plugin folder](#3-the-plugin-folder)
4. [The manifest: stl-plugin.json](#4-the-manifest-stl-pluginjson)
5. [Lifecycle: from install to injection](#5-lifecycle-from-install-to-injection)
6. [Rules for the DLL](#6-rules-for-the-dll)
7. [Game build compatibility](#7-game-build-compatibility)
8. [Settings files](#8-settings-files)
9. [Logs and troubleshooting](#9-logs-and-troubleshooting)
10. [Packaging and releases](#10-packaging-and-releases)
11. [Automatic updates](#11-automatic-updates)
12. [Code of conduct](#12-code-of-conduct)
13. [Release checklist](#13-release-checklist)
14. [Appendix A: a minimal C++ template](#appendix-a-a-minimal-c-template)
15. [Appendix B: JSON Schema of the manifest](#appendix-b-json-schema-of-the-manifest)
16. [Appendix C: moving from schema 1](#appendix-c-moving-from-schema-1)

---

## 1. What a plugin is

A plugin is a **native Windows DLL** that the launcher **injects** into the `stellaris.exe` process once the game has started. It can hook
rendering, read game memory, call engine functions, or bridge the game and an outside program. Existing examples:

| Plugin | What it does |
|---|---|
| `stellaris-mcp` | lets AI agents read and play the game over MCP |
| `stellaris-live2d` | draws Live2D models in the portraits |
| `stellaris-perf` | performance improvements |

How plugins differ from **mods**:

| | Mod | Plugin |
|---|---|---|
| What it is | game scripts, pictures, localisation | a native DLL |
| Loaded by | the game itself (`dlc_load.json`) | Stellaris Launcher, by injection |
| Lives in | `Documents\…\Stellaris\mod\` | `Documents\…\Stellaris\plugins\` |
| Game versions | usually works across minor versions | depends on addresses inside the exe: **may break with every game update** |
| Achievements / Ironman | depends on what it changes | changes no checksummed file, but can do anything: users must trust the author |

The launcher keeps plugins, enables them per playset, checks that each fits the installed game build, makes and edits their settings files,
updates them, and loads them once the game is up.

**Only the launcher loads plugins.** Spec v2 has no proxy DLLs (a stand-in `d3dx9_43.dll` or the like) and no loaders placed in the game
folder. A game started from Steam or the Paradox Launcher runs without plugins; that is intended.

---

## 2. Ten-minute start

The example plugin's id is `hello-stellaris`.

**1. Make a folder.** It can be anywhere; during development the launcher uses it where it is ("link"), with no copying.

```
D:\dev\hello-stellaris\
  stl-plugin.json
  hello_stellaris.dll        ← your build output
  defaults\
    hello_stellaris.ini
```

**2. Write the manifest** `stl-plugin.json`:

```json
{
  "schema": 2,
  "id": "hello-stellaris",
  "name": "Hello Stellaris",
  "version": "0.1.0",
  "description": "A minimal plugin.",
  "dll": "hello_stellaris.dll",
  "game": { "exe_timestamps": [] },
  "load": { "wait": "window", "delay_ms": 1000 },
  "config": [
    { "file": "hello_stellaris.ini", "default": "defaults/hello_stellaris.ini", "title": "Hello" }
  ]
}
```

**3. Write the DLL** from the template in [Appendix A](#appendix-a-a-minimal-c-template). Its `DllMain` only starts a thread; the thread finds
the plugin's folder, reads `config\hello_stellaris.ini` and writes a line to `logs\hello.log`.

**4. Link it to the launcher.** While developing, link: the launcher uses the folder in place, so a rebuild is picked up at the next game
start.

```
stl plugin install D:\dev\hello-stellaris --link
```

Or use *Link a plugin under development* on the Plugins page.

**5. Enable it in a playset:**

```
stl plugin enable hello-stellaris
```

Or switch it on under Playsets → Plugins. Also check that Settings → Launch → *Load DLL plugins* is on.

**6. Start the game:**

```
stl launch
```

Or press *Play* in the launcher. The command line prints `plugin hello-stellaris: loaded`.

**7. See the result** in `D:\dev\hello-stellaris\logs\hello.log`.

> A game that is already running can be given a DLL by hand: `stl inject D:\dev\hello-stellaris\hello_stellaris.dll`. That is for development
> only. A loaded DLL cannot be swapped for a new build without restarting the game; see [6.6](#66-never-unload-yourself).

---

## 3. The plugin folder

Every plugin has **one folder**, next to the game's own `mod` folder:

```
Documents\Paradox Interactive\Stellaris\
  mod\                          the game's mods (nothing to do with plugins)
  plugins\
    <id>\                       the folder name is the plugin id
      stl-plugin.json           the manifest (MUST)
      <yours>.dll               the main DLL (MUST); DLLs it needs sit beside it
      defaults\                 the default versions of the settings files (read-only, replaced by an update)
      data\                     the plugin's own read-only data (MAY)
      config\                   the user's settings (made and edited by the launcher; kept by an update)
      logs\                     where the plugin writes its logs (MAY)
```

Its contents belong to one of two owners:

| The plugin package (replaced whole by an update) | The user (kept by an update, never shipped) |
|---|---|
| `stl-plugin.json`, the DLL, `defaults\`, `data\`, other shipped files | `config\` (kept as it is by an update), `logs\` |

Rules:

1. The plugin **MUST** find its folder from its own module path (see [6.2](#62-find-your-own-folder)), never from the working folder or the
   game folder.
2. It **MUST NOT** write into `defaults\`, `data\` or next to the DLL: the next update replaces them whole, and the user's changes would be
   lost.
3. It **MUST NOT** write anything into the game folder.
4. A release package **MUST NOT** contain `config\` or `logs\`: they belong to the user.
5. A plugin linked for development uses the folder where it is; its `config\` and `logs\` are there too.

---

## 4. The manifest: stl-plugin.json

The manifest is JSON in UTF-8 (a BOM is accepted).

### 4.1 Full example

```json
{
  "schema": 2,
  "id": "stellaris-live2d",
  "name": "Live2D portraits",
  "version": "0.2.0",
  "description": "Draws Live2D models into Stellaris' portraits (portrait mods declare them).",
  "dll": "stellaris_live2d.dll",
  "game": { "exe_timestamps": ["0x6ABEAA3F"] },
  "load": { "wait": "window", "delay_ms": 1500 },
  "config": [
    { "file": "stellaris_live2d.ini", "default": "defaults/stellaris_live2d.ini", "title": "Live2D", "substitute": true }
  ],
  "update": { "github": "Yidhar/stellaris-live2d", "asset": "stellaris-live2d-*.zip" },
  "homepage": "https://github.com/Yidhar/stellaris-live2d"
}
```

### 4.2 Fields

| Field | Required | Type | Meaning |
|---|---|---|---|
| `schema` | SHOULD | integer | `2` |
| `id` | **yes** | string | letters, digits, `-`, `_`, `.` only. Also the folder name, the reference in playsets and the name on the command line. **Do not change it after the first release** |
| `name` | **yes** | string | the name people see |
| `version` | SHOULD | string | a dotted numeric version such as `0.2.0`; updates compare it (see [11.2](#112-comparing-versions)) |
| `description` | MAY | string | one sentence, shown on the Plugins page |
| `dll` | **yes** | string | the main DLL, relative to the plugin folder; not absolute, no `..` |
| `game.exe_timestamps` | SHOULD | array of strings | the PE timestamps of the `stellaris.exe` builds the plugin was made for, in hex, e.g. `"0x6ABEAA3F"`. When listed, the plugin is loaded only into these builds; an empty list means no check (see [section 7](#7-game-build-compatibility)) |
| `load.wait` | MAY | `"window"` / `"none"` | `window` (default): load once the game has a visible window; `none`: as soon as the process exists |
| `load.delay_ms` | MAY | integer | how many more milliseconds to wait after `wait` is met |
| `config` | MAY | array | the settings files; see [4.3](#43-config-entries) |
| `update.github` | MAY | string | the GitHub repository `owner/repo` (`https://github.com/owner/repo` is accepted too). Needed for automatic updates |
| `update.asset` | MAY | string | the release asset to install, `*` as a wildcard; `*.zip` by default |
| `homepage` | MAY | string | a page for people; the Plugins page links it (the `update.github` repository when empty) |
| `seed_files` | do not use | array | schema 1, writes files into the game folder. Still honoured, but new plugins **MUST NOT** use it; see [Appendix C](#appendix-c-moving-from-schema-1) |

Unknown fields are ignored, which leaves room for later versions; do not use that to store data of your own (that goes in `data\`).

### 4.3 `config` entries

| Field | Meaning |
|---|---|
| `file` | a file name in `config\`: **no sub-folders**, no `..` |
| `default` | a file in the plugin folder, e.g. `defaults/x.ini`. When `config\` lacks the file, the launcher copies it from here |
| `title` | what the settings editor calls the file; the file name when empty |
| `substitute` | `true`: while copying the default, replace `{plugin_dir}` and `{config_dir}` with the actual absolute paths |

Files in `config\` that are not declared are shown in the editor too (after the declared ones), so settings files the plugin writes at run
time can be edited as well.

### 4.4 What the launcher checks

A manifest that fails any of these is listed as a problem and its plugin is not loaded:

- valid JSON;
- `id` not empty, allowed characters only;
- `dll` not empty, relative, without `..`;
- each `config` `file` without `/`, `\` or `..`, each `default` a relative path inside the plugin folder;
- `seed_files` paths not leaving their folders.

On install, the file `dll` names must exist; when installing an update, the `id` in the package's manifest must match the installed one.

---

## 5. Lifecycle: from install to injection

### 5.1 Install

| How | Result |
|---|---|
| `stl plugin install <folder>`, or *Install* on the Plugins page | copied to `plugins\<id>\` |
| `stl plugin install <folder> --link`, or *Link a plugin under development* | not copied: the folder is used where it is (for development) |
| automatic update | the release package is downloaded, checked, and installed like a folder |

Installing over an existing plugin (updates included) is **atomic**:

1. the new version is prepared completely in `plugins\.<id>.new\`;
2. the installed `config\` is copied into it (the user's settings win over the new defaults), then missing settings files are made;
3. the old folder is swapped for the new one by renaming.

While the game runs it holds the old DLL, so the rename fails: the installed version and its settings stay **exactly as they were**, and the
user retries after closing the game. The old folder may wait as `.<id>.old` and is deleted once nothing holds it.

### 5.2 Enabling

Plugins are enabled **per playset**: the switch under Playsets → Plugins, or `stl plugin enable|disable <id>` (for the active playset). With
the global switch Settings → Launch → *Load DLL plugins* off, no plugin is loaded.

### 5.3 Checks at launch

When the user presses *Play* or runs `stl launch`, for each enabled plugin of the active playset:

| Case | Result |
|---|---|
| not installed | skipped: `not installed` |
| the DLL file is missing | skipped |
| `exe_timestamps` listed, the current game build not among them | skipped, naming the builds it is for and the current one |
| no `exe_timestamps` | loaded (no check) |

Then the missing settings files are made and the game is started.

### 5.4 Injection

```
the game process starts
   │
   ├─ load.wait = "window": wait until the game has a visible window (its title contains "stellaris")
   ├─ then wait load.delay_ms more
   │
   ├─ already in the process? → not loaded again
   │
   └─ a remote thread in the game process runs LoadLibraryW("<plugin folder>\<dll>")
         ├─ waits for it to return, at most 30 s
         └─ checks that the module really is in the game's process
```

Facts to keep in mind:

- Plugins are loaded **one after another**, in the order of the playset, each with its own wait; `delay_ms` counts from when that plugin
  starts waiting.
- Waiting for the game has an overall limit of 180 s; past it, or if the game exits, the load is abandoned and the reason reported.
- **`DllMain` runs on the remote thread the launcher created, not on the game's main thread.**
- When the window appears the game is usually **still loading its databases** (the loading screen). A plugin cannot assume the engine is
  fully initialised, and it never sees the game's early start-up.
- `LoadLibraryW` returning `NULL` (`DllMain` returned `FALSE`, or a DLL it needs was not found) is reported as a failed load.
- The launcher **never** calls `FreeLibrary` on a plugin in the game. A plugin stays until the game exits.

### 5.5 Removal

`stl plugin remove <id>`, or deleting it on the Plugins page: the folder is renamed first, then deleted. While the game runs and holds the
DLL, the removal fails whole instead of leaving half a folder. A linked plugin is only unlinked; its folder is not touched.

---

## 6. Rules for the DLL

### 6.1 Keep DllMain minimal

`DllMain` runs under the loader lock. Waiting, loading other libraries, creating windows or calling the engine there can **dead-lock the whole
game**.

`DllMain` **MUST** do as little as possible:

```cpp
BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        HANDLE t = CreateThread(nullptr, 0, InitThread, module, 0, nullptr);  // the real start-up happens there
        if (t) CloseHandle(t);
    }
    return TRUE;
}
```

In `DllMain` the plugin **MUST NOT**:

- `WaitForSingleObject`, `Sleep`, or wait for a thread it created;
- `LoadLibrary` (implicit dependencies are loaded by the system; that is fine);
- call game or D3D functions;
- do slow file or network work.

### 6.2 Find your own folder

Take the DLL's path from **an address inside your own module**; its parent folder is the plugin folder. The same code works when installed,
linked, or injected by hand:

```cpp
const std::wstring& PluginDir() {
    static const std::wstring dir = [] {
        HMODULE self = nullptr;
        wchar_t path[MAX_PATH * 4] = {};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(&PluginDir), &self);
        GetModuleFileNameW(self, path, static_cast<DWORD>(std::size(path)));
        std::wstring p(path);
        return p.substr(0, p.find_last_of(L"\\/") + 1);   // with the trailing backslash
    }();
    return dir;
}
```

The plugin **MUST NOT** use the current working folder: in the game's process that is the game folder.
It **MUST NOT** hard-code `Documents\…\plugins\<id>`: during development the plugin is linked from elsewhere, and the Documents folder may be
redirected.

### 6.3 Threads: call the engine only on the game's main thread

This is what most often makes the game **crash or freeze without a word**.

- Engine functions, game commands, and objects the main thread changes concurrently **MUST** only be touched on the game's own thread
  (usually the render/main thread). The common way: hook `IDXGISwapChain::Present` or a function of the main loop, run a task queue in the
  hook, and let other threads only put tasks in that queue.
- The plugin **MUST NOT** call the engine's UI or logic functions directly from its own thread or from a remote thread.
- Its own threads are for waiting, files, networking, a named-pipe server, computation.

### 6.4 Read memory without crashing

After a game update addresses move; pointers can be null or freed.

- Raw reads of game memory **MUST** be wrapped in SEH (`__try / __except`): one bad pointer must not take the game down.
- Functions and globals **SHOULD** be located by pattern scanning rather than hard-coded RVAs, which may survive small patches. When locating
  fails, switch the feature off safely and write it to the log.
- Do not rely on structure offsets from a decompile of the Linux build: Windows (MSVC) and Linux (GCC) lay objects out differently.

### 6.5 Living with other plugins

Several plugins can run in one game, and they often hook the same function (`Present`, for one).

- A hook **MUST** call the original function (the trampoline) and pass the call on down the chain.
- It **MUST NOT** assume it is the first or last hook, and **MUST NOT** restore or overwrite other hooks.
- Hook only what you need (SHOULD); keep the work inside hooks short and do the heavy work on your own thread.
- Put shared state back as it was: window subclassing, `SetWindowLongPtr` window procedures, D3D state.

### 6.6 Never unload yourself

The launcher never unloads plugins, and a plugin **MUST NOT** unload itself (`FreeLibraryAndExitThread`) or other modules.

Unloading a DLL that has hooks installed, even with a `DLL_PROCESS_DETACH` handler, very easily crashes the game when several plugins hook
across each other. To try a new build during development: **close the game and start it again**.

### 6.7 Other rules

- **MUST NOT** open console windows (`AllocConsole`), or start console programs without `CREATE_NO_WINDOW`.
- **SHOULD** link the C/C++ runtime statically (MSVC `/MT`), or ship the runtime DLLs it needs in the plugin folder, so users without the
  VC++ redistributable can run it.
- x64 only.
- The plugin's own errors must not bring the game down: when start-up fails, log it and quietly do nothing.

---

## 7. Game build compatibility

Plugins usually depend on addresses and structure layouts inside `stellaris.exe`, which can be wrong after any game update.
`game.exe_timestamps` exists for exactly this.

**What it is:** the PE header timestamp of `stellaris.exe` (`IMAGE_FILE_HEADER.TimeDateStamp`), different for every build. To see the
current game's:

```
> stl status
game        Cygnus v4.5.2 (9776) (…\Stellaris)
exe build   0x6ABEAA3F (2026-10-01 18:45 UTC)
```

**How to use it:**

- List the builds you have tested; several are allowed.
- The launcher loads the plugin only into listed builds; on a mismatch it tells the user which builds the plugin is for and which one is
  installed.
- After a game update: verify the addresses again (pattern scans, SDK generator, …), **add** the new timestamp to the list, and release a new
  version. Users get it through the automatic update.

**An empty list** means "I check for myself, or I do not depend on the build". The plugin then **MUST** decide at run time (switch itself off
when a pattern is not found, for instance) and never blindly read or write memory in an unknown build.

Do both (SHOULD): list the builds in the manifest, and check again inside the DLL. If the DLL is injected by hand or the manifest is edited,
its own check still protects the game.

---

## 8. Settings files

### 8.1 Where they live, and how they are made

- The user's settings are in `<plugin folder>\config\`, and the plugin **MUST** read them from there.
- Defaults go in `defaults\` and are declared in the manifest's `config`. On install, on update and before each launch the launcher makes the
  files missing from `config\`; **an existing file is never overwritten**.
- If a file is missing (the plugin was injected by hand, say), the plugin **MUST** run with built-in defaults.

### 8.2 Encoding and format

- Text files are UTF-8; a BOM at the start **SHOULD** be tolerated (the launcher's editor keeps a file's encoding and line endings).
- Any format: INI, JSON, TOML. The launcher's editor gives syntax hints for INI and JSON.
- For paths in settings, use `"substitute": true` and write `{plugin_dir}` / `{config_dir}` in the default file; they become absolute paths
  when the file is made.

### 8.3 Editing, and reloading while the game runs

On the Plugins page the gear button opens the settings editor, with *Save* (writes in place), *Restore default* and *Open folder*.

The plugin **SHOULD** re-read its settings while the game runs. The simplest way: check the file's modification time every second or two and
re-read when it changes (as in [Appendix A](#appendix-a-a-minimal-c-template), and as stellaris-mcp does). A plugin that only reads at start
needs a game restart after a change; it **SHOULD** say so in the default file's comments.

---

## 9. Logs and troubleshooting

### 9.1 The plugin's logs

- Write them to `<plugin folder>\logs\`, not into `config\` (the user is shown that folder as settings) and not into the game folder.
- The first line **SHOULD** give the plugin version, the plugin folder, the settings file read and the game build: the most useful facts when
  something goes wrong.
- Open the log so that others can read it while the game runs (`_wfsopen` with `_SH_DENYWR`, not `_wfopen_s`, which locks it until the game
  exits).
- Keep it small: start a new one each run or rotate; do not let it grow without end.

### 9.2 What the launcher tells you

| Where | What |
|---|---|
| the output of `stl launch`, the launcher window's log | for each plugin: `loaded` / skipped / the reason it failed |
| `stl plugins` | the installed plugins, and whether each fits the current game build |
| `stl plugin info <id>` | manifest, folder, SHA-256 of the DLL |
| `Documents\…\Stellaris\logs\error.log` | the game's own error log |

### 9.3 Common problems

| Symptom | Cause |
|---|---|
| `LoadLibraryW failed in the game` | the DLL or a DLL it needs was not found (no VC++ runtime?); `DllMain` returned `FALSE`; not x64 |
| `LoadLibraryW did not return within 30 s` | `DllMain` waits or dead-locks; see [6.1](#61-keep-dllmain-minimal) |
| `is not among the game's modules` | the DLL unloaded itself after loading |
| skipped: `made for builds …` | the game was updated and `exe_timestamps` lacks the new build |
| install or update fails: `a file in it is in use` | the game is running and holds the DLL; close it and retry, the installed version is unaffected |
| the game freezes or crashes | engine called from a thread other than the main one; raw memory read without SEH; a hook not calling the original |

### 9.4 Debugging

Attach Visual Studio to `stellaris.exe` (*Attach to Process*) and load the plugin's PDB. During development, link the plugin folder and have
the build write into it: close the game, rebuild, start again.

---

## 10. Packaging and releases

### 10.1 The package is the plugin folder

A release package is a **zip** whose contents are the plugin folder:

```
hello-stellaris-v0.2.0.zip
  stl-plugin.json
  hello_stellaris.dll
  defaults\hello_stellaris.ini
  data\…                        (if any)
```

The manifest may be at the root of the zip, or inside **a single** folder.

- It **MUST** contain the manifest and the file `dll` names.
- It **MUST NOT** contain `config\`, `logs\`, or any file meant for the game folder.
- The manifest's `version` must equal the release's version.

Users install it:

- through the launcher's automatic update (recommended);
- by downloading and unpacking it, then *Install* on the Plugins page;
- by unpacking it straight into `Documents\Paradox Interactive\Stellaris\plugins\<id>\`.

### 10.2 GitHub releases

Automatic updates read the repository's **latest full release** (*Latest release*):

| Requirement | Meaning |
|---|---|
| tag | `v0.2.0` or `0.2.0`, equal to the manifest's `version` |
| asset | a zip matching `update.asset`, e.g. `hello-stellaris-v0.2.0.zip` |
| checksum file (SHOULD) | `<asset name>.sha256`: the hex digest, optionally followed by the file name: `<sha256>  hello-stellaris-v0.2.0.zip`. When present, the download must match it |
| pre-releases, drafts | never offered as updates |

### 10.3 CI example (GitHub Actions)

```yaml
on:
  push:
    tags: ["v*"]
permissions:
  contents: write
jobs:
  release:
    runs-on: windows-2022
    steps:
      - uses: actions/checkout@v4
      - name: Tag matches the manifest
        shell: pwsh
        run: |
          $v = (Get-Content plugin/stl-plugin.json -Raw | ConvertFrom-Json).version
          if ("v$v" -ne $env:GITHUB_REF_NAME) { throw "tag $env:GITHUB_REF_NAME != manifest version $v" }
      - name: Build
        run: |
          cmake -S . -B build -G "Visual Studio 17 2022" -A x64
          cmake --build build --config Release
      - name: Package
        shell: pwsh
        run: |
          $name = "hello-stellaris-$env:GITHUB_REF_NAME"
          $dir = "package/$name"
          New-Item -ItemType Directory -Force $dir, "$dir/defaults" | Out-Null
          Copy-Item plugin/stl-plugin.json $dir/
          Copy-Item plugin/defaults/* "$dir/defaults/"
          Copy-Item build/Release/hello_stellaris.dll $dir/
          Compress-Archive -Path "$dir/*" -DestinationPath "package/$name.zip"
          $h = (Get-FileHash "package/$name.zip" -Algorithm SHA256).Hash.ToLower()
          "$h  $name.zip" | Set-Content -Encoding ascii "package/$name.zip.sha256"
      - uses: softprops/action-gh-release@v2
        with:
          files: package/*.zip*
```

The `stellaris-perf` repository's `tools/check_plugin.py` is a fuller example: before packaging it checks the manifest's fields, that the
package has no `config\`, and that the tag equals the version.

---

## 11. Automatic updates

### 11.1 How it works

1. Once per session the Plugins page checks every plugin that declares `update` (and again on *Check for updates*); on the command line,
   `stl plugin update [<id>] [--check]`.
2. The launcher reads the latest release of the `update.github` repository; when its tag is a higher version than the manifest's `version`,
   the Plugins page shows *Update*.
3. Pressing it downloads the zip matching `update.asset` and, when a `.sha256` is published, checks it: a mismatch is refused.
4. The zip is unpacked, its manifest's `id` must match, and it is installed atomically as in [5.1](#51-install), **keeping the user's
   `config\`**.
5. While the game runs the DLL cannot be replaced: the user is told to close the game, and the new version loads at the next start.

**Linked** plugins (under development) are never updated.

### 11.2 Comparing versions

- A leading `v` is dropped and the numbers between the dots are compared one by one: `0.10.0` is newer than `0.9.2`, `1.0` newer than `0.9`.
- Anything after `-` or `+` is ignored: `0.2.0-rc1` counts as `0.2.0` and is **not** offered as an update. Mark previews as pre-releases on
  GitHub.
- A missing number counts as 0: `0.2.1` is newer than `0.2`.

### 11.3 When the game is updated

After a game update, old plugins are skipped because `exe_timestamps` does not match: that is the protection working, not a fault. The author
SHOULD:

1. verify the plugin on the new build;
2. add the new timestamp to `exe_timestamps`;
3. raise the version, tag, and release.

Users then press *Update* on the Plugins page.

---

## 12. Code of conduct

A plugin has the game process's rights; the launcher cannot sandbox it. So that users can trust plugins:

- A plugin **MUST NOT** change the game folder, saves, or other plugins' folders.
- It **MUST NOT** go online, collect or upload anything without the user knowing. A feature that needs the network **MUST** be described in
  the plugin's description and settings, and be off by default or possible to switch off.
- It **MUST NOT** load other plugins or install loaders (proxy DLLs, autorun registry entries, …).
- It **MUST NOT** get around the launcher's build check (by injecting itself into an unsupported build, say).
- It **SHOULD** be open source, or at least publish its release page and checksums, and **SHOULD** use `homepage` to say what it does, its
  known problems and the game versions it supports.
- A feature that affects multiplayer synchronisation or achievements **MUST** say so plainly in the description.

---

## 13. Release checklist

**Manifest**
- [ ] `"schema": 2`; `id` valid and unchanged; `version` raised
- [ ] `exe_timestamps` lists every tested build (or is empty, and the DLL checks the build itself)
- [ ] every `default` file named in `config` is in the package
- [ ] for automatic updates: `update.github` and `update.asset` are right

**DLL**
- [ ] `DllMain` only starts a thread: no waiting, no loading libraries
- [ ] the plugin folder comes from the module path; settings only from `config\`, logs to `logs\`
- [ ] runs with built-in defaults when a settings file is missing
- [ ] engine calls only on the game's main thread; raw memory reads under SEH
- [ ] hooks call the original; the plugin never unloads itself
- [ ] x64, static runtime (or the runtime shipped with it); no console windows
- [ ] does nothing harmful on a build it does not list (when injected by hand)

**Package and release**
- [ ] the zip's contents are the plugin folder; no `config\`, `logs\` or files for the game folder
- [ ] tag = `v` + `version`; asset name matches `update.asset`; `.sha256` alongside
- [ ] a full release (not a pre-release or draft)

**Tested**
- [ ] fresh install → enable → `stl launch`: `loaded` in the log, the feature works
- [ ] automatic update from the previous version: settings kept, the new version loads at the next start
- [ ] *Update* while the game runs: fails safely, the installed version is unaffected
- [ ] together with other common plugins: no conflict

---

## Appendix A: a minimal C++ template

The code below builds a plugin that follows this spec. It does four things:

- starts up on a thread of its own;
- finds the plugin folder;
- reads `config\hello_stellaris.ini` (built-in defaults when it is missing), checks the file's modification time every 2 seconds, and re-reads
  it when it changes;
- writes its log to `logs\hello.log`.

Your hooks and game logic start where the comment says `Work()`; remember that engine calls must move to the game's main thread (see
[6.3](#63-threads-call-the-engine-only-on-the-games-main-thread)). This template was compiled with MSVC (`/W4`, no warnings), loaded into
Stellaris 4.5.2 with `stl inject`, and read and reloaded its settings there.

**CMakeLists.txt**

```cmake
cmake_minimum_required(VERSION 3.20)
project(hello_stellaris CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")   # /MT: no VC++ runtime needed
add_library(hello_stellaris SHARED src/plugin.cpp)
target_compile_definitions(hello_stellaris PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX)
```

**src/plugin.cpp**

```cpp
#include <windows.h>
#include <cstdarg>
#include <cstdio>
#include <fstream>
#include <iterator>
#include <share.h>
#include <string>

namespace {

// ---- the plugin's folder: from this module, never the working folder
const std::wstring& PluginDir() {
    static const std::wstring dir = [] {
        HMODULE self = nullptr;
        wchar_t path[MAX_PATH * 4] = {};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(&PluginDir), &self);
        GetModuleFileNameW(self, path, static_cast<DWORD>(std::size(path)));
        std::wstring p(path);
        return p.substr(0, p.find_last_of(L"\\/") + 1);
    }();
    return dir;
}

// ---- logs\hello.log, started anew each run
FILE* g_log = nullptr;
void Log(const char* fmt, ...) {
    if (!g_log) return;
    va_list args;
    va_start(args, fmt);
    vfprintf(g_log, fmt, args);
    va_end(args);
    fputc('\n', g_log);
    fflush(g_log);
}

// ---- config\hello_stellaris.ini: [general] greeting = ...; built-in defaults when missing
struct Settings {
    std::string greeting = "hello";
};

std::wstring SettingsPath() { return PluginDir() + L"config\\hello_stellaris.ini"; }

std::string Trim(std::string s) {
    const char* ws = " \t\r\n";
    s.erase(0, s.find_first_not_of(ws));
    s.erase(s.find_last_not_of(ws) + 1);
    return s;
}

Settings ReadSettings(bool* found) {
    Settings s;
    std::ifstream in(SettingsPath(), std::ios::binary);
    *found = in.is_open();
    std::string line, section;
    bool first = true;
    while (std::getline(in, line)) {
        if (first && line.rfind("\xEF\xBB\xBF", 0) == 0) line.erase(0, 3);  // UTF-8 BOM
        first = false;
        line = Trim(line);
        if (line.empty() || line[0] == ';' || line[0] == '#') continue;
        if (line.front() == '[' && line.back() == ']') { section = line.substr(1, line.size() - 2); continue; }
        auto eq = line.find('=');
        if (eq == std::string::npos) continue;
        if (section == "general" && Trim(line.substr(0, eq)) == "greeting") s.greeting = Trim(line.substr(eq + 1));
    }
    return s;
}

FILETIME ModifiedTime() {
    WIN32_FILE_ATTRIBUTE_DATA d{};
    return GetFileAttributesExW(SettingsPath().c_str(), GetFileExInfoStandard, &d) ? d.ftLastWriteTime : FILETIME{};
}

// ---- the plugin's own thread: everything happens here (or in hooks it installs)
DWORD WINAPI InitThread(LPVOID) {
    CreateDirectoryW((PluginDir() + L"logs").c_str(), nullptr);
    // _wfsopen with _SH_DENYWR: others may read the log while the game runs (_wfopen_s would lock it until the game exits)
    g_log = _wfsopen((PluginDir() + L"logs\\hello.log").c_str(), L"w", _SH_DENYWR);

    bool found = false;
    Settings settings = ReadSettings(&found);
    Log("hello-stellaris 0.1.0, folder %ls, settings %s", PluginDir().c_str(), found ? "read" : "missing: defaults");
    Log("greeting: %s", settings.greeting.c_str());

    // Work(): install hooks here; anything that calls the engine must run on the game's main thread.

    // re-read the settings when the launcher's editor saves them
    FILETIME seen = ModifiedTime();
    for (;;) {
        Sleep(2000);
        FILETIME now = ModifiedTime();
        if (CompareFileTime(&now, &seen) != 0) {
            seen = now;
            settings = ReadSettings(&found);
            Log("settings %s; greeting: %s", found ? "reloaded" : "removed (defaults)", settings.greeting.c_str());
        }
    }
}

}  // namespace

// DllMain: under the loader lock, so it only starts the thread.
BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        if (HANDLE t = CreateThread(nullptr, 0, InitThread, nullptr, 0, nullptr)) CloseHandle(t);
    }
    return TRUE;
}
```

**defaults/hello_stellaris.ini**

```ini
; Hello Stellaris settings. Saved changes are picked up within two seconds while the game runs.
[general]
greeting = hello
```

---

## Appendix B: JSON Schema of the manifest

Point your editor at this schema for `stl-plugin.json` to get completion and checks (the launcher's own checks are what count):

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Stellaris Launcher plugin manifest (schema 2)",
  "type": "object",
  "required": ["id", "name", "dll"],
  "properties": {
    "schema": { "const": 2 },
    "id": { "type": "string", "pattern": "^[A-Za-z0-9._-]+$" },
    "name": { "type": "string", "minLength": 1 },
    "version": { "type": "string", "pattern": "^v?\\d+(\\.\\d+)*([-+].*)?$" },
    "description": { "type": "string" },
    "dll": { "type": "string", "pattern": "^(?![A-Za-z]:|[\\\\/])(?!.*\\.\\.).+$" },
    "game": {
      "type": "object",
      "properties": {
        "exe_timestamps": { "type": "array", "items": { "type": "string", "pattern": "^(0[xX])?[0-9A-Fa-f]{1,8}$" } }
      }
    },
    "load": {
      "type": "object",
      "properties": {
        "wait": { "enum": ["window", "none"] },
        "delay_ms": { "type": "integer", "minimum": 0 }
      }
    },
    "config": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string", "pattern": "^[^\\\\/]+$" },
          "default": { "type": "string" },
          "title": { "type": "string" },
          "substitute": { "type": "boolean" }
        }
      }
    },
    "update": {
      "type": "object",
      "required": ["github"],
      "properties": {
        "github": { "type": "string" },
        "asset": { "type": "string" }
      }
    },
    "homepage": { "type": "string" }
  }
}
```

---

## Appendix C: moving from schema 1

Schema 1 used `seed_files` to write settings files into the **game folder**, and allowed proxy DLLs to load plugins on their own. Spec v2:

| Schema 1 | Schema 2 |
|---|---|
| `seed_files: [{ "from": "x.default.ini", "to": "x.ini" }]` (into the game folder) | `config: [{ "file": "x.ini", "default": "defaults/x.ini" }]` (into the plugin's own `config\`) |
| the plugin reads its settings from the game folder | the plugin reads them from `<plugin folder>\config\` |
| a proxy DLL (`d3dx9_43.dll`, …) loads it automatically | only the launcher injects it; remove the proxy DLL |
| plugins in `%APPDATA%\stellaris-launcher\plugins` | plugins in `Documents\Paradox Interactive\Stellaris\plugins` (the launcher moves them once) |

Steps:

1. Move the default settings file into `defaults\`, declare it in `config`, and remove `seed_files`.
2. Make the DLL read its settings from `PluginDir() + L"config\\…"`, and write its logs to `logs\`.
3. Remove the proxy loader. Code that reads the old settings from the game folder may stay for one version as a fallback, then go.
4. Tell users they can delete the old settings file and proxy DLL from the game folder (the plugin itself **MUST NOT** delete files in the
   game folder).
5. Set `schema` to `2`, raise the version, and release.
