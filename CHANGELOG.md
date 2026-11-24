# Changelog

axross-files-matrix turns the data on your mountpoints into a 3D city you
fly through, Matrix style, with real file access.

## 0.0.9 - 2026-11-24

A preview for a first look, before 0.1.0: the state of "Unreleased - 0.1.0"
below as it stands today. It is published as a pre-release; the program's
own update check takes no pre-release, and 0.1.0 will be offered to it.

### Known in this preview
- **In flight the city is dimmer and hazier than standing still** (about
  half the contrast): the toning that keeps the flash guard out of ordinary
  flight is too strong. Being reworked for 0.1.0.
- **The flash guard does not see alternating flashes** (a pattern reversing
  in place, such as a checkerboard): flashes of the whole picture or of a
  part of it are held to the limit, and what the city itself draws measured
  0 to 2 transitions a second in flight. Being reworked for 0.1.0.
- **After axross's server has been started again**, a system that was open
  has to be closed and opened again.
- "Open in axross", the systems of axross's connection manager and its
  remote backends need axross 0.15.186 or newer, which is not released
  yet; local folders and every mountpoint work without axross.
- The Teleport menu's rows are not reached by the arrow keys.
- Frame rates were only measured on a software renderer so far:
  `axross-files-matrix --benchmark` prints them for a real GPU.
- macOS and Windows builds are built and tested by CI but were not tried
  by hand.

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
  A mount finds its disks through every layer: LVM on LUKS (device-mapper
  slaves down to the disk), MD RAID (all members), btrfs and ZFS pools
  (every device), loop devices. The island grows with the biggest city, so
  it is never a speck beside it. Bind mounts of what another island shows
  and bind-mounted files are left out (a setting shows the bind mounts).
- **The city**: folders are districts (a squarified treemap by the space
  files take on the disk - a sparse file is small; a setting sizes by
  length - with a least footprint for every file, and streets), files are
  buildings (footprint and height by size, colour by
  type, glow by how recently they changed), unread folders dashed
  placeholders, unreadable ones amber. Floating labels with level of
  detail. Digital rain in the sky, code running down the tallest towers,
  scanlines and bloom; edges smoothed with four samples a pixel (a
  setting). A colour legend in the view (L hides it); labels never
  overlap each other, an island's title or the legend.
- **An instanced wgpu renderer** inside egui's frame: one draw call for
  every building, an HDR scene, a bloom chain, the final picture; far
  districts collapse into single blocks and open as you approach (and
  unread folders are read then); a big folder's buildings are cut into
  tiles that collapse one by one; a hard budget of boxes a frame (160 000)
  holds for any tree. The layout, its instances, the cache's encoding and
  every island's move run off the window's thread; an island that does not
  move is not rebuilt.
- **Streaming walks**: one thread per island and a few readers, the city
  growing as entries arrive; symbolic links listed, never followed;
  permission errors marked; folders of any size read whole, in pieces;
  any depth; a limit of 8 million entries in all, said on the status line
  when reached; names that are no UTF-8 or hold newlines kept and shown
  escaped (`\xNN`). The mounts, the machine and the folders given are
  looked at on a thread of their own: a dead mount never keeps the window
  from coming. The last walk of each island is cached (checksummed, every
  field checked when read back; a folder and files only you can read;
  pruned by size and age, cleared from the settings; removable, network
  and remote islands only when a setting says so), so the city stands at
  once on the next start while the new walk runs - and stays, marked, when
  its folder cannot be read any more.
