//! Clayform's sculpting engine. No interface or GPU code lives here, so it
//! compiles both natively (tests, Tauri) and to WebAssembly (the interface).

pub mod bvh;
pub mod camera;
pub mod dyntopo;
pub mod editor;
pub mod export;
pub mod io;
pub mod mannequin;
pub mod mesh;
pub mod poly;
pub mod print;
pub mod project;
pub mod remesh;
pub mod retopo;
pub mod sculpt;
pub mod snapshot;
pub mod symmetry;
pub mod wire;

pub use glam;
