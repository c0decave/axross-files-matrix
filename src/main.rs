//! axross-files-matrix: the data on your mountpoints as a 3D city you fly
//! through, Matrix style - `axross-files-matrix [FOLDER ...]`.
//!
//! Every mountpoint is an island (and every backend of axross's MCP
//! server, when axross is installed), folders are districts, files are
//! buildings; the machine itself is an island in the middle. Read-only.

// (a GUI program on Windows: no console window opened beside it; started in
// a terminal it attaches to that one for --help and what it says)
#![cfg_attr(windows, windows_subsystem = "windows")]

use axross_files_matrix::{app, dump, update};
use std::path::PathBuf;

const USAGE: &str = "axross-files-matrix [OPTIONS] [--] [FOLDER ...]
       axross-files-matrix --dump-city FOLDER [--json] [--depth N]
       axross-files-matrix --benchmark [FILES] [--frames N] [--size WxH] [--all] [--flat] [--no-msaa] [--json]
       axross-files-matrix --help | --version | --self-test

Without folders every mountpoint the system shows is an island (pseudo file
systems hidden), with axross's remote backends when axross-headless-mcp or
axross is installed, and the machine's own island in the middle. With
folders: islands of those alone. After `--` everything is a folder, even
a name that starts with a dash.

  --synthetic N     a made-up city of N files (to look at, to measure)
  --no-mcp          do not look for axross's MCP server
  --no-hardware     no island of the machine
  --no-cache        neither read nor write the walk cache
  --frame-stats     times of the frames drawn to stderr, at most once a
                    second (it makes no frames itself: an idle window
                    reports nothing until something moves)
  --fixed-lod       no adaptive level of detail (measurements)
  --foreground, -F  stay in the terminal (else it goes to the background)
  --dump-city DIR   walk DIR, lay it out, print the city (text, or --json),
                    down to --depth N levels (default 3); no window
  --benchmark [N]   a city of N files (default 200000) flown through for
                    --frames frames (600) at --size (1920x1080), offscreen,
                    no vsync: the adapter, boxes and draw calls per frame,
                    CPU and whole-frame times (median, p95, p99); --all
                    draws every building (no level of detail), --flat
                    puts the files in one folder, --no-msaa draws without
                    smooth edges; --json

The GPU: a dedicated one by default (Settings > GPU); WGPU_ADAPTER_NAME
picks one by a part of its name, WGPU_BACKEND the API (vulkan, metal,
dx12, gl), WGPU_POWER_PREF (high, low) the preference.

Keys: W A S D fly (faster the higher), Q / E down and up, Shift faster,
the wheel a speed factor, right drag or M looks around, F switches free
flight and orbit, / searches, N the next hit, T teleports, 1-9 saved spots
(Ctrl saves), Home back to the first view, Z frames the chosen island, O
opens in axross, L the legend, H the help. Read-only: nothing on any disk
is changed.";

