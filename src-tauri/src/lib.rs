//! MediaForge: download and convert media you're authorised to use, on macOS and Windows.
//!
//! The engine (`engine`, `media`, `process`, `store`, `support`, `tools`) has no dependency on the window and
//! is tested on its own. `app` connects it to Tauri: commands, events, menus and start-up.

pub mod app;
pub mod artwork;
pub mod engine;
pub mod error;
pub mod licensing;
pub mod media;
pub mod models;
pub mod process;
pub mod store;
pub mod support;
pub mod tools;

pub use app::run;
