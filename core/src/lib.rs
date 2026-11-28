//! Clayform's sculpting engine. No interface or GPU code lives here, so it
//! compiles both natively (tests, Tauri) and to WebAssembly (the interface).

pub mod bake;
pub mod brush;
pub mod bvh;
pub mod camera;
pub mod dynamics;
pub mod dyntopo;
pub mod deform;
pub mod detail;
pub mod editor;
pub mod export;
pub mod fibers;
pub mod io;
pub mod mannequin;
pub mod mesh;
pub mod paint;
pub mod poly;
pub mod poly_ops;
pub mod print;
pub mod project;
pub mod remesh;
pub mod retopo;
pub mod scene;
pub mod shadowbox;
pub mod sculpt;
pub mod select;
pub mod snapshot;
pub mod symmetry;
pub mod text3d;
pub mod topo;
pub mod tube;
pub mod uv;
pub mod volume;
pub mod wire;
pub mod zspheres;

pub use glam;
