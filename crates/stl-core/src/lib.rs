//! The core of the Stellaris launcher: finds the game, reads its mods and the official launcher's playsets, keeps our own playsets and DLL
//! plugins, writes what the game reads and starts it. The command line (`stl`) and the window are thin layers over this crate.
//!
//! It is a Windows program. Elsewhere (the wiki's Linux server) only the parts that read and check files build: the modules that start or
//! inspect the game, read the registry or display settings are Windows-only, and `net` has no client.

pub mod artwork;
pub mod dlc;
pub mod dlcload;
pub mod game;
#[cfg(windows)]
pub mod gamesettings;
pub mod import;
#[cfg(windows)]
pub mod install;
pub mod ironman;
pub mod conflicts;
pub mod leveldb;
pub mod selfupdate;
pub mod signed;
#[cfg(windows)]
pub mod launch;
pub mod modmake;
pub mod mods;
pub mod net;
pub mod news;
pub mod official;
pub mod paths;
pub mod pe;
pub mod pluginindex;
pub mod plugins;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod saves;
pub mod script;
pub mod store;
#[cfg(windows)]
pub mod textenc;
pub mod updates;
#[cfg(windows)]
pub mod uploadcheck;
#[cfg(windows)]
pub mod workshop;

pub use anyhow::{anyhow, bail, Context, Result};
