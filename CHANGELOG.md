# Changelog

axross-files-matrix turns the data on your mountpoints into a 3D city you
fly through, Matrix style, with real file access.

## Unreleased - 0.1.0

The first release: the MVP of the spec.

### Added
- **Islands**: the local file system and every mountpoint the system shows
  (Linux `/proc/self/mountinfo`, macOS `/` and `/Volumes`, Windows's
  logical drives); pseudo file systems (proc, sysfs, cgroup, tmpfs but
  `/tmp`, overlay but `/`, squashfs, ...) hidden unless the setting shows
  them; network file systems read lazily. Folders given on the command line
  are islands of their own instead.
- **axross's remote backends** through its MCP server
  (`axross-headless-mcp` on the PATH, `axross --mcp-server`,
  `uvx --offline axross-headless-mcp`, or a command in the settings):
  `list_backends`, a shallow `walk_tree`, then `list_directory` folder by
  folder, `get_path_info` and `read_file` for the inspector - read-only;
  the former tool names of older servers work too; saved remote profiles
  can be opened from the Sources window. Without axross: local disks and
  mounts only, and no message about it.
- **The machine's island**: the mainboard as its ground plan, the
  processor's cores as towers lit by their load, the memory's slots and
  modules (from SMBIOS or EDAC where readable without root) lit by how much
  is in use, the PCIe cards, the disks, the network and USB ports; each
  mountpoint's island tied to its disk by a beam of light. Linux reads
  `/proc`, `/sys` and DMI without root; missing data is left out quietly.
- **The city**: folders are districts (a squarified treemap by bytes, with
  streets), files are buildings (footprint and height by size, colour by
  type, glow by how recently they changed), unread folders dashed
  placeholders, unreadable ones amber. Floating labels with level of
  detail. Digital rain in the sky, code running down the tallest towers,
  scanlines and bloom.
- **An instanced wgpu renderer** inside egui's frame: one draw call for
  every building, an HDR scene, a bloom chain, the final picture; far
  districts collapse into single blocks and open as you approach (and
  unread folders are read then), with the level of detail adapting to the
  frame time.
- **Streaming walks**: one thread per island and a few readers, the city
  growing as entries arrive; symbolic links listed, never followed;
  permission errors marked; huge folders read up to a cap; names that are
  no UTF-8 or hold newlines kept and shown escaped. The last walk of each
  island is cached (checksummed, every field checked when read back), so
  the city stands at once on the next start while the new walk runs - and
  stays, marked, when its folder cannot be read any more.
- **Flight**: free flight (WASD, Q/E, mouse look on the right button or
  captured with M, Shift faster, the wheel sets the speed), orbit and map
  (drag, pan, zoom, click a district to fly there), flights along paths
  that keep above every roof, walls you slide along.
- **Teleport**: a menu of the islands, their largest districts, the saved
  spots and the search hits; 1-9 jump to saved spots, Ctrl+1-9 save them;
  a short warp, at most two teleports a second.
- **Search** by name (any part, any case) or glob (`*`, `?`, `[a-z]`)
  across everything walked, a few milliseconds a frame; hits are pillars of
  light; N flies to the next.
- **Inspect and preview**: picking by a ray against a bounding volume
  hierarchy; the path, size, times, owner and mode, type; a preview of text
  (the first 64 KB, control characters escaped), images (decoded on a
  thread of their own, scaled down) and a hex dump; remote files through
  MCP `read_file` with a byte limit.
- **Open** in axross (`axross <path>`, a remote profile's URL) or with the
  system's default program (`xdg-open`, `open`, Explorer) - never through
  a shell.
- **Read-only**: nothing on any disk is changed; the write mode is
  visibly off; "read-only or write?" before the flight can be asked (only
  read-only can be chosen in this release).
- **Settings** in the configuration folder, read robustly (a file that is
  no JSON is set aside, values out of range are brought back).
- **The Matrix theme**, the theme system ready for more.
- **A flash guard** as axross-amp's: the picture shown is measured on a
  grid on the GPU and read back; where a field would flash more than three
  times a second it is veiled towards its own slow average. The frames are
  held to an independent meter in the tests.
- **The GPU**: the dedicated one preferred (*Settings › GPU*: automatic,
  high performance, low power), never a software renderer while a GPU is
  there; `WGPU_ADAPTER_NAME`, `WGPU_BACKEND`, `WGPU_POWER_PREF` override
  it. The status line names the API and the adapter, and says calmly when
  it is a software renderer.
- **`--benchmark [FILES]`**: a synthetic city (200 000 files) flown through
  offscreen with no vsync - the adapter, boxes and draw calls per frame, the
  CPU side and the whole frame as median, p95 and p99, as text or JSON.
- **Headless**: `--version`, `--self-test`, `--dump-city DIR [--json]`.
- **Signed updates** from the GitHub releases (as axross-amp's), off until
  the maintainer pins the release key; CI with the supply-chain gate,
  tests and builds on Linux, macOS and Windows; releases with an AppImage,
  a DMG and a draft for signing.
