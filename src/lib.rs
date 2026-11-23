//! axross-files-matrix as a library: the parts of the city (the binary is
//! `src/main.rs`; the tests drive the window through [`app::App`]).

pub mod app;
pub mod bench;
pub mod cache;
pub mod camera;
pub mod city;
pub mod detach;
pub mod dump;
pub mod flash;
pub mod fspath;
pub mod gpu;
pub mod hw;
pub mod layout;
pub mod math;
pub mod mcp;
pub mod mounts;
pub mod open;
pub mod pick;
pub mod preview;
pub mod render;
pub mod search;
pub mod settings;
#[doc(hidden)]
pub mod testdir;
pub mod theme;
pub mod tree;
pub mod treemap;
pub mod update;
pub mod update_ui;
pub mod walk;
pub mod world;
