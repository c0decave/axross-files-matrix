# Performance

Back to the [README](../README.md).

What the city costs, how it is measured, and what a performance round
changed. Every number here was measured on one machine: no GPU (Mesa
llvmpipe, a software renderer), 8 cores shared with other work - so the
CPU-side numbers are the ones to compare; whole-frame times on a real GPU
are a matter for that GPU.

## How it is measured

- `axross-files-matrix --benchmark FILES [--path spiral|fly|orbit|teleport]
  [--all] [--size WxH] [--frames N] [--json]` - offscreen, no window, no
  vsync. It prints the city's making (the walk, the layout, the picking
  boxes and their hierarchy, the instances), the boxes a frame, the CPU
  side of a frame (median, p95, p99) and where it goes (choosing what is
  drawn - the level of detail -, handing the instances over, encoding the
  passes and uploading the instances), the whole frame waited for, and
  the process's peak memory. Run it on a real GPU to compare.
- `cargo test --release --test perf -- --nocapture` (and `AFM_BIG=1` for
  two million files): the window's own work a frame while it flies (its
  parts: picking under the pointer, the level of detail and the instances,
  the labels), allocations a frame, the time to the first city cold and
  from the walk cache, walks (entries a second, local and through the test
  MCP server), the walk cache written, read and its size. Each has a limit
  several times what is measured here - a guard against getting much
  worse, median-based.
- `--frame-stats` in the window: frame times once a second, and where a
  slow frame's time went.
- `tests/flash.rs`, `tests/menu_cost.rs`: the flash guard in ordinary use
  (no veiled frame), the Teleport menu's cost with 2000 systems and half a
  million hits.

## The performance round

Measured first (the benchmark at 20 000, 200 000 and 2 million files on
four camera paths, 640 x 360, release build; the window's parts in the
test profile), changed what the numbers showed, measured again - the two
binaries run alternately on the same machine.

| Measure (release, CPU side) | Before | After |
|---|---|---|
| 2M: the picking boxes' hierarchy built | 0.63 - 0.79 s | 0.32 - 0.34 s |
| 2M: the city made (walk, layout, boxes, instances) | 1.49 - 1.97 s | 1.19 - 1.23 s |
| 200k: the picking boxes' hierarchy built | 0.05 - 0.06 s | 0.02 s |
| 2M teleport path: level of detail a frame (median) | 1.14 ms | 0.96 ms |
| 2M fly path: level of detail a frame | 0.98 ms | 0.89 ms |
| 200k spiral: level of detail a frame | 0.45 ms | 0.41 ms |
| 200k teleport: CPU side a frame | 0.77 ms | 0.68 ms |
| 2M teleport: CPU side a frame | 1.53 ms | 1.42 ms |
| 200k, every building drawn (`--all`): CPU side | 1.60 ms | 1.48 ms |
| Boxes a frame, every path and size | unchanged | unchanged |

What changed:

- **The picking boxes' hierarchy** compared each box's middle by
  computing it again at every comparison of every split; now each middle is
  computed once. Half the time at 2 million files.
- **The level of detail** raised a district's culling box (a logarithm
  and a square root) for every district; now only for one the view's
  frustum misses as it is (most are in it). And below a district wholly
  inside the frustum no district is tested at all (a hierarchical bound).
  8 - 12 % of the level of detail.
- **Picking under the pointer** is not cast again while the pointer, the
  camera and the world are the same (a still view costs no ray).
- **Labels**: the ray that finds whether a label is behind a building is
  cast only for a candidate in the view (before: for every one).

The window's own work a frame while flying (test profile, medians; this
machine was busy and these vary by 20 % from run to run):

| Files | Work a frame | of it: picking | level of detail | labels | allocations |
|---|---|---|---|---|---|
| 20 000 | 0.43 ms | 0.04 ms | 0.10 ms | 0.12 - 0.15 ms | 780 - 800 |
| 200 000 | 0.92 ms | 0.22 ms | 0.24 - 0.27 ms | 0.26 - 0.29 ms | 1070 - 1100 |
| 2 000 000 | 1.8 - 2.7 ms | 0.31 ms | 0.30 - 0.35 ms | 0.8 - 1.2 ms | 1290 - 1370 |

Other measures (test profile):

| Measure | Value |
|---|---|
| Walk, local tree (20 000 files in 200 folders) | 0.2 - 0.6 million entries a second |
| Walk through the test MCP server (60 000 entries, pages of 6000, 80 ms a page) | 13 000 - 21 000 entries a second (the server's pause the most of it) |
| Walk cache of 200 000 files: written / read / size | 10 - 11 ms / 36 - 95 ms / 5.7 MiB |
| First city on the screen, 200 000 synthetic files: cold / from the cache | 0.85 - 0.95 s / 0.88 - 0.91 s |
| Peak memory (benchmark, release): 20k / 200k / 2M | 170 - 190 / 250 - 330 / 730 - 920 MiB |
| Idle (`idle_fps` 0, nothing moving) | no frames, 0 - 1 CPU ticks in 10 s |

## Tried, and not worth it (yet)

- The level of detail's "wholly inside" bound pays at 2 million files
  (about 9 %) and is within the noise at 200 000.
- Testing the label's occlusion last (only for labels that would be
  drawn) saved little time and tripled the allocations a frame (every
  candidate's text laid out): kept as it is now, a ray for a candidate in
  the view.
- The walk cache from the start does not show a synthetic city sooner
  (made in memory, it is as quick as its cache); it pays for real folders
  on real disks, which this machine cannot measure fairly.

## What needs a real GPU to judge

- The whole frame: here llvmpipe draws a frame in 75 - 750 ms; on a GPU the
  CPU side (under 1.5 ms at 2 million files) is what is left, and the
  pixel shading (bloom, the far view's specks and fog, the flash guard's
  measure) decides the rest.
- Persistent instance buffers with partial updates, smaller instance
  records, fewer faces for far boxes, a quarter-size bloom: their worth is
  GPU time and bus bandwidth, not measurable on a software renderer.
- GPU-side culling (compute, indirect draws): the CPU's level of detail is
  under 1 ms at 2 million files; worth it only where that matters.
