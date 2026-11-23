# Clayform

A digital sculpting app in Rust inspired by [SculptGL](https://stephaneginier.com/sculptgl/): a
[Tauri](https://tauri.app) desktop app with a [Leptos](https://leptos.dev) interface.

![Clayform](docs/screenshot.png)

## Interface

Like SculptGL, the functions are grouped into dropdown areas:

- **Top-bar menus:** **File** (import, export OBJ/STL), **Edit** (undo, redo), **Scene** (new sphere, cube or plane,
  sphere detail), **Camera** (frame, reset, front/back/left/right/top/bottom views, field of view), **Background**
  (Dark gradient, Studio grey, Light, Black) and **Help** (controls).
- **Side-panel folders** (click a title to fold it): **Sculpting** (tool dropdown, radius, intensity, negative,
  symmetry), **Masking** (mask brush, clear, invert, show mask), **Topology** (subdivide, mesh statistics),
  **Rendering** (material dropdown, flat shading, wireframe) and **Interface** (panel side).
- The side panel docks on the **left or right** of the viewport. Use the **Panel left / Panel right** button in the
  top bar, or **Interface › Panel side**. Your choice is remembered.

## Features

- **Brushes:** Draw, Clay, Smooth, Flatten, Inflate, Pinch, Crease, Grab and Mask, each with its own radius and
  intensity. Hold Ctrl to invert a brush and Shift to smooth.
- **X symmetry**, with a second cursor showing where the mirrored stroke lands.
- **Masking**, **Loop subdivision** (up to 6M triangles) and **undo/redo** for every stroke and mesh change.
- **Import/export** OBJ and STL. Imported meshes are rescaled for sculpting and exported at their original scale.
- **Viewport:** WebGL 2 through wgpu, 4× MSAA, six procedural matcaps, wireframe and flat shading.

## Controls

| Input | Action |
|---|---|
| Left drag on mesh | Sculpt |
| Shift + drag | Smooth |
| Ctrl + drag | Invert brush |
| Left drag on background, Alt + left, right drag | Rotate |
| Middle drag, Shift + right drag | Pan |
| Wheel | Zoom |
| `D C S F I P E G M` | Draw, Clay, Smooth, Flatten, Inflate, Pinch, Crease, Grab, Mask |
| `[` `]` | Brush radius |
| `X` / `W` / `N` | Toggle symmetry / wireframe / negative |
| Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo / redo |
| Ctrl+D | Subdivide |
| Ctrl+O / Ctrl+S | Import / export OBJ |
| Home | Frame mesh |
| `1` `3` `7` (Ctrl for the opposite side) | Front, right, top views |

## Project layout

| Folder | What it is |
|---|---|
| `core/` | **clayform-core**: the engine. Mesh, brushes, undo, BVH picking, camera, editor input logic, OBJ/STL. No interface code. |
| `app/` | **clayform-app**: the Leptos interface and wgpu (WebGL 2) viewport, compiled to WebAssembly together with the engine, so strokes never wait on the desktop backend. |
| `src-tauri/` | **clayform**: the Tauri desktop shell. It opens the window and provides native Open/Save dialogs. |

## Run

    rustup target add wasm32-unknown-unknown
    cargo install trunk
    npx @tauri-apps/cli@2 dev        # live-reloading window
    npx @tauri-apps/cli@2 build      # target/release/clayform.exe and an installer

The interface also runs in a browser with WebGL 2 (`cd app && trunk serve`, then open http://localhost:1420). There,
Import and Export use the browser's file picker and downloads.

Tests: `cargo test` (engine: meshes, subdivision, picking, brushes, strokes, undo, symmetry, OBJ/STL).

## Windows build

- **From Linux without GitHub:** `./build-windows.sh` produces `dist/Clayform-setup.exe` (installer) and
  `dist/Clayform-portable.zip` (Clayform.exe + WebView2Loader.dll). If rustup can't download the WebAssembly and
  Windows targets, the script builds Rust's standard library from source instead.
- **GitHub Actions:** every push to `main` is built on Windows. Publishing a release with a `v*` tag attaches
  `Clayform.exe` and `Clayform-setup.exe` to it.

Clayform uses Microsoft Edge WebView2, which Windows 10 and 11 include. The installer adds it if a PC lacks it.

## Roadmap ideas

Dynamic topology (Sculptris-style local remeshing), multiresolution levels, voxel remesh, alpha/stamp textures, layers, vertex painting, and a lasso mask.
