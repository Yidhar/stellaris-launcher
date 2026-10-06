//! The core of the Stellaris launcher: finds the game, reads its mods and the official launcher's playsets, keeps our own playsets and DLL
//! plugins, writes what the game reads and starts it. The command line (`stl`) and the window are thin layers over this crate.

pub mod artwork;
pub mod dlc;
pub mod dlcload;
pub mod game;
pub mod import;
pub mod launch;
pub mod mods;
pub mod net;
pub mod news;
pub mod official;
pub mod paths;
pub mod pe;
pub mod plugins;
pub mod process;
pub mod saves;
pub mod script;
pub mod store;

pub use anyhow::{anyhow, bail, Context, Result};