fn value_after(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn main() -> eframe::Result {
    #[cfg(windows)]
    attach_to_the_terminal();
    let all: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // (options only before `--`; after it, folders - whatever they look like)
    let (raw, rest): (Vec<std::ffi::OsString>, Vec<std::ffi::OsString>) =
        match all.iter().position(|a| a == "--") {
            Some(i) => (all[..i].to_vec(), all[i + 1..].to_vec()),
            None => (all.clone(), Vec::new()),
        };
    let args: Vec<String> = raw
        .iter()
        .map(|a| axross_files_matrix::fspath::os_to_string(a))
        .collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version") {
        println!("axross-files-matrix {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--self-test") {
        #[cfg(target_os = "linux")]
        if let Some(missing) = window_library_missing() {
            eprintln!("axross-files-matrix: the window cannot be opened: {missing}");
            std::process::exit(1);
        }
        println!("{}", update::self_test_answer(update::Version::current()));
        return Ok(());
    }
    if let Some(i) = raw.iter().position(|a| a == "--benchmark") {
        let files = args
            .get(i + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(200_000);
        let size = value_after(&args, "--size")
            .and_then(|v| {
                let (w, h) = v.split_once(['x', 'X'])?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((1920, 1080));
        let settings_path =
            axross_files_matrix::settings::config_dir().join(axross_files_matrix::settings::FILE);
        let opts = axross_files_matrix::bench::BenchOptions {
            files,
            frames: value_after(&args, "--frames")
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            size,
            all: args.iter().any(|a| a == "--all"),
            flat: args.iter().any(|a| a == "--flat"),
            msaa: !args.iter().any(|a| a == "--no-msaa"),
            preference: axross_files_matrix::gpu::Preference::from_key(
                &axross_files_matrix::settings::load(&settings_path).0.gpu,
            ),
        };
        match axross_files_matrix::bench::run(&opts) {
            Ok(r) if args.iter().any(|a| a == "--json") => {
                println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default())
            }
            Ok(r) => print!("{}", r.text()),
            Err(e) => {
                eprintln!("axross-files-matrix: the benchmark cannot run: {e}");
                std::process::exit(3);
            }
        }
        return Ok(());
    }
    if let Some(i) = raw.iter().position(|a| a == "--dump-city") {
        // (the folder: right after the option, or after `--`)
        let Some(dir) = raw
            .get(i + 1)
            .filter(|a| !a.to_string_lossy().starts_with('-'))
            .or(rest.first())
        else {
            eprintln!("axross-files-matrix: --dump-city needs a folder");
            std::process::exit(2);
        };
        let opts = dump::DumpOptions {
            json: args.iter().any(|a| a == "--json"),
            depth: value_after(&args, "--depth")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            ..Default::default()
        };
        let synthetic = value_after(&args, "--synthetic").and_then(|v| v.parse().ok());
        let (tree, layout, status, w, l) = dump::city_of(&PathBuf::from(dir), synthetic);
        print!("{}", dump::render(&tree, &layout, &status, &opts, w, l));
        if status.error.is_some() && synthetic.is_none() {
            std::process::exit(1);
        }
        return Ok(());
    }
    // the options; what is left over are folders (raw: names that are no
    // UTF-8 are kept)
    let mut opts = app::StartOptions::default();
    let mut skip_next = false;
    for (k, a) in raw.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let s = a.to_string_lossy();
        match s.as_ref() {
            "--frames" | "--size" | "--depth" => skip_next = true,
            "--all" | "--json" | "--flat" | "--no-msaa" => {}
            "--synthetic" => {
                opts.synthetic = args.get(k + 1).and_then(|v| v.parse().ok());
                skip_next = true;
            }
            "--no-mcp" => opts.mcp = false,
            "--no-hardware" => opts.hardware = false,
            "--no-cache" => opts.use_cache = false,
            "--frame-stats" => opts.frame_stats = true,
            "--fixed-lod" => opts.fixed_lod = true,
            "--foreground" | "-F" | "--after-update" => {}
            s if s.starts_with('-') => {
                eprintln!("axross-files-matrix: unknown option {s} (--help says which there are)");
                std::process::exit(2);
            }
            _ => opts.paths.push(PathBuf::from(a)),
        }
    }
    opts.paths.extend(rest.iter().map(PathBuf::from));
    // (from a terminal: to the background)
    if axross_files_matrix::detach::wanted(&args) {
        match axross_files_matrix::detach::spawn(&all) {
            axross_files_matrix::detach::Start::Exit(0) => return Ok(()),
            axross_files_matrix::detach::Start::Exit(code) => std::process::exit(code),
            axross_files_matrix::detach::Start::Run => {}
        }
    }
    // (the first start of a program the updater put in place: watched -
    // where the one before never drew its window, the old one is put back)
    {
        use update::{FirstStart, Install, Version, first_start};
        let install = Install::current();
        if install.can_update()
            && let FirstStart::RolledBack(old) = first_start(&install.target, Version::current())
        {
            eprintln!(
                "axross-files-matrix: {} did not start here; the program it replaced is back",
                Version::current()
            );
            let mut command = std::process::Command::new(&old);
            command.args(std::env::args_os().skip(1));
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let e = command.exec();
                eprintln!("axross-files-matrix: {}: {e}", old.display());
                std::process::exit(1);
            }
            #[cfg(not(unix))]
            std::process::exit(command.status().ok().and_then(|s| s.code()).unwrap_or(1));
        }
    }
    #[cfg(target_os = "linux")]
    if let Some(missing) = window_library_missing() {
        eprintln!("axross-files-matrix: the window cannot be opened: {missing}");
        std::process::exit(1);
    }
    let settings_path =
        axross_files_matrix::settings::config_dir().join(axross_files_matrix::settings::FILE);
    let saved = axross_files_matrix::settings::load(&settings_path).0;
    let size = saved.window.unwrap_or([1280.0, 800.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("axross files matrix")
            .with_app_id("axross-files-matrix")
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0]),
        // (the dedicated GPU unless the settings say otherwise)
        wgpu_options: axross_files_matrix::gpu::configuration(
            axross_files_matrix::gpu::Preference::from_key(&saved.gpu),
        ),
        ..Default::default()
    };
    eframe::run_native(
        "axross-files-matrix",
        options,
        Box::new(move |cc| {
            let app = app::App::new(cc, opts);
            #[cfg(unix)]
            app::close_on_signals(&cc.egui_ctx);
            axross_files_matrix::detach::signal_ready();
            Ok(Box::new(app))
        }),
    )
}

/// The first library the window needs (winit loads them as it starts, by
/// name) that cannot be loaded, said with the packages that carry it.
#[cfg(target_os = "linux")]
fn window_library_missing() -> Option<String> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    let wayland =
        set("WAYLAND_DISPLAY") && std::env::var("WINIT_UNIX_BACKEND").map_or(true, |b| b != "x11");
    let mut needed = vec![(
        "libxkbcommon.so.0",
        ["libxkbcommon0", "libxkbcommon", "libxkbcommon"],
    )];
    if wayland {
        needed.push((
            "libwayland-client.so.0",
            ["libwayland-client0", "libwayland-client", "wayland"],
        ));
    } else {
        needed.extend([
            (
                "libxkbcommon-x11.so.0",
                ["libxkbcommon-x11-0", "libxkbcommon-x11", "libxkbcommon-x11"],
            ),
            ("libX11.so.6", ["libx11-6", "libX11", "libx11"]),
            ("libX11-xcb.so.1", ["libx11-xcb1", "libX11-xcb", "libx11"]),
            (
                "libXcursor.so.1",
                ["libxcursor1", "libXcursor", "libxcursor"],
            ),
            ("libXi.so.6", ["libxi6", "libXi", "libxi"]),
        ]);
    }
    needed.into_iter().find_map(|(library, packages)| {
        let name = std::ffi::CString::new(library).ok()?;
        // SAFETY: a library name as the toolkit opens it, closed again at
        // once; nothing of it is used
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if handle.is_null() {
            let [debian, fedora, arch] = packages;
            Some(format!("{library} is not installed (Debian and Ubuntu: {debian}, Fedora: {fedora}, Arch: {arch})"))
        } else {
            // SAFETY: the handle dlopen just gave
            unsafe { libc::dlclose(handle) };
            None
        }
    })
}

/// A GUI program on Windows has no console; started from one it writes to
/// that one (--help, --version, a start that fails).
#[cfg(windows)]
fn attach_to_the_terminal() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no arguments but a constant; fails harmlessly without a parent
    // console
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