- **Flight**: free flight (WASD, Q/E, mouse look on the right button or
  captured with M, Shift faster, the wheel a speed factor), orbit and map
  (drag, pan, zoom, click a district to fly there), flights along paths
  that keep above every roof, walls you slide along. The speed grows with
  the height over the ground and the world's size (slow in a street, a
  third of the world a second high up); it eases in and out (no dead stop)
  and moves in steps of at most 2 m, so a fast flight never passes through
  a wall or the ground; orbit zooms out to three world widths. Home flies
  back to the first view; Z (or the inspect panel's *Frame island*) shows
  the chosen island whole and keeps it framed while it grows.
- **Teleport to other systems**: the Teleport menu lists every connection
  saved in axross's connection manager (MCP `list_remote_profiles`) - name,
  protocol, host and its state: not open, connecting, walking, open,
  failed with the reason - and a filter that searches systems, islands,
  districts, spots and hits at once. Teleporting to one connects it through
  axross's server (`open_remote_profile`, read-only, the `remote` tool
  group; Sources shows the command line), raises its island at once as
  "connecting…", streams its walk and flies there; a time limit and a
  cancel for a hung host; a failure stays on the island and in the menu
  with a retry. Every protocol axross has a backend for works - the list
  comes from axross's registry at run time; families (shell logins,
  shares, cloud stores, phones, mail, databases, version control) have
  their sign and colour, any other protocol the generic look. A backend
  that tells no sizes gets equal footprints and says so; mail stores,
  databases and slow backends are read folder by folder as they are
  approached. A saved spot inside a system opens it again. The matrix
  never asks for or stores a secret: a system that needs one says "needs
  unlocking in axross" and opens axross's connection manager.
