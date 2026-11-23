//! The updater against a fake GitHub: the API's answer, the release's
//! files and their redirects, all served from memory.

use super::*;
use crate::testdir::Scratch;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const TAG: &str = "v99.0.0";
const PROGRAM: &[u8] = b"the new axross-files-matrix\x7fELF...";

fn keypair() -> Ed25519KeyPair {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

fn public(key: &Ed25519KeyPair) -> [u8; 32] {
    key.public_key().as_ref().try_into().unwrap()
}

fn sha(data: &[u8]) -> String {
    hex(ring::digest::digest(&SHA256, data).as_ref())
}

/// One answer of the fake: status, Location, body, and a Content-Length
/// that may lie.
#[derive(Clone)]
struct Canned {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
    length: Option<u64>,
}

fn ok(body: &[u8]) -> Canned {
    Canned {
        status: 200,
        location: None,
        body: body.to_vec(),
        length: Some(body.len() as u64),
    }
}

fn redirect(to: &str) -> Canned {
    Canned {
        status: 302,
        location: Some(to.into()),
        body: Vec::new(),
        length: Some(0),
    }
}

#[derive(Default)]
struct FakeGitHub {
    answers: Mutex<HashMap<String, Canned>>,
    asked: Mutex<Vec<String>>,
}

impl Fetch for FakeGitHub {
    fn get(&self, url: &str) -> Result<Reply, String> {
        self.asked.lock().unwrap().push(url.to_string());
        let canned = self
            .answers
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .unwrap_or(Canned {
                status: 404,
                location: None,
                body: Vec::new(),
                length: Some(0),
            });
        Ok(Reply {
            status: canned.status,
            location: canned.location,
            length: canned.length,
            body: Box::new(Cursor::new(canned.body)),
        })
    }
}

impl FakeGitHub {
    fn set(&self, url: &str, canned: Canned) {
        self.answers.lock().unwrap().insert(url.into(), canned);
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

fn url(name: &str) -> String {
    format!("{DOWNLOAD_PREFIX}{TAG}/{name}")
}

fn blob(name: &str) -> String {
    format!("https://release-assets.githubusercontent.com/github-production-release-asset/1/{name}")
}

/// A release of 99.0.0 for the Linux binary, signed with *key*; its files
/// redirect to release-assets as github.com's do.
fn release(github: &FakeGitHub, key: &Ed25519KeyPair) -> Release {
    let name = "axross-files-matrix-99.0.0-linux-x86_64";
    let sums = format!(
        "{}  {name}\n{}  axross-files-matrix-99.0.0-x86_64.AppImage\n",
        sha(PROGRAM),
        sha(b"appimage")
    );
    let sig = key.sign(sums.as_bytes());
    for (file, body) in [
        (name, PROGRAM.to_vec()),
        (SUMS_NAME, sums.as_bytes().to_vec()),
        (SIG_NAME, sig.as_ref().to_vec()),
    ] {
        github.set(&url(file), redirect(&blob(file)));
        github.set(&blob(file), ok(&body));
    }
    let asset = |name: &str, size: usize| {
        serde_json::json!({
            "name": name, "size": size, "browser_download_url": url(name),
        })
    };
    let api = serde_json::json!({
        "tag_name": TAG, "draft": false, "prerelease": false, "body": "Notes\u{202e}",
        "assets": [asset(name, PROGRAM.len()), asset(SUMS_NAME, sums.len()), asset(SIG_NAME, 64)],
    });
    github.set(LATEST_URL, ok(api.to_string().as_bytes()));
    latest(github).unwrap()
}

fn linux(dir: &Path) -> Install {
    let target = dir.join("axross-files-matrix");
    fs::write(&target, b"the old axross-files-matrix").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    }
    detect("linux", "x86_64", &target, None, None)
}

fn run(
    release: &Release,
    install: &Install,
    github: &FakeGitHub,
    keys: &[[u8; 32]],
    cache: &Path,
) -> Result<Downloaded, String> {
    download(
        release,
        install,
        Version(0, 15, 0),
        github,
        keys,
        cache,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn happy_a_signed_release_replaces_the_program_and_keeps_the_old_one_until_the_new_starts() {
    let dir = Scratch::new("update-happy");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    assert_eq!(release.version, Version(99, 0, 0));
    assert_eq!(release.notes, "Notes");
    let inst = linux(&dir);
    let mut seen = Vec::new();
    let got = download(
        &release,
        &inst,
        Version(0, 15, 0),
        &github,
        &[public(&key)],
        &dir.join("cache"),
        &mut |done, total| seen.push((done, total)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(got.in_place);
    assert_eq!(
        seen.last(),
        Some(&(PROGRAM.len() as u64, PROGRAM.len() as u64))
    );
    // the signature is fetched and checked before the program
    let asked = github.asked();
    let first_program = asked
        .iter()
        .position(|u| u.ends_with("linux-x86_64"))
        .unwrap();
    assert!(asked.iter().position(|u| u.ends_with(SIG_NAME)).unwrap() < first_program);
    let done = install(&got, |_, _| Ok(())).unwrap();
    assert_eq!(fs::read(&done.path).unwrap(), PROGRAM);
    assert_eq!(
        fs::read(&done.previous).unwrap(),
        b"the old axross-files-matrix"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&done.path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    assert_eq!(
        leftovers(&dir),
        ["axross-files-matrix", "axross-files-matrix.previous"]
    );
    restart_checks_the_file(&done);
    cleanup_previous(&done.path);
    assert_eq!(leftovers(&dir), ["axross-files-matrix"]);
}

fn restart_checks_the_file(done: &Installed) {
    let mut wrong = done.sha256;
    wrong[0] ^= 1;
    assert!(
        restart(&done.path, &wrong, Duration::ZERO)
            .unwrap_err()
            .contains("changed")
    );
}

#[test]
fn sad_a_bad_signature_is_refused_before_the_program_is_fetched() {
    let dir = Scratch::new("update-badsig");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    let mut sig = key.sign(b"something else").as_ref().to_vec();
    sig[0] ^= 0x55;
    github.set(&blob(SIG_NAME), ok(&sig));
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(
        err.contains("not signed with axross-files-matrix's release key"),
        "{err}"
    );
    assert!(!github.asked().iter().any(|u| u.ends_with("linux-x86_64")));
    assert_eq!(leftovers(&dir), ["axross-files-matrix"]);
}

#[test]
fn sad_a_signature_by_another_key_is_refused() {
    let dir = Scratch::new("update-wrongkey");
    let github = FakeGitHub::default();
    let theirs = keypair();
    let release = release(&github, &theirs);
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&keypair())],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("not signed"), "{err}");
    // and the key pinned in the program is not a test key
    assert!(verify_signature(b"x", theirs.sign(b"x").as_ref(), TRUSTED_KEYS).is_err());
}

#[test]
fn sad_no_signature_or_one_of_another_size() {
    let dir = Scratch::new("update-nosig");
    let github = FakeGitHub::default();
    let key = keypair();
    let mut release = release(&github, &key);
    github.set(&blob(SIG_NAME), ok(&[0u8; 65]));
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("larger") || err.contains("bytes"), "{err}");
    release.assets.retain(|a| a.name != SIG_NAME);
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("not signed"), "{err}");
}

#[test]
fn sad_a_downgrade_or_the_same_version_is_not_fetched() {
    let dir = Scratch::new("update-downgrade");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    let asked = github.asked().len();
    for current in [Version(99, 0, 0), Version(99, 0, 1), Version(100, 0, 0)] {
        let err = download(
            &release,
            &linux(&dir),
            current,
            &github,
            &[public(&key)],
            &dir.join("c"),
            &mut |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(err.contains("not newer"), "{err}");
    }
    assert_eq!(github.asked().len(), asked);
}

#[test]
fn sad_hostile_names_never_become_files() {
    let api = serde_json::json!({
        "tag_name": TAG,
        "assets": [
            {"name": "../axross-files-matrix", "size": 1, "browser_download_url": url("../axross-files-matrix")},
            {"name": ".hidden", "size": 1, "browser_download_url": url(".hidden")},
            {"name": "a b", "size": 1, "browser_download_url": url("a b")},
            {"name": "ok", "size": 1, "browser_download_url": "https://evil.example/ok"},
            {"name": "ok2", "size": 1, "browser_download_url": format!("{DOWNLOAD_PREFIX}v1.0.0/ok2")},
            {"name": "good", "size": 1, "browser_download_url": url("good")},
        ],
    });
    let release = parse_release(api.to_string().as_bytes()).unwrap();
    let names: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["good"]);
    let line = format!("{}  ../../.bashrc\n", sha(b"x"));
    assert!(parse_sums(&line).is_err());
    let twice = format!("{0}  a\n{0}  a\n", sha(b"x"));
    assert!(parse_sums(&twice).is_err());
    assert!(
        parse_sums(&format!("{} *a\n", sha(b"x")))
            .unwrap()
            .contains_key("a")
    );
    for tag in [
        "99.0.0",
        "v99.0",
        "v99.0.0-rc1",
        "v99.0.0\n",
        "v1234567.0.0",
    ] {
        let api = serde_json::json!({"tag_name": tag, "assets": []});
        assert!(
            parse_release(api.to_string().as_bytes()).is_err(),
            "{tag:?}"
        );
    }
    let draft = serde_json::json!({"tag_name": TAG, "draft": true, "assets": []});
    assert!(parse_release(draft.to_string().as_bytes()).is_err());
}

#[test]
fn sad_an_oversized_download_is_cut_off_and_removed() {
    let dir = Scratch::new("update-big");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    // the body longer than the release says (the length header lying)
    let mut longer = PROGRAM.to_vec();
    longer.extend_from_slice(&[0u8; 100_000]);
    github.set(
        &blob("axross-files-matrix-99.0.0-linux-x86_64"),
        Canned {
            status: 200,
            location: None,
            body: longer,
            length: None,
        },
    );
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("larger"), "{err}");
    assert_eq!(leftovers(&dir), ["axross-files-matrix"]);
    // a release that states a size over the cap is not fetched at all
    let mut huge = release.clone();
    huge.assets.iter_mut().for_each(|a| {
        if a.name.ends_with("linux-x86_64") {
            a.size = MAX_ASSET_BYTES + 1;
        }
    });
    let err = run(
        &huge,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("at most"), "{err}");
    // and the API's answer is bounded too
    github.set(
        LATEST_URL,
        Canned {
            status: 200,
            location: None,
            body: vec![b' '; MAX_API_BYTES as usize + 10],
            length: None,
        },
    );
    assert!(latest(&github).unwrap_err().contains("larger"));
}

#[test]
fn sad_a_file_whose_sha256_is_not_the_signed_one() {
    let dir = Scratch::new("update-sha");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    let mut evil = PROGRAM.to_vec();
    evil[0] ^= 1;
    github.set(&blob("axross-files-matrix-99.0.0-linux-x86_64"), ok(&evil));
    let err = run(
        &release,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("SHA-256"), "{err}");
    assert_eq!(leftovers(&dir), ["axross-files-matrix"]);
    // GitHub's digest disagreeing with the signed sums
    let mut other = release.clone();
    other
        .assets
        .iter_mut()
        .for_each(|a| a.digest = Some([7; 32]));
    github.set(
        &blob("axross-files-matrix-99.0.0-linux-x86_64"),
        ok(PROGRAM),
    );
    let err = run(
        &other,
        &linux(&dir),
        &github,
        &[public(&key)],
        &dir.join("c"),
    )
    .unwrap_err();
    assert!(err.contains("digest"), "{err}");
}

#[test]
fn sad_a_redirect_to_another_host_or_over_http_is_not_followed() {
    let github = FakeGitHub::default();
    for to in [
        "https://evil.example/axross-files-matrix",
        "http://release-assets.githubusercontent.com/x",
        "https://user@github.com/x",
        "https://github.com:8443/x",
        "/relative",
    ] {
        github.set(LATEST_URL, redirect(to));
        let err = latest(&github).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            !github
                .asked()
                .iter()
                .any(|u| u.contains("evil") || u.starts_with("http:"))
        );
    }
    // a chain that never ends
    github.set(LATEST_URL, redirect("https://github.com/a"));
    github.set("https://github.com/a", redirect("https://github.com/b"));
    github.set("https://github.com/b", redirect("https://github.com/a"));
    assert!(latest(&github).unwrap_err().contains("redirects"));
    // GitHub's rate limit in words
    github.set(
        LATEST_URL,
        Canned {
            status: 403,
            location: None,
            body: vec![],
            length: Some(0),
        },
    );
    assert!(latest(&github).unwrap_err().contains("too many checks"));
}

