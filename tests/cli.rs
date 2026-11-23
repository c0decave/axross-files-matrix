//! The program from the outside: --version, --self-test, --help, an unknown
//! option, and --dump-city (text and JSON) on a folder with odd names.

use std::process::Command;

fn exe() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_axross-files-matrix"));
    // (never to the background, never an update check)
    c.env("AXROSS_FILES_MATRIX_NO_UPDATE_CHECK", "1");
    c
}

#[test]
fn version_self_test_and_help() {
    let v = exe().arg("--version").output().unwrap();
    assert!(v.status.success());
    assert_eq!(
        String::from_utf8_lossy(&v.stdout).trim(),
        format!("axross-files-matrix {}", env!("CARGO_PKG_VERSION"))
    );
    let h = exe().arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&h.stdout).contains("--dump-city"));
    let bad = exe().arg("--frobnicate").output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    // (the self-test needs the window's libraries; where they are, it answers)
    let st = exe().arg("--self-test").output().unwrap();
    if st.status.success() {
        assert_eq!(
            String::from_utf8_lossy(&st.stdout).trim(),
            format!("axross-files-matrix {} ok", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn dumps_a_city_as_json() {
    let dir = std::env::temp_dir().join(format!("afm-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs/deep")).unwrap();
    std::fs::write(dir.join("docs/readme.md"), "hello").unwrap();
    std::fs::write(dir.join("docs/deep/data.bin"), vec![0u8; 4096]).unwrap();
    std::fs::write(dir.join("tab\tname.txt"), "x").unwrap();
    let out = exe()
        .arg("--dump-city")
        .arg(&dir)
        .arg("--json")
        .arg("--depth")
        .arg("5")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["city"]["files"], 3);
    assert_eq!(v["city"]["bytes"], 4096 + 5 + 1);
    let names: Vec<String> = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect();
    // (exact: JSON escapes the tab itself; a script gets the name back)
    assert!(names.contains(&"tab\tname.txt".to_string()), "{names:?}");
    let data = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "data.bin")
        .unwrap();
    assert_eq!(data["path"], "docs/deep/data.bin");
    assert!(data["rect"].is_array());
    // after `--` a name is a folder, whatever it looks like - and an
    // option there is no option
    std::fs::create_dir_all(dir.join("-x")).unwrap();
    std::fs::write(dir.join("-x/inside.txt"), "y").unwrap();
    let dashed = exe()
        .current_dir(&dir)
        .args(["--dump-city", "--json", "--", "-x"])
        .output()
        .unwrap();
    assert!(
        dashed.status.success(),
        "{}",
        String::from_utf8_lossy(&dashed.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&dashed.stdout).unwrap();
    assert_eq!(v["city"]["files"], 1);
    let not_help = exe()
        .args(["--dump-city"])
        .arg(dir.join("-x"))
        .args(["--", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&not_help.stdout).starts_with("city of "));
    let text = exe().arg("--dump-city").arg(&dir).output().unwrap();
    assert!(String::from_utf8_lossy(&text.stdout).starts_with("city of "));
    let missing = exe()
        .arg("--dump-city")
        .arg(dir.join("nope"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_benchmark_runs_headless_on_a_small_city() {
    let out = exe()
        .args([
            "--benchmark",
            "3000",
            "--frames",
            "12",
            "--size",
            "320x200",
            "--json",
        ])
        .output()
        .unwrap();
    if out.status.code() == Some(3) {
        // (no GPU adapter at all: said so, exit status 3)
        let msg = "SKIPPED: no GPU adapter - the benchmark was not run";
        assert!(std::env::var_os("AFM_REQUIRE_GPU").is_none(), "{msg}");
        eprintln!("{msg}");
        return;
    }
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["files"], 3000);
    assert_eq!(v["frames"], 12);
    assert_eq!(v["size"], serde_json::json!([320, 200]));
    assert!(!v["adapter"].as_str().unwrap().is_empty());
    assert!(v["instances"]["max"].as_u64().unwrap() > 0);
    assert_eq!(v["instanced_draw_calls"], 1);
    assert!(v["draw_calls"].as_u64().unwrap() >= 10);
    for key in ["cpu_ms", "frame_ms"] {
        let s = &v[key];
        let (m, p95, p99) = (
            s["median"].as_f64().unwrap(),
            s["p95"].as_f64().unwrap(),
            s["p99"].as_f64().unwrap(),
        );
        assert!(m > 0.0 && m <= p95 && p95 <= p99, "{key}: {s}");
    }
    // and as text
    let text = exe()
        .args(["--benchmark", "1000", "--frames", "6", "--size", "160x100"])
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&text.stdout);
    assert!(
        t.contains("adapter: ") && t.contains("draw calls") && t.contains("p99"),
        "{t}"
    );
}