- **Systems can be closed** (the menu's *close*; it frees one of axross's
  open sessions). A connect that is cancelled or runs past its time limit
  is given up on the server at that moment (`close_remote_profile`: with
  axross 0.15.186 its slot is free at once, and a retry is not told it is
  "being connected already"); a late success is closed again when it comes.
  A server that ends is noticed: the window says so, keeps the systems'
  last list marked as old, and starts the server again by itself (after 2,
  4, 8 ... up to 60 seconds, five times; Sources' *try again* any time). When axross's server holds as
  many systems as it may (8), the window says which are open and offers
  to close the one visited longest ago. The Teleport menu costs well under
  2 ms a frame with thousands of systems and half a million hits: its
  rows are made once, only those in sight are laid out, they never move
  under the mouse; Esc closes it first.
- **Teleport**: a menu of the islands, their largest districts, the saved
  spots and every search hit (paged); 1-9 jump to saved spots, Ctrl+1-9 save them;
  a short warp, at most two teleports a second.
- **Search** by name (any part, any case) or glob (`*`, `?`, `[a-z]`)
  across everything walked, a few milliseconds a frame; every hit is kept;
  the first 400 are pillars of light (said); N flies to the next.
- **Inspect and preview**: picking by a ray against a bounding volume
  hierarchy; the path, size, times, owner and mode, type; a preview of text
  (the first 64 KB and 2000 lines, control characters escaped), images
  (at most 128 MB to decode, refused from the header before), a hex dump;
  one preview at a time, the newest asked for; files opened without
  following a link or waiting on a pipe; paths and link targets escaped;
  times in local time; remote files through MCP `read_file` with a byte
  limit, by their exact bytes.
- **Open** in axross (`axross -- PATH`, `axross --profile=NAME -- PATH`
  for a remote backend's saved profile - axross 0.15.186 and later, found
  by asking its `--help` once; an older one is said to be too old, nothing
  falls through), in the file manager (runs nothing), or with the system's
  default program - programs, scripts, desktop entries, shortcuts,
  executables, links and anything on removable or network media only after
  a question that says what may happen. Never through a shell; a path never
  read as an option.
- **Read-only**: nothing on any disk is changed; the write mode is
  visibly off; "read-only or write?" before the flight can be asked (only
  read-only can be chosen in this release).
- **Settings** in the configuration folder, read robustly (a file that is
  no JSON is set aside, values out of range are brought back).
- **Small windows**: below 1000 points the bar's buttons go into a menu,
  the READ-ONLY badge always stays; the status line is cut short with the
  rest in its tooltip; every window scrolls. Keys work with a button in
  focus; the mouse is let go when the window loses focus.
- **An idle window rests**: nothing is drawn while hidden or minimised;
  when nothing moves only the rain asks for frames (8 a second, a setting;
  0 stills it), one a second unfocused.
- **Every folder is read once**: a big folder streaming in pieces or pages
  is never read again when the window asks for it meanwhile (it was
  counted twice). Folders past the longest path a system takes are read
  folder by folder (`openat`).
- **Answers read into typed, bounded shapes**: lists cut at their bound
  and the rest skipped unbuilt, strings cut, wrong types counted as
  missing; no answer is held as a generic JSON tree (four million tiny
  objects cost about their own size). Every string a server sends is cut
  and escaped; a saved connection whose name is blank or holds control,
  invisible or bidirectional characters is left out (and counted). Calls
  without file data take answers of at most 8 MiB. The server runs under
  a guard of its process group: its own children end with it, also when
  this program is killed.
- **A preview never waits on a dead file**: each on a thread of its own,
  one that does not answer is left behind after five seconds; a request
  that finds three threads busy for 300 ms gets a fresh one.
- **The Open guard is an allow-list of everyday types**: text, source
  code, configuration, documents (the old Office ones too), archives,
  pictures, sound, video and PDFs open without a question (README,
  LICENSE, Makefile and the like as well) - when their first bytes agree:
  no program, script or desktop entry, no web page under a picture's name,
  no zip under a picture's or a text's, no old Office file (macros) under
  a docx's. A type nobody listed asks, says it is unknown, and offers "do
  not ask again for this type" (Settings › Opening asks again). Programs,
  scripts, shortcuts, links and special files (a pipe, a socket, a
  device) always ask.
- **Every script drawn**: fonts for Chinese, Japanese, Korean, Arabic,
  Hebrew, Indic scripts, Thai and symbols are found among the system's on
  a thread of its own (the window never waits for the scan) and added as
  fallbacks; a character no font has is a box, never a stop. Each font is
  checked whole before the window gets it - read by egui's parser, laid
  out, and every glyph drawn once (unhinted and hinted) within a time
  budget, inside `catch_unwind`; one with a hole is left out and said. A
  frame that still fails with a font drops the fonts found for egui's own,
  says so and goes on.
- **The far view**: the first view and Home look across the biggest city
  15 degrees down, the island whole in the window (2 million files too),
  the horizon and the rain in its upper third; the Overview button keeps
  the map. A far district keeps a skyline: its block is as high as the
  90th percentile of its buildings (never their mean) and its five
  tallest still stand as towers; far roofs are raised gently with the
  distance (1 near, growing with its log from 300 m, about a tenth of the
  city's width in relief); far faces carry lit specks of windows, far
  blocks stand back from their plates so the streets stay lit lines of at
  least a pixel and a half, a green glow along the horizon, rain mirrored
  in the far water, fog tinted green-black that lies low (towers rise out
  of it) and never takes more than 72 % of a city's contrast; ground grids
  and faces smaller than a pixel fade to their average (no shimmer). No
  blown-out specks (bright points far away are pressed down). Search hits
  are tinted beams a few pixels wide at least, with a ring on the ground;
  the teleport's warp is strongest at the cut and fades. A connecting
  island's outline breathes, a failed one is hatched amber with its reason
  on a line of its own, a system's empty plot carries its protocol written
  large, an empty folder says so. Near the machine's island the legend
  names its parts, and their labels are brighter.
- **Truly idle**: the machine's load wakes the window only for a light
  that moves visibly, and not at all while it rests (hidden, unfocused,
  no frames while idle).
- **Settings**: a value of the wrong type resets that value alone (inside
  a nested object or a list too: `mcp.enabled`, `bookmarks[2]`, an item of
  a list of plain values), the file as it was is kept as
  `settings.json.bak` (an earlier copy never written over) and the window
  says so, naming a few; a byte order mark reads; a list is salvaged to
  its first 64 items (a huge broken file costs no time); Settings measures
  the walk cache when it opens; the file only the user can read (0600 in a
  0700 folder).
- **First start and empty states**: a one-line key hint until the first
  key moves the camera (closable; the Help window keeps it); Help and
  Settings open tall enough to read; with no folder at all the view says
  how to add one. Sources adds and removes folders and stops a walk
  without a restart; Teleport opens at the left above the legend, Sources
  and Settings at the right. Counts say "1 island", "1 entry".
- **axross's server, hardened**: answers longer than 32 MiB are read past
  in bounded memory and fail their call at once, as does an answer that is
  no JSON; three such and the server is ended; names with a slash, a
  dot-dot or a control character are refused (and counted); the server is
  killed and reaped at the end with its process group, a call still
  waiting included (Linux: also when the program dies).
- **What is drawn is what is picked**: the far roofs raised by the shader,
  the towers over far blocks and the far towers widened to two pixels are
  picked, highlighted and labelled as drawn (one function, shared by the
  shader and the window, with a test over raised towers at 200 000 and 2
  million files); a slim far tower is raised at most twelve of its widths.
- **The camera is never shut in**: an eye inside a building is lifted out
  over its roof (on a switch of mode, a zoom into a roof); the orbit's
  pivot is always on the ground; keys that move nothing say why.
- **Open, closer**: a folder that is a program (a macOS bundle, a Linux
  AppDir, a Windows shortcut folder) always asks; a web page or Java Web
  Start file under a name the desktop opens by content (`LICENSE`, `x.lst`,
  XHTML in `.xml`), PostScript in a `.pdf`, and types that run (`xlsb`,
  `pyc`, `ahk`, `rdp`, ...) always ask, never "trusted for good"; the file
  is looked at again right before it is opened and not opened if it was
  swapped (another file or a link in its place); a check that hangs on a
  dead disk is said after a second and a half.
- **Keyboard and windows**: T puts the keyboard in the Teleport filter (a
  count of what matches beside it); Esc closes the menu, then Help,
  Settings and Sources; Settings and Sources stay in the 3D view's area,
  off the inspect panel; the wheel's speed is said in the view for a
  moment; the first-start hint goes for a mouse user too.
- **Sources**: folders are made whole (`~`, links, `..`) and compared so:
  one island a folder, none inside another; a file is no island; added
  folders are islands at every start until removed; removing an island
  keeps the choice on another; thousands of systems cost no more than a
  few (only the rows in sight). A chain of folders thousands deep is read
  from the nearest open folder above (a call a folder, not one per level
  above it); `--dump-city` says how far its walk got.
- **Previews** of Latin-1 and UTF-16 text show the text, not hex.
- **Performance, measured**: `--benchmark` says where the CPU's time goes
  (level of detail, instances, encoding and upload), how the city was made
  (walk, layout, picking boxes, instances), the peak memory, and takes
  `--path spiral|fly|orbit|teleport`; `tests/perf.rs` guards the window's
  work a frame and its parts, allocations, the first city cold and cached,
  walks and the walk cache (docs/PERFORMANCE.md). The picking boxes'
  hierarchy is built in half the time; the level of detail tests a
  district's raised box only when it is needed and none below a district
  wholly in view; picking under a still pointer and view is not cast again.
- **The Matrix theme**, the theme system ready for more.
- **A flash guard** as axross-amp's: the picture shown is measured on a
  grid on the GPU (each cell its area's mean, 256 taps) and read back;
  where a field flashes more than three times a second - by the net share
  of its cells turning one way: lit things moving across a field brighten
  as much as they darken and are no flash - it is veiled with soft edges
  towards its own slow average, dimmed by at most 40 %, never black. While
  the view moves fast, far window specks, edge glow, the rain and windows a
  hand's breadth away are toned down, so ordinary flight never needs it:
  flying, boosting, a street, a next-hit flight, teleports, orbit and Home
  are held to no veiled frame at 30 and 60 frames a second at 20 000,
  200 000 and 2 million files, and a strobe is still caught and veiled. The
  frames are held to an independent meter in the tests.
- **The GPU**: the dedicated one preferred (*Settings › GPU*: automatic,
  high performance, low power), never a software renderer while a GPU is
  there; `WGPU_ADAPTER_NAME`, `WGPU_BACKEND`, `WGPU_POWER_PREF` override
  it. The status line names the API and the adapter, and says calmly when
  it is a software renderer.
- **`--benchmark [FILES]`**: a synthetic city (200 000 files) flown through
  offscreen with no vsync - the adapter, boxes and draw calls per frame, the
  CPU side and the whole frame as median, p95 and p99, as text or JSON;
  `--flat` puts the files in one folder, `--no-msaa` draws without smooth
  edges.
- **Headless**: `--version`, `--self-test`, `--dump-city DIR [--json]`
  (names exact in JSON, with their bytes as base64 when they are no UTF-8).
  `--` ends the options: after it every argument is a folder.
- **Signed updates** from the GitHub releases (as axross-amp's), checked
  against the pinned release key (`scripts/sign_release.py keygen` refuses
  a path inside a checkout or a folder others may read; `*.pem` and
  `*.key` are ignored by git); CI with the supply-chain gate and the
  release binary's `--self-test`,
  tests and builds on Linux, macOS and Windows; releases with an AppImage,
  a DMG and a draft for signing.
