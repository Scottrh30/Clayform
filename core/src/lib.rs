//! Clayform's sculpting engine. No interface or GPU code lives here, so it
//! compiles both natively (tests, Tauri) and to WebAssembly (the interface).

pub mod bvh;
pub mod camera;
pub mod dyntopo;
pub mod editor;
pub mod io;
pub mod mesh;
pub mod project;
pub mod remesh;
pub mod sculpt;
pub mod symmetry;
pub mod wire;

pub use glam;
