# DLL plugins

A plugin is a native Windows DLL that is loaded into `stellaris.exe` (hooks, tools, a bridge to something outside). The launcher keeps a list of
them, says which belong to which playset, checks that each was made for the installed game build, and loads them when the game is up.
Nothing is copied into the game folder except files a plugin's manifest asks for.

## The manifest: `stl-plugin.json`

It sits next to the DLL, in the plugin's folder. Example (`examples/plugins/stellaris-live2d/`):

```json
{
  "schema": 1,
  "id": "stellaris-live2d",
  "name": "Live2D portraits",
  "version": "0.1.0",
  "description": "Draws Live2D models into Stellaris' portraits (portrait mods declare them).",
  "dll": "stellaris_live2d.dll",
  "game": { "exe_timestamps": ["0x6AB5181D"] },
  "load": { "wait": "window", "delay_ms": 1500 },
  "seed_files": [
    { "from": "stellaris_live2d.default.ini", "to": "stellaris_live2d.ini", "if_missing": true, "substitute": true }
  ]
}
```

| Key | Meaning |
|---|---|
| `id` | letters, digits, `-` `_` `.`; names the plugin in playsets and on the command line |
| `name`, `version`, `description` | for people |
| `dll` | the DLL, a path inside the plugin folder (no `..`, not absolute) |
| `game.exe_timestamps` | the PE timestamps of the `stellaris.exe` builds it was made for. A plugin that locates addresses in the exe (all of this project's do) is only loaded into a build it lists; with none listed the launcher does not check (`Unchecked`). The timestamp is what `stl status` prints as "exe build" |
| `load.wait` | `window` (default): load when the game has a visible window; `none`: as soon as the process exists |
| `load.delay_ms` | extra wait after that |
| `seed_files` | files to create in the game folder: `from` (in the plugin folder) → `to` (relative to the game folder); `if_missing` (default true) never overwrites; `substitute` replaces `{plugin_dir}` and `{game_dir}` in the text. For an ini the plugin reads at start |

## Commands

```
stl plugin install <folder> [--link]   copy it to %APPDATA%\stellaris-launcher\plugins\<id>\ (--link keeps it where it is: for a plugin under development)
stl plugins                            what is installed, and whether each fits the installed game build
stl plugin info <id>                   manifest, folder, SHA-256 of the DLL
stl plugin enable <id>                 in the active playset
stl plugin disable <id>
stl plugin remove <id>
stl launch                             writes dlc_load.json, starts the game, loads the active playset's enabled plugins
stl inject <dll> [--pid N]             load any DLL into the running game by hand
```

## What the launcher does with them

1. Skips (and says why) a plugin that is not installed, whose DLL is missing, or that lists builds not including this one.
2. Creates the `seed_files` that are missing.
3. Starts the game, waits for its window (and `delay_ms`), and loads each plugin with `LoadLibraryW` in a remote thread — the DLL's own
   `DllMain` / start-up code does the rest. A plugin already in the process (loaded by a proxy DLL) is not loaded twice.
4. Reports plugins that failed to load; `stl launch` then exits with an error.

Plugins are loaded in playset order. Unloading is the plugin's business (the plugins of this project unload themselves on request); the launcher
never calls `FreeLibrary` in the game.

## Not done yet

Per-plugin settings pages, dependencies between plugins, checksums pinned in the manifest, a zip format, and a proxy-DLL mode for starting the
game from Steam directly. See DESIGN.md.
