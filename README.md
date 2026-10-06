# stellaris-Launcher

Taking the Paradox Launcher apart, to build a launcher that can manage **native DLL mods** (plugins such as the Live2D portraits plugin, the
perf plugin, the MCP bridge) next to the usual `.mod` playsets, and that starts the game without three programs and a mandatory click.

Status: **research.** Nothing here runs the game yet.

- [docs/FINDINGS.md](docs/FINDINGS.md) — how the launcher works (the chain from Steam to `stellaris.exe`, where its data lives, the database, the
  files it writes for the game).
- [docs/PROBLEMS.md](docs/PROBLEMS.md) — what is wrong with it (L) and what went wrong while decoding it (D). Keep adding to it.
- `tools/asar_extract.py` — extracts the Electron `app.asar` of the launcher (no dependencies).

`decoded/` and `work/` hold Paradox's code, a copy of the launcher's database and beautified sources: they stay on this machine and are not in
git. Everything here is about this machine's Stellaris 4.5.1 installation and the launcher 2026.12-rc (open beta).
