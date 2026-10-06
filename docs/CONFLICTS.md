# Mod conflicts: what the game does, and what the launcher reports

The Playsets page (Mods → **Check**) and `stl check` / `stl sort` read the mods of the **selected playset** and report three things:
problems that stop mods working, what overrides what, and a load order that respects what the mods need. This page says how the game
decides who wins, what was measured to know it, and how the launcher uses it (`crates/stl-core/src/conflicts.rs`).

## How the game decides (measured on 4.5.2)

Two test mods, A and B, defined the same things in files whose names sort differently (`zzz_stltest.txt` in A, `aaa_stltest.txt` in B),
and one file at the same path in both. The game was started with the mods in both orders, and each definition carried a marker (a reference
to something that does not exist) that the game reports in `error.log` with its file and line only if that definition survived loading.

| What | Result (same in both mod orders) |
|---|---|
| The **same file** in two mods | Only the file of the mod **loaded last** is read (the other's markers never appear). |
| `common/technology` | Files are read in **file-name order** (`aaa_` before `zzz_`), whatever the mod order; `Object with key … already exists, using the one at … zzz_stltest.txt`: the **last read wins** (LIOS). |
| `common/strategic_resources` | One copy survives, the last read (LIOS). The CWTools config says "duplicates", Irony says FIOS; both are wrong for 4.5.2. |
| `common/scripted_variables` | `Variable name … is already taken … zzz_stltest.txt`, and a user of the variable got B's value: the **first read wins** (FIOS). |
| `events` | `an event with id [stltest.1] already exists! … zzz_stltest.txt`: the first read wins (FIOS). |
| `common/section_templates` | `duplicate section template found … [STLTEST_SECTION] … zzz_stltest.txt`: the first read wins (FIOS). Entries are known by their inner `key`, not by `ship_section_template`. |
| `common/traits` | Both `opposites` were resolved after loading: **both copies are kept**. A trait defined twice is broken. |
| `common/scripted_triggers`, `common/scripted_effects` | `Object with key … using the one at … zzz_stltest.txt`: the last read wins (LIOS). |
| `common/agreement_term_values` | The same message: **LIOS**. The CWTools config says FIOS. |
| `common/component_templates` | Only the `aaa_` copy's prerequisite was resolved after loading (and the game logs `Component template key used multiple times`): FIOS. |
| `common/static_modifiers`, `common/scripted_loc` | Nothing tells which copy is used (both modifier icons are looked up; defined texts are checked only when shown). Static modifiers keep the CWTools rule (LIOS); scripted_loc, where the CWTools config and Irony disagree, is "unknown". |
| **The order of file names** | `Bcase_stltest.txt` was read before `acase_stltest.txt`: byte by byte, **upper case before lower case** (`FOXCrisis.txt` comes before `crisis_events_1.txt`). |
| A UTF-8 byte-order mark | The game takes it as part of the **first name** in the file (`﻿stltest_tech_lios`), so that first definition gets another name and overrides nothing. A comment after the mark is harmless (84 vanilla files start that way). |

So **the load order matters only for whole files.** Which of two definitions of the same name wins depends on the file names and the
folder's rule; that is why mods name files `00_…`, `!!!_…` or `zzz_…`.

Every other folder uses the rule of the CWTools Stellaris config (`config/override_modes.cwt`, MIT,
[github.com/Aa728848/cwtools-stellaris-config](https://github.com/Aa728848/cwtools-stellaris-config)): LIOS for most of `common/`,
FIOS for `component_templates`, `event_chains`, `global_ship_designs`, `scripted_loc`, `special_projects`, `solar_system_initializers`, …,
merged for `on_actions`, `job_tags`, `component_tags`, `trait_tags`, `defines`, both kept for `name_lists`, `terraform`, `achievements`.
Irony Mod Manager ([github.com/bcssov/IronyModManager](https://github.com/bcssov/IronyModManager), MIT) agrees except where measured above.
**Localisation** is not measured (the game logs nothing about duplicate texts): its duplicates are shown as possible only.

## What the launcher reports

**Problems** (red: breaks something; orange: probably wrong):

- the playset lists a mod that is not installed (unsubscribed), or whose files are gone;
- a dependency (`dependencies = { "name" }` in the descriptor) not installed, not on, or loaded after the mod;
- the same mod on twice (same name, or the same Workshop id local and subscribed);
- made for another *minor* version and **replacing game files whole**: the game's copies of those files may have changed since;
- files whose first definition a byte-order mark renames;
- definitions that do nothing: in a FIOS folder the game's file is read first. (In a LIOS folder a mod file that sorts *before* the
  game's — `!!!ph_…`, `000_…_dummy` — is a fallback on purpose, a placeholder for DLC content the player may not own; it is counted
  in the mod's details, not reported as a problem);
- definitions kept twice in a folder that keeps both;
- a patch loaded before the mod it patches (so its files lose).

**Overrides**: for each mod, how much of other mods it overrides and how much of it is overridden, with the details by the other mod:
the file or `folder: key`, which mod's version is used, and by which rule. A **patch** over what it patches is not a conflict: a mod is
taken as a patch of another when it declares the dependency, or when at least half of its own files (not the game's) are files the other
also brings and not the other way round.

**Order**: the current order changed as little as possible (a topological sort that always takes the earliest free mod), so that every
mod comes after the mods it needs and the mods it patches. Mods that need each other are left where they are and named. Nothing else is
reordered, because nothing else depends on the order. The change is shown first and applied only on request; switched-off mods keep
their places.

## Limits

- Only folders the game has are read (`.git/`, notes and archives inside mod folders are ignored); `replace_path` hides that folder of
  everything loaded before the mod.
- `interface/*.gui` and `gfx/*.gfx` are compared as whole files only, not by their element names.
- The rule for a folder nobody measured is the CWTools config's; a folder it does not list is "unknown" and its duplicates are shown as
  possible only.
- A playset of 112 mods (136 000 files, 3.4 million definitions including the game's) takes about 6 seconds, on a worker thread.