#[test]
fn edge_allowed_urls() {
    assert_eq!(
        allowed_url("https://api.github.com/repos").unwrap(),
        "api.github.com"
    );
    assert_eq!(
        allowed_url("https://GitHub.com:443/x").unwrap(),
        "github.com"
    );
    for bad in [
        "https://github.com.evil.example/",
        "https://evilgithub.com/",
        "ftp://github.com/",
        "https://github.com@evil.example/",
    ] {
        assert!(allowed_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn edge_a_folder_that_cannot_be_written_keeps_the_program_and_says_where_the_file_is() {
    let dir = Scratch::new("update-readonly");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    // (its folder does not exist: nothing can be written beside it)
    let install = detect(
        "linux",
        "x86_64",
        &dir.join("gone/axross-files-matrix"),
        None,
        None,
    );
    let got = run(
        &release,
        &install,
        &github,
        &[public(&key)],
        &dir.join("cache"),
    )
    .unwrap();
    assert!(!got.in_place);
    assert_eq!(
        got.path,
        dir.join("cache/axross-files-matrix-99.0.0-linux-x86_64")
    );
    assert_eq!(fs::read(&got.path).unwrap(), PROGRAM);
    assert!(install_err(&got).contains(&got.path.display().to_string()));
}

fn install_err(got: &Downloaded) -> String {
    install(got, |_, _| Ok(())).unwrap_err()
}

#[test]
fn edge_how_the_program_runs_picks_the_file() {
    let v = Version(1, 2, 3);
    let name = |i: &Install| asset_name(i, v);
    let bin = detect(
        "linux",
        "x86_64",
        Path::new("/opt/amp/axross-files-matrix"),
        None,
        None,
    );
    assert_eq!(
        name(&bin).as_deref(),
        Some("axross-files-matrix-1.2.3-linux-x86_64")
    );
    let image = detect(
        "linux",
        "x86_64",
        Path::new("/tmp/.mount_axrossX/usr/bin/axross-files-matrix"),
        Some("/opt/u/Apps/axross-files-matrix-1.0.0-x86_64.AppImage".into()),
        Some("/tmp/.mount_axrossX".into()),
    );
    assert_eq!(image.kind, Kind::AppImage);
    assert_eq!(
        image.target,
        Path::new("/opt/u/Apps/axross-files-matrix-1.0.0-x86_64.AppImage")
    );
    assert_eq!(
        name(&image).as_deref(),
        Some("axross-files-matrix-1.2.3-x86_64.AppImage")
    );
    // APPIMAGE set by hand, the program not inside APPDIR: the binary
    let hand = detect(
        "linux",
        "x86_64",
        Path::new("/opt/amp/axross-files-matrix"),
        Some("/opt/u/x.AppImage".into()),
        Some("/tmp/.mount_x".into()),
    );
    assert_eq!(hand.kind, Kind::Binary);
    let app = detect(
        "macos",
        "aarch64",
        Path::new("/Applications/axross-files-matrix.app/Contents/MacOS/axross-files-matrix"),
        None,
        None,
    );
    assert_eq!(app.kind, Kind::MacApp);
    assert_eq!(
        name(&app).as_deref(),
        Some("axross-files-matrix-1.2.3-macos-universal")
    );
    let win = detect(
        "windows",
        "x86_64",
        Path::new(r"C:\Tools\axross-files-matrix.exe"),
        None,
        None,
    );
    assert_eq!(
        name(&win).as_deref(),
        Some("axross-files-matrix-1.2.3-windows-x64.exe")
    );
    let cargo = detect(
        "linux",
        "x86_64",
        Path::new("/opt/u/.cargo/bin/axross-files-matrix"),
        None,
        None,
    );
    assert_eq!(cargo.kind, Kind::Cargo);
    assert!(cargo.why_not().unwrap().contains("cargo install"));
    let tree = detect(
        "linux",
        "x86_64",
        Path::new("/src/amp/target/release/axross-files-matrix"),
        None,
        None,
    );
    assert_eq!(tree.kind, Kind::Source);
    let arm = detect(
        "linux",
        "aarch64",
        Path::new("/opt/amp/axross-files-matrix"),
        None,
        None,
    );
    assert_eq!(arm.kind, Kind::Unsupported);
    assert_eq!(name(&arm), None);
}

#[test]
fn edge_versions() {
    assert_eq!(Version::parse("v0.15.0"), Some(Version(0, 15, 0)));
    assert!(Version(0, 15, 10) > Version(0, 15, 9));
    for bad in [
        "",
        "1.2",
        "1.2.3.4",
        "1.2.x",
        "01.2.3a",
        "1.2.-3",
        "1234567.0.0",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn edge_the_start_check_is_off_unless_asked_for() {
    assert!(!start_check_allowed(false, None));
    assert!(start_check_allowed(true, None));
    assert!(!start_check_allowed(true, Some("1")));
    assert!(!start_check_allowed(true, Some(" Yes ")));
    assert!(start_check_allowed(true, Some("0")));
}

#[test]
fn edge_cleanup_takes_a_file_only() {
    let dir = Scratch::new("update-cleanup");
    let target = dir.join("axross-files-matrix");
    fs::create_dir(previous_of(&target)).unwrap();
    cleanup_previous(&target);
    assert!(previous_of(&target).is_dir());
}

/// A new program that does not start here (its self-test fails) is not
/// put in place: the old one stays as it was (it was never
/// run before it replaced the old one).
#[test]
fn sad_a_new_program_that_does_not_start_here_is_not_put_in_place() {
    let dir = Scratch::new("update-selftest");
    let github = FakeGitHub::default();
    let key = keypair();
    let release = release(&github, &key);
    let inst = linux(&dir);
    let got = run(
        &release,
        &inst,
        &github,
        &[public(&key)],
        &dir.join("cache"),
    )
    .unwrap();
    let asked = Mutex::new(Vec::new());
    let err = install(&got, |path, version| {
        asked.lock().unwrap().push((path.to_path_buf(), version));
        Err("it does not start here (exit status: 127: version `GLIBC_2.99' not found)".into())
    })
    .unwrap_err();
    assert!(
        err.contains("not installed") && err.contains("GLIBC_2.99"),
        "{err}"
    );
    // it was asked of the file downloaded, for the release's version
    assert_eq!(
        *asked.lock().unwrap(),
        [(got.path.clone(), Version(99, 0, 0))]
    );
    assert_eq!(
        fs::read(&inst.target).unwrap(),
        b"the old axross-files-matrix"
    );
    // (nothing left but the note of the version refused here: never put
    // in place by itself again)
    assert_eq!(
        leftovers(&dir),
        ["axross-files-matrix", "axross-files-matrix.refused"]
    );
    assert_eq!(refused(&inst.target), [Version(99, 0, 0)]);
}

/// The self-test runs the program and wants its answer.
#[cfg(unix)]
#[test]
fn the_self_test_runs_the_new_program() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("update-selftest-run");
    let script = |name: &str, body: &str| {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let v = Version(99, 0, 0);
    let good = script(
        "good",
        &format!("[ \"$1\" = --self-test ] && echo '{}'", self_test_answer(v)),
    );
    assert_eq!(self_test(&good, v), Ok(()));
    // another version, a failure, a program that is no program
    assert!(self_test(&good, Version(98, 0, 0)).is_err());
    let failing = script(
        "failing",
        "echo 'error while loading shared libraries: libfoo.so' >&2; exit 127",
    );
    let err = self_test(&failing, v).unwrap_err();
    assert!(err.contains("libfoo.so"), "{err}");
    let junk = dir.join("junk");
    fs::write(&junk, PROGRAM).unwrap();
    fs::set_permissions(&junk, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(self_test(&junk, v).is_err());
}

/// A new program that ends at its start (Restart now): said, and the old
/// one put back in its place; one that starts is left running.
#[cfg(unix)]
#[test]
fn a_new_program_that_ends_at_its_start_is_rolled_back() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("update-rollback");
    let target = dir.join("axross-files-matrix");
    fs::write(&target, "#!/bin/sh\nexit 3\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    let previous = previous_of(&target);
    fs::write(&previous, b"the old axross-files-matrix").unwrap();
    let installed = Installed {
        path: target.clone(),
        previous: previous.clone(),
        sha256: sha256_of(&target).unwrap(),
        version: Version(99, 0, 0),
    };
    let wait = std::time::Duration::from_secs(5);
    let err = restart(&target, &installed.sha256, wait).unwrap_err();
    assert!(err.contains("ended"), "{err}");
    roll_back(&installed).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"the old axross-files-matrix");
    assert!(!previous.exists());
    // one that starts (its window up: "ok" on the pipe) or still runs
    let up = dir.join("up");
    fs::write(
        &up,
        "#!/bin/sh\necho ok > /dev/fd/$AXROSS_FILES_MATRIX_READY_FD\nsleep 2\n",
    )
    .unwrap();
    fs::set_permissions(&up, fs::Permissions::from_mode(0o755)).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(restart(&up, &sha256_of(&up).unwrap(), wait), Ok(()));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "its word waited for"
    );
    let slow = dir.join("slow");
    fs::write(&slow, "#!/bin/sh\nsleep 3\n").unwrap();
    fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).unwrap();
    let short = std::time::Duration::from_millis(500);
    assert_eq!(restart(&slow, &sha256_of(&slow).unwrap(), short), Ok(()));
}

/// Restart now starts the new program with the argument that tells it so.
#[test]
fn the_new_program_is_started_to_take_over_the_name() {
    assert_eq!(RESTART_ARGS, ["--after-update"]);
}

/// A build in a cargo folder of any name is no release file (not only a
/// folder named "target").
#[test]
fn edge_a_build_in_a_custom_target_dir_is_a_source_build() {
    let dir = Scratch::new("update-custom-target");
    let profile = dir.join("my-build/debug");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join(".cargo-lock"), b"").unwrap();
    let exe = profile.join("axross-files-matrix");
    fs::write(&exe, b"x").unwrap();
    assert_eq!(
        detect("linux", "x86_64", &exe, None, None).kind,
        Kind::Source
    );
    // the tests' binaries, deeper (build/<crate>/<hash>/out)
    let top = dir.join("shared-target");
    let deep = top.join("debug/build/axross-files-matrix/0123456789abcdef/out");
    fs::create_dir_all(&deep).unwrap();
    fs::write(top.join("debug/.cargo-lock"), b"").unwrap();
    let test = deep.join("update_window-0123456789abcdef");
    fs::write(&test, b"x").unwrap();
    assert_eq!(
        detect("linux", "x86_64", &test, None, None).kind,
        Kind::Source
    );
    // a release file copied elsewhere: one
    let plain = dir.join("opt/axross-files-matrix");
    fs::create_dir_all(plain.parent().unwrap()).unwrap();
    fs::write(&plain, b"x").unwrap();
    assert_eq!(
        detect("linux", "x86_64", &plain, None, None).kind,
        Kind::Binary
    );
}

/// A download that ended with its program (a closed window, a crash)
/// leaves nothing for good: an hour old, it is removed at the next start.
#[test]
fn edge_a_download_left_behind_is_removed() {
    let dir = Scratch::new("update-leftover");
    let old = dir.join(".axross-files-matrix-update-123-abcdef");
    let fresh = dir.join(".axross-files-matrix-update-456-012345");
    let other = dir.join("axross-files-matrix");
    for p in [&old, &fresh, &other] {
        fs::write(p, b"x").unwrap();
    }
    let hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(hours_ago)
        .unwrap();
    File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(hours_ago)
        .unwrap();
    cleanup_downloads(&dir);
    assert_eq!(
        leftovers(&dir),
        [
            ".axross-files-matrix-update-456-012345",
            "axross-files-matrix"
        ]
    );
}

/// The checks in the background: at the start, then every 24 hours; an
/// hour after one that could not be made.
#[test]
fn the_checks_come_at_the_start_and_then_once_a_day() {
    let h = |hours: u64| Some(Duration::from_secs(hours * 3600));
    assert!(check_due(None, false), "at the start");
    assert!(!check_due(h(1), false));
    assert!(!check_due(h(23), false));
    assert!(check_due(h(24), false), "a day later");
    assert!(!check_due(Some(Duration::from_secs(600)), true));
    assert!(check_due(h(1), true), "an hour after one not made");
    // not on an offline or a metered network (where the system says so)
    assert!(network_allows(Network::default()));
    assert!(!network_allows(Network {
        offline: Some(true),
        metered: None
    }));
    assert!(!network_allows(Network {
        offline: Some(false),
        metered: Some(true)
    }));
    assert!(network_allows(Network {
        offline: Some(false),
        metered: Some(false)
    }));
    // the settings' words
    for m in UpdateMode::ALL {
        assert_eq!(UpdateMode::from_key(m.key()), Some(m));
    }
    assert_eq!(UpdateMode::from_key("sometimes"), None);
}

/// An updater on a fake GitHub (a release of 99.0.0 signed by `key`) for
/// the program in `dir`, the version running 0.15.0.
fn updater(
    dir: &Path,
    github: Arc<FakeGitHub>,
    key: &Ed25519KeyPair,
    self_test: fn(&Path, Version) -> Result<(), String>,
    network: fn() -> Network,
) -> crate::update_ui::Updater {
    crate::update_ui::Updater::with_source(
        linux(dir),
        crate::update_ui::Source {
            web: github,
            keys: vec![public(key)],
            cache: dir.join("cache"),
            self_test,
            network,
            current: Version(0, 15, 0),
        },
    )
}

/// Frames until the updater rests (no thread at work), at most 10 s.
fn until_rest(u: &mut crate::update_ui::Updater, ctx: &egui::Context, mode: UpdateMode) {
    use crate::update_ui::Stage;
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < end {
        u.tick(ctx, mode);
        u.poll(ctx);
        if !matches!(u.stage, Stage::Checking | Stage::Downloading(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the updater still works: {:?}", u.stage);
}

/// "Download and install by themselves": a newer signed release is fetched,
/// checked, run once and put in place in the background - and nothing is
/// restarted (the flight goes on; the new version runs from the next start
/// or when asked).
#[test]
fn installed_by_itself_and_nothing_restarted() {
    use crate::update_ui::Stage;
    let dir = Scratch::new("update-auto");
    let github = Arc::new(FakeGitHub::default());
    let key = keypair();
    release(&github, &key);
    let mut u = updater(&dir, github.clone(), &key, |_, _| Ok(()), Network::default);
    let ctx = egui::Context::default();
    until_rest(&mut u, &ctx, UpdateMode::Install);
    assert!(matches!(u.stage, Stage::Ready(_)), "{:?}", u.stage);
    assert_eq!(fs::read(dir.join("axross-files-matrix")).unwrap(), PROGRAM);
    assert!(
        fs::read(dir.join("axross-files-matrix.previous")).is_ok(),
        "the old one kept"
    );
    let said = u.take_notice().unwrap_or_default();
    assert!(
        said.contains("installed") && said.contains("next start"),
        "{said}"
    );
    assert!(!u.closing(), "the program was closed");
    // and no second check before a day is over
    let asked = github.asked().len();
    for _ in 0..5 {
        u.tick(&ctx, UpdateMode::Install);
        u.poll(&ctx);
    }
    assert_eq!(github.asked().len(), asked);
}

/// "Say when one is out" (the default): said, nothing fetched but the
/// release's word.
#[test]
fn said_when_one_is_out_and_nothing_fetched() {
    use crate::update_ui::Stage;
    let dir = Scratch::new("update-notify");
    let github = Arc::new(FakeGitHub::default());
    let key = keypair();
    release(&github, &key);
    let mut u = updater(&dir, github.clone(), &key, |_, _| Ok(()), Network::default);
    let ctx = egui::Context::default();
    until_rest(&mut u, &ctx, UpdateMode::Notify);
    assert!(matches!(u.stage, Stage::Available(_)), "{:?}", u.stage);
    assert!(
        u.take_notice()
            .unwrap_or_default()
            .contains("99.0.0 is out")
    );
    assert_eq!(
        fs::read(dir.join("axross-files-matrix")).unwrap(),
        b"the old axross-files-matrix"
    );
    assert!(
        github.asked().iter().all(|u| u == LATEST_URL),
        "{:?}",
        github.asked()
    );
    // off: nothing asked at all
    let quiet = Arc::new(FakeGitHub::default());
    let mut off = updater(&dir, quiet.clone(), &key, |_, _| Ok(()), Network::default);
    off.tick(&ctx, UpdateMode::Off);
    std::thread::sleep(Duration::from_millis(100));
    off.poll(&ctx);
    assert!(quiet.asked().is_empty());
}

/// By themselves too: a release signed by another key, an older one, a
/// new program that does not start here - none is put in place, the old
/// program stays; on a metered network nothing is asked.
#[test]
fn installed_by_itself_only_when_every_check_holds() {
    let key = keypair();
    let ctx = egui::Context::default();
    // another key
    let dir = Scratch::new("update-auto-badsig");
    let github = Arc::new(FakeGitHub::default());
    release(&github, &keypair());
    let mut u = updater(&dir, github, &key, |_, _| Ok(()), Network::default);
    until_rest(&mut u, &ctx, UpdateMode::Install);
    assert_eq!(
        fs::read(dir.join("axross-files-matrix")).unwrap(),
        b"the old axross-files-matrix"
    );
    assert!(!dir.join("axross-files-matrix.previous").exists());
    // no downgrade: the running version as new as the release
    let dir = Scratch::new("update-auto-old");
    let github = Arc::new(FakeGitHub::default());
    release(&github, &key);
    let mut u = crate::update_ui::Updater::with_source(
        linux(&dir),
        crate::update_ui::Source {
            web: github,
            keys: vec![public(&key)],
            cache: dir.join("cache"),
            self_test: |_, _| Ok(()),
            network: Network::default,
            current: Version(99, 0, 0),
        },
    );
    until_rest(&mut u, &ctx, UpdateMode::Install);
    assert_eq!(
        fs::read(dir.join("axross-files-matrix")).unwrap(),
        b"the old axross-files-matrix"
    );
    // the new program does not start here: the old one kept (its self-test)
    let dir = Scratch::new("update-auto-selftest");
    let github = Arc::new(FakeGitHub::default());
    release(&github, &key);
    let mut u = updater(
        &dir,
        github,
        &key,
        |_, _| Err("it does not start here".into()),
        Network::default,
    );
    until_rest(&mut u, &ctx, UpdateMode::Install);
    assert_eq!(
        fs::read(dir.join("axross-files-matrix")).unwrap(),
        b"the old axross-files-matrix"
    );
    assert!(
        u.take_notice()
            .unwrap_or_default()
            .contains("not installed")
    );
    assert!(!u.closing());
    // a metered network: nothing asked
    let dir = Scratch::new("update-auto-metered");
    let github = Arc::new(FakeGitHub::default());
    release(&github, &key);
    let asked_before = github.asked().len();
    let mut u = updater(
        &dir,
        github.clone(),
        &key,
        |_, _| Ok(()),
        || Network {
            offline: Some(false),
            metered: Some(true),
        },
    );
    until_rest(&mut u, &ctx, UpdateMode::Install);
    assert_eq!(
        github.asked().len(),
        asked_before,
        "asked on a metered network"
    );
    assert_eq!(
        fs::read(dir.join("axross-files-matrix")).unwrap(),
        b"the old axross-files-matrix"
    );
}

/// A program put in place: its first start watched, wherever it comes (by
/// itself it runs from the next start, not by *Restart now*). The first
/// frame drawn ends the watch; a start of that version that finds the one
/// before never drew its window puts the old program back, to run instead,
/// and that version is never put in place by itself again (else it would
/// run with nothing watching, and be put in place again the next day).
#[test]
fn the_first_start_of_a_program_put_in_place_is_watched() {
    let dir = Scratch::new("update-first-start");
    let target = dir.join("axross-files-matrix");
    let previous = previous_of(&target);
    let installed = Installed {
        path: target.clone(),
        previous: previous.clone(),
        sha256: [0; 32],
        version: Version(99, 0, 0),
    };
    // it starts, draws its window: nothing more
    fs::write(&target, b"the new axross-files-matrix").unwrap();
    fs::write(&previous, b"the old axross-files-matrix").unwrap();
    mark_first_start(&installed);
    assert_eq!(first_start(&target, Version(99, 0, 0)), FirstStart::Watched);
    first_start_done(&target);
    assert_eq!(first_start(&target, Version(99, 0, 0)), FirstStart::None);
    assert_eq!(fs::read(&target).unwrap(), b"the new axross-files-matrix");
    // it starts and never draws its window: at its next start, the old one
    // is back (and run), that version refused
    mark_first_start(&installed);
    assert_eq!(first_start(&target, Version(99, 0, 0)), FirstStart::Watched);
    assert_eq!(
        first_start(&target, Version(99, 0, 0)),
        FirstStart::RolledBack(target.clone())
    );
    assert_eq!(fs::read(&target).unwrap(), b"the old axross-files-matrix");
    assert!(refused(&target).contains(&Version(99, 0, 0)));
    assert!(!first_start_of(&target).exists());
    // the old program's start: nothing to watch
    assert_eq!(first_start(&target, Version(0, 15, 0)), FirstStart::None);
    // a marker of another version (the program replaced by hand) is let go
    mark_first_start(&installed);
    assert_eq!(first_start(&target, Version(98, 0, 0)), FirstStart::None);
    assert!(!first_start_of(&target).exists());
}
