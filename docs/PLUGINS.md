# DLL plugins (spec, schema 2)

A plugin is a native Windows DLL that is loaded into `stellaris.exe` (hooks, tools, a bridge to something outside). The launcher keeps a list of
them, says which belong to which playset, checks that each was made for the installed game build, makes its settings files, lets the user edit
them, and loads the plugin when the game is up.

## Where a plugin lives

Every plugin has one folder, next to the game's own `mod` folder:

```
Documents\Paradox Interactive\Stellaris\
  mod\                         the game's mods (not ours)
  plugins\
    <id>\                      one folder per plugin, named by its id
      stl-plugin.json          the manifest (below)
      <id>.dll                 the DLL (any DLLs it needs sit beside it)
      config\                  the user's settings: the launcher's Plugins page lists and edits every file in here
        <name>.ini
      defaults\                the defaults the launcher makes config\ files from (read-only, replaced by an update)
        <name>.ini
      data\                    optional: the plugin's own read-only data
      logs\                    optional: where the plugin writes its logs
```

Rules for the plugin:

1. **Find your folder from your own module**, not from the working folder or the game folder: `GetModuleHandleExW(FROM_ADDRESS, <a function
   of yours>)` + `GetModuleFileNameW` gives `...\plugins\<id>\<dll>`; its parent is your folder. Everything you read is relative to it.
2. **Read settings from `config\`** (`<folder>\config\<name>.ini`). Never write settings into the game folder. If a file is missing, use built-in
   defaults (the launcher normally makes it from `defaults\` first, but a plugin loaded some other way must still start).
3. **Do not write into `defaults\`** or next to the DLL: an update replaces the folder except `config\`.
4. **Optional, recommended: reload settings while the game runs** (watch the file's modification time, or re-read on a hot key). The launcher's
   editor saves in place and says so; a plugin that only reads at start needs a game restart.
5. Logs go to `<folder>\logs\`, not `config\` (that folder is shown to the user as settings).
6. Text files are UTF-8 (a BOM is tolerated). Paths in them may use `{plugin_dir}` / `{config_dir}`, which the launcher fills in when it makes
   the file from a default with `"substitute": true`.

## The manifest: `stl-plugin.json`

```json
{
  "schema": 2,
  "id": "stellaris-live2d",
  "name": "Live2D portraits",
  "version": "0.2.0",
  "description": "Draws Live2D models into Stellaris' portraits (portrait mods declare them).",
  "dll": "stellaris_live2d.dll",
  "game": { "exe_timestamps": ["0x6AB5181D"] },
  "load": { "wait": "window", "delay_ms": 1500 },
  "config": [
    { "file": "stellaris_live2d.ini", "default": "defaults/stellaris_live2d.ini", "title": "Live2D", "substitute": true }
  ]
}
```

| Key | Meaning |
|---|---|
| `schema` | `2` |
| `id` | letters, digits, `-` `_` `.`; also the folder name; names the plugin in playsets and on the command line |
| `name`, `version`, `description` | for people |
| `dll` | the DLL, a path inside the plugin folder (no `..`, not absolute) |
| `game.exe_timestamps` | the PE timestamps of the `stellaris.exe` builds it was made for. A plugin that locates addresses in the exe is only loaded into a build it lists; with none listed the launcher does not check. `stl status` prints the installed build |
| `load.wait` | `window` (default): load when the game has a visible window; `none`: as soon as the process exists |
| `load.delay_ms` | extra wait after that |
| `config[]` | the settings files: `file` (a name in `config/`, no sub-folders), `default` (a file in the plugin folder to make it from when missing), `title` (what the editor calls it), `substitute` (fill `{plugin_dir}`, `{config_dir}` in the default). Files in `config/` that are not declared are edited too |
| `seed_files` | schema 1, deprecated: files written into the game folder. Kept working for old plugins; new ones use `config` |

## What the launcher does

- **Install** (`stl plugin install <folder>`, or the Plugins page): copies the folder to `plugins\<id>\`. Over an existing install it keeps the
  files of `config\` (the user's settings win over new defaults), then makes the declared settings files that are missing.
- **Link** (`--link`, "Link a plugin under development"): uses the folder where it is (for development); its `config\` is in that folder.
- **Launch**: skips (and says why) a plugin that is not installed, whose DLL is missing, or that lists builds not including this one; makes
  missing settings files; starts the game; waits for its window and `delay_ms`; loads each plugin with `LoadLibraryW` in a remote thread. A
  plugin already in the process (loaded by a proxy DLL) is not loaded twice. The launcher never calls `FreeLibrary` in the game.
- **Settings**: the gear button of a plugin opens its `config\` files as text, with *Save*, *Restore default* and *Open folder*.
- An earlier version kept plugins in `%APPDATA%\stellaris-launcher\plugins`; they are moved here once (that folder becomes
  `plugins.migrated`).

## How plugins get into the game: only the launcher

Plugins are loaded **only by the launcher's injection** (above). There is no proxy-DLL / stand-in loading (`d3dx9_43.dll` or any other library
the game looks for):

- A plugin package contains nothing for the game folder, and a plugin never writes into the game folder.
- A plugin does not load other plugins and does not install loaders.
- Starting the game from Steam or the Paradox Launcher starts it **without** plugins; that is intended. Use `stl launch` or the Play button.
- `DllMain` stays minimal (no waiting, no loading of libraries under the loader lock); start your work on a thread of your own, or from your
  first hook. The game is already running when you are loaded: hook what exists, and do not assume you saw its start-up.

## Commands

```
stl plugins                            what is installed, and whether each fits the installed game build
stl plugin install <folder> [--link]   install (or link) a plugin
stl plugin info <id>                   manifest, folder, SHA-256 of the DLL
stl plugin enable|disable <id>         in the active playset
stl plugin remove <id>
stl launch                             writes dlc_load.json, starts the game, loads the active playset's enabled plugins
stl inject <dll> [--pid N]             load any DLL into the running game by hand
```
