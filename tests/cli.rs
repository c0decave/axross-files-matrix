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
    assert!(names.contains(&"tab\\tname.txt".to_string()), "{names:?}");
    let data = v["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "data.bin")
        .unwrap();
    assert_eq!(data["path"], "docs/deep/data.bin");
    assert!(data["rect"].is_array());
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
