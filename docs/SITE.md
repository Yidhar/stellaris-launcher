# stellarismod.com

How the launcher works with stellarismod.com, the community site of this launcher. The site's side is its `docs/LAUNCHER_API.md`
(github.com/Yidhar/stellarismod-wiki); the code here is `stl-core/src/site.rs` (links, API, account, playsets, wiki entries),
`stl-core/src/relay.rs` (one window), `stl-gui/src/site_ui.rs` and `stl-cli/src/site_cmd.rs`.

## `stl://` links

Registered for the current user under `HKCU\Software\Classes\stl` (`"<exe>" "%1"`): by the setup (`installer/stellaris-launcher.iss`),
or by the first start of a copy from the zip (`install::register_scheme`; a build under `target\` never registers). Uninstalling removes it.

| Link | What happens after "Go ahead" |
|---|---|
| `stl://playset/import?url=<site>/api/v1/playsets/<id>/export` | the playset is made; Workshop mods it lacks are subscribed to and downloaded, the site's packages and plugins downloaded and checked |
| `stl://playset/import?data=<base64url JSON>` | the same, for a playset carried in the link (at most 12 KB of base64url) |
| `stl://playset/add-mod?id=mod/ugc_<id>.mod` | the mod goes into the active playset (subscribed to first when it is not here) |
| `stl://mod/install-r2?id=<item>&version=<v>` | a package the site hosts: a plugin is installed as a plugin; a mod goes to `mod/<name>/` with its descriptor, into the active playset |
| `stl://plugin/install?id=<id>&version=<v>&sha256=<hex>` | the plugin, from the site's `plugins.jsonl` (a `sha256` in the link must be the site's) |
| `stl://index/add?url=<https URL>` | the plugin index is read from then on |
| `stl://account/link?code=<64 hex>&site=<site>` | the launcher signs in with the account the site allowed |

**A link only asks.** Any web page can open one, so the window shows what the link would do — the playset's mods and where each comes
from, the plugin with its download and SHA-256, the index URL — and changes nothing until you go ahead. `parse_link` checks every value
first: ids are `[A-Za-z0-9._-]` (no paths), descriptors `mod/<name>.mod`, playset URLs only the site's export path, index URLs https, the
sign-in code 64 hex digits for this site. Downloads must be https and match their SHA-256; a playset's mods are checked the same way
(`parse_export`). The command line asks the same questions on the terminal (`stl open <link>`, `--yes` to skip them).

**One window.** A start with a link while a window is open hands the link over a named pipe of this user
(`\\.\pipe\stellaris-launcher-links-<user>`, local clients only) and ends; the window comes to the front with the link's sheet. Without an
open window the start opens one and shows the sheet.

## The account

Device code: `POST /launcher/link` → the window shows the code and opens the site's page, where the member allows it; the launcher asks
`POST /launcher/token` every few seconds until it is allowed (`200`), refused (`403`) or ran out (`410`). The token
(`stl_…`) is kept in `%APPDATA%\stellaris-launcher\site-account.dat`, encrypted with DPAPI for the Windows user. Any `401` forgets it
(the member removed it on the site, or the account changed). Signing out calls `POST /auth/logout`. No `Origin` header is ever sent.

## Wiki entries

After an upload to the Workshop, a signed-in launcher offers "create its entry on stellarismod.com" (`stl wiki-entry <mod>` on the command
line): `GET /creator/workshop/<id>` (stop when it is listed or waits for review), a category (`GET /creator/categories`, the suggested one
first), optionally the mod's pages read from its files (`POST /creator/parse` with a zip of `common`, `events`, `localisation` in Simplified
Chinese and English, and `interface/*.gfx`) and their pictures (`POST /creator/icons` in batches of at most 400 files and 12 MB), then
`POST /creator/entries` with the Workshop requirements as relations. A member's entry waits for an admin (`202`).

## Testing

`STL_SITE=http://127.0.0.1:<port>` points the launcher at a local copy of the site. `APPDATA` set to a temporary folder keeps the playsets
and the account out of the real ones; `STL_PLUGINS_DIR` does the same for plugins.
