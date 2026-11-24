//! The Teleport menu's cost per frame with long lists: 2000 saved
//! connections and 500 000 search hits - rows made once, only the rows in
//! sight laid out. (Measured in the window's own frame times.)

mod support;

use axross_files_matrix::app::App;
use egui_kittest::Harness;
use std::time::{Duration, Instant};
use support::{Scratch, fake_mcp, options};

/// The median UI work of `n` frames (milliseconds): a frame the machine
/// stalled (a throttled sandbox, another process) does not count.
fn work(h: &mut Harness<'static, App>, n: usize) -> f32 {
    h.state_mut().times.work.clear();
    for _ in 0..n {
        h.run_steps(1);
    }
    let w: Vec<f32> = h.state_mut().times.work.iter().copied().collect();
    median(&w) * 1000.0
}

fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_by(f32::total_cmp);
    let m = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[m - 1] + v[m]) * 0.5
    } else {
        v[m]
    }
}

#[test]
fn the_median_leaves_out_stalls() {
    assert_eq!(median(&[1.0, 1.0, 50.0]), 1.0);
    assert_eq!(median(&[1.0, 2.0, 3.0, 40.0]), 2.5);
    assert_eq!(median(&[]), 0.0);
}

#[test]
fn two_thousand_systems_and_half_a_million_hits_cost_little_a_frame() {
    let s = Scratch::new("menu-cost");
    let (client, _) = fake_mcp::start_with_profiles(2000);
    let mut o = options(&s);
    o.mcp = true;
    o.mcp_client = Some(client);
    o.hardware = false;
    o.synthetic = Some(3000);
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_max_steps(4)
        .build_eframe(move |cc| App::new(cc, o));
    let start = Instant::now();
    while h.state_mut().system_profiles().len() < 2000 {
        assert!(start.elapsed() < Duration::from_secs(60));
        h.state_mut().pump(Duration::from_millis(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state_mut().settle(Duration::from_secs(60)));
    // half a million hits (the city's files, over and over)
    h.state_mut().run_search("*");
    h.state_mut().finish_search();
    {
        let app = h.state_mut();
        let s = app.search.as_mut().unwrap();
        let base = s.hits.clone();
        while s.hits.len() < 500_000 {
            s.hits.extend_from_slice(&base);
        }
    }
    let closed = work(&mut h, 30);
    h.state_mut().teleport_open = true;
    work(&mut h, 5);
    let open = work(&mut h, 30);
    h.state_mut().teleport_filter = "many-01".into();
    work(&mut h, 40); // (the filter's first pass over the hits, a little each frame)
    let filtered = work(&mut h, 30);
    eprintln!(
        "menu cost: closed {closed:.2} ms, open {open:.2} ms, filtered {filtered:.2} ms a frame"
    );
    // (medians over closed; debug builds are several times slower than
    // the release, whose about 2 ms get twice that as margin)
    let limit = if cfg!(debug_assertions) { 25.0 } else { 4.0 };
    assert!(
        open - closed < limit,
        "open {open:.2} vs closed {closed:.2}"
    );
    assert!(
        filtered - closed < limit,
        "filtered {filtered:.2} vs closed {closed:.2}"
    );
}
