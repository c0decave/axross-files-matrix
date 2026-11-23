//! The window's tree against a plain walk: a big folder that streams in
//! pieces (local) or pages (remote) while the camera stands right over it -
//! the window asks for it then - holds every entry once. Repeated: it was
//! a race.

mod support;

use axross_files_matrix::app::App;
use axross_files_matrix::walk::{Limits, Source, walk_blocking};
use egui_kittest::Harness;
use std::time::{Duration, Instant};
use support::{Scratch, options};

fn settle_close(h: &mut Harness<'static, App>) {
    let start = Instant::now();
    loop {
        // the camera over the island's middle, low: near folders are asked for
        let c = h
            .state_mut()
            .islands
            .iter()
            .filter(|i| i.hw.is_none())
            .find_map(|i| i.city.as_ref().map(|c| (c.center(), c.side())));
        if let Some((c, side)) = c {
            h.state_mut().look_from(
                c + axross_files_matrix::math::vec3(
                    0.0,
                    side.max(20.0) * 0.3,
                    side.max(20.0) * 0.2,
                ),
                c,
            );
        }
        h.run_steps(1);
        h.state_mut().pump(Duration::from_millis(5));
        let a = h.state_mut();
        let walking = a.islands.iter().any(|i| !i.status.done && i.hw.is_none());
        if !walking
            && !a.starting
            && a.islands
                .iter()
                .all(|i| i.deferred.is_empty() && i.fresh.is_none())
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "settled");
    }
    assert!(h.state_mut().settle(Duration::from_secs(60)));
}

#[test]
fn a_big_local_folder_is_counted_once_in_the_window() {
    let s = Scratch::new("counts-local");
    let dir = s.0.join("big");
    std::fs::create_dir_all(&dir).unwrap();
    let n = 30_000;
    for i in 0..n {
        std::fs::File::create(dir.join(format!("f{i:06}.dat"))).unwrap();
    }
    let (plain, _) = walk_blocking(
        Source::Local {
            root: dir.clone(),
            skip: Vec::new(),
            auto_depth: None,
        },
        Limits::default(),
        std::ffi::OsStr::new("big"),
    );
    for round in 0..3 {
        let mut o = options(&s);
        o.paths = vec![dir.clone()];
        o.hardware = false;
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size(egui::vec2(1000.0, 700.0))
            .with_max_steps(4)
            .build_eframe(move |cc| App::new(cc, o));
        settle_close(&mut h);
        let isl = &h.state_mut().islands[0];
        assert_eq!(
            isl.tree.len(),
            plain.len(),
            "round {round}: every entry once"
        );
        assert_eq!(isl.tree.root().count, n as u32);
    }
}

#[test]
fn a_big_remote_folder_is_counted_once_in_the_window() {
    let s = Scratch::new("counts-remote");
    for round in 0..3 {
        let (client, _) = support::fake_mcp::start_with_big_folder(6012, 2000);
        let mut o = options(&s);
        o.mcp = true;
        o.mcp_client = Some(client.clone());
        o.hardware = false;
        let mut h = Harness::builder()
            .allow_missing_glyphs()
            .with_size(egui::vec2(1000.0, 700.0))
            .with_max_steps(4)
            .build_eframe(move |cc| App::new(cc, o));
        let start = Instant::now();
        while h.state_mut().system_profiles().is_empty() {
            assert!(start.elapsed() < Duration::from_secs(30));
            h.state_mut().pump(Duration::from_millis(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        h.state_mut().teleport_system("big", Vec::new());
        settle_close(&mut h);
        let i = h.state_mut().system_island("big").unwrap();
        let isl = &h.state_mut().islands[i];
        let names: std::collections::HashSet<_> = isl
            .tree
            .nodes
            .iter()
            .skip(1)
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(isl.tree.len(), 6013, "round {round}: every entry once");
        assert_eq!(names.len(), 6012);
    }
}
