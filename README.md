# Clayform

A digital sculpting app in Rust inspired by [SculptGL](https://stephaneginier.com/sculptgl/), built with egui + wgpu.

![Clayform](docs/screenshot.png)

## Features (milestone 1)

- **Brushes:** Draw, Clay, Smooth, Flatten, Inflate, Pinch, Crease, Grab, Mask, each with a radius and intensity setting.
- **Modifiers:** hold Ctrl to invert the brush (dig instead of raise, or unmask), hold Shift to smooth.
- **X symmetry**, with a ghost cursor that shows where the mirrored dab lands.
- **Masking:** paint a mask to protect areas from sculpting, then clear or invert it.
- **Loop subdivision** for more detail (up to 6M triangles).
- **Undo/redo** for every stroke, subdivision and mesh change.
- **Primitives:** sphere (adjustable detail), cube and plane.
- **Import/export:** OBJ and STL (binary and ASCII). Imported meshes are normalised for sculpting and exported back at their original scale and position.
- **Viewport:** 4× MSAA, procedural matcaps (Clay, Red wax, Pearl, Jade, Chrome, Normals), wireframe and flat shading.

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

You can also open a mesh from the command line: `clayform model.obj`

## Build

```
cargo run --release
```

The GitHub Actions workflow builds `clayform.exe` on Windows and uploads it as a build artifact. If you push a `v*` tag, it also attaches the .exe to a GitHub release.

## Architecture

| File | Purpose |
|---|---|
| `src/mesh.rs` | Indexed triangle mesh with CSR adjacency, normals, mask, Loop subdivision and primitives |
| `src/bvh.rs` | Triangle BVH for ray picking, refitted once per frame after sculpting |
| `src/sculpt.rs` | Brush engine (flood-fill region, falloff, tools), strokes, grab, undo/redo |
| `src/render.rs` + `shader.wgsl` | Offscreen wgpu renderer (MSAA, matcaps, wireframe), shown in egui as a native texture |
| `src/camera.rs` | Orbit camera, picking rays and screen-to-world brush radius |
| `src/app.rs` | egui UI, input handling, shortcuts |

## Roadmap ideas

Dynamic topology (Sculptris-style local remeshing), multiresolution levels, voxel remesh, alpha/stamp textures, layers, vertex painting, and a lasso mask.
