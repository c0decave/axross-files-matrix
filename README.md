# axross-files-matrix

The data on your mountpoints as a 3D city you fly through - Matrix style,
with real file access. Written in Rust (wgpu and egui), a companion of
[axross](https://github.com/c0decave/axross).

![Flying over a city of 200 000 files under the digital rain](docs/matrix.png)

- **Every mountpoint is an island** - the local disks, every mount the
  system shows (pseudo file systems hidden), and, when axross is installed,
  every remote backend of its MCP server (SFTP, S3, WebDAV, ...)
- **Folders are districts, files are buildings** - a squarified treemap by
  bytes with streets between the districts; a building's footprint and
  height by its size, its colour by its type, its glow by how recently it
  changed
- **The machine is an island too** - the mainboard as the ground plan, the
  processor's cores as towers lit by their load, the memory's modules in
  their slots, the PCIe cards, the disks - and a beam of light from each
  disk to the islands that live on it
- **Huge trees** - the walk streams in the background and the city grows
  while you watch; far districts are single blocks that open as you come
  near (unread folders are read then); the last walk is cached, so the city
  stands at once on the next start
- **Fly** - free flight with WASD and the mouse, or orbit and map; flights
  that keep above the roofs; teleport to islands, districts, saved spots
  (1-9) and search hits
- **Find, inspect, open** - search by name or glob, hits as pillars of
  light; details and a preview of text, images and bytes; open in axross or
  with the system's program
- **Read-only** - nothing on any disk is changed in this release (file
  operations with pick-up-and-fly come later)
- **Guarded against flashes** - the picture is measured on the GPU and
  veiled where it would flash more than three times a second, as in
  [axross-amp](https://github.com/c0decave/axross-amp)
  ([Photosensitivity](docs/FLASH_SAFETY.md))

> **Photosensitivity warning.** The city moves: digital rain, flights, the
> teleport's warp. The picture is guarded against flashes (WCAG 2.3.1 and
> 2.3.2: at most three flashes a second, no saturated red flashes), but it
> is not guaranteed safe for everyone. *Settings › Reduce motion* stops the
> rain and the warp. If you or anyone watching has epilepsy or feels
> unwell, stop.

## Screenshots

<table>
  <tr>
    <td><img src="docs/city-200k.png" alt="A city of 200 000 files from above"></td>
    <td><img src="docs/street.png" alt="Among the towers"></td>
  </tr>
  <tr>
    <td><img src="docs/city-small.png" alt="A small city beside the machine's island"></td>
    <td><img src="docs/hardware.png" alt="The machine's island: cores, memory, cards, disks"></td>
  </tr>
  <tr>
    <td><img src="docs/search.png" alt="Search hits as pillars of light"></td>
    <td><img src="docs/preview.png" alt="The inspect panel with a preview"></td>
  </tr>
  <tr>
    <td><img src="docs/teleport.png" alt="The teleport menu"></td>
    <td></td>
  </tr>
</table>

<sub>The cities in the pictures are made-up trees (`--synthetic`), the
machine a made-up one; the pictures are rendered by the tests
(`AFM_SHOTS=docs cargo test --test screenshots`).</sub>

## Install

Each [release](https://github.com/c0decave/axross-files-matrix/releases)
has the files and their `SHA256SUMS` (signed by the maintainer):

| System | File |
|---|---|
| Linux x86_64 | `axross-files-matrix-X.Y.Z-x86_64.AppImage`, or the `.tar.gz` (glibc 2.34 or later) |
| macOS (Apple silicon and Intel) | `axross-files-matrix-X.Y.Z-macos-universal.dmg` |
| Windows x64 | `axross-files-matrix-X.Y.Z-windows-x64.zip` |

Or with cargo: `cargo install axross-files-matrix`.

It needs a GPU that Vulkan, Metal, DirectX 12 or OpenGL can reach (a
software renderer works, slowly).

For remote backends install axross's MCP server -
`pip install axross-headless-mcp` (or run it once with
`uvx axross-headless-mcp`) - or axross itself; the city finds it on the
PATH. Without it: local disks and mounts only.

## GPU

The city is drawn by [wgpu](https://wgpu.rs) - Vulkan, Metal, DirectX 12
or OpenGL, whichever the system has; no vendor's own toolkit is needed.
The dedicated GPU is preferred (*Settings › GPU*: automatic, high
performance, low power - from the next start). The status line says what
draws the city (`Vulkan · <adapter>`), and says so calmly when it is a
software renderer (llvmpipe, lavapipe, WARP): that works, a few frames a
second.

Overrides, for a start from a terminal:

| Variable | |
|---|---|
| `WGPU_BACKEND` | the API: `vulkan`, `metal`, `dx12`, `gl` |
| `WGPU_ADAPTER_NAME` | a part of the adapter's name, e.g. `nvidia`, `radeon`, `intel` |
| `WGPU_POWER_PREF` | `high` or `low` (when the setting is automatic) |

`axross-files-matrix --benchmark` measures it on your machine: a city of
200 000 files flown through for 600 frames offscreen (no vsync), with the
adapter, the boxes and draw calls per frame, and the CPU side and the whole
frame as median, p95 and p99 (`--json` for the numbers, `--all` to draw
every building with no level of detail, `--size 3840x2160`).

## Use

```sh
axross-files-matrix                 # every mountpoint, axross's backends, the machine
axross-files-matrix ~/projects /srv # islands of these folders only
axross-files-matrix --synthetic 200000   # a made-up city, to look at
axross-files-matrix --dump-city DIR --json   # the city as JSON, no window
```

`--help` lists every option. Started from a terminal it goes to the
background (`--foreground` keeps it there).

### Controls

| Key / mouse | |
|---|---|
| W A S D (arrows) | fly forward, left, back, right |
| Q / E | down / up |
| Shift | five times as fast |
| wheel | flight speed; in orbit: zoom |
| right drag | look around (fly) / turn (orbit) |
| M | capture the mouse to look (Esc lets go) |
| F | free flight / orbit and map |
| drag, Shift+drag | orbit: turn, pan |
| click | choose (orbit: and fly there); double-click: fly there |
| / or Ctrl+F | search (text, or a glob like `*.iso`) |
| N / Enter | fly to the next hit (Shift: teleport) |
| T | teleport menu: islands, districts, spots, hits |
| 1 - 9, Ctrl+1 - 9 | jump to a saved spot, save one |
| Home | overview |
| O / Shift+O | open in axross / with the default program |
| H / F1 | help |

### What you see

- **Islands**: the machine in the middle, the local mounts in a ring round
  it, axross's remote backends further out (bluish). The beams run from a
  disk on the machine's island to the islands on it.
- **Buildings**: colour by type - source code bright green, documents pale,
  images teal, video lime, audio mint, archives gold, programs deep green,
  data blue, configuration soft green. Recently changed files glow and
  have more lit windows.
- **Blocks**: a far district drawn as one block in the colour of what fills
  it most. Dashed outlines: a folder not read yet. Amber outlines: a folder
  that could not be read.

### Settings

In the configuration folder (`~/.config/axross-files-matrix/settings.json`
on Linux): read-only (the only mode in this release), the question before
the flight, pseudo file systems, the machine's island, axross's server and
its command, the look (labels, rain, bloom, scanlines, reduce motion), the
mouse, updates. A file that cannot be read is set aside, not fatal. The
walk cache lives in the cache folder (`~/.cache/axross-files-matrix/`).

## Building

```sh
cargo build --release
cargo test
cargo run --release -- --benchmark    # frame times on this machine's GPU
cargo run --release --example bench   # also the walk, the layout and the memory
```

What the bench measures with 200 000 files: the city is laid out in about
30 ms on its own thread; the level of detail picks 25 000 - 60 000 boxes a
frame in 0.1 - 2 ms (all 200 000 buildings opened: about 1 ms); every box
is one instance of a single draw call. Walking a real tree of 200 000
files (warm, four readers) takes about 0.35 s. The window keeps its own
work per frame near 1 ms; the frame rate then depends on the GPU (a
software renderer - Mesa's llvmpipe - needs 0.2 s for a 1080p frame).

Rust 1.95 or later (what egui 0.37 and wgpu 30 need). Linux needs nothing beyond a C compiler (for `ring`);
the tests render with any wgpu adapter (Mesa's lavapipe on CI).

Releasing: [docs/RELEASING.md](docs/RELEASING.md).

## Licence

Apache-2.0 ([LICENSE](LICENSE)); what it is built from:
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
