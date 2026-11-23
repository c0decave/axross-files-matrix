//! The Teleport menu's cost per frame with long lists: 2000 saved
//! connections and 500 000 search hits - rows made once, only the rows in
//! sight laid out. (Measured in the window's own frame times.)

mod support;

use axross_files_matrix::app::App;
use egui_kittest::Harness;
use std::time::{Duration, Instant};
use support::{Scratch, fake_mcp, options};

/// The mean UI work of `n` frames (milliseconds).
fn work(h: &mut Harness<'static, App>, n: usize) -> f32 {
    h.state_mut().times.work.clear();
    for _ in 0..n {
        h.run_steps(1);
    }
    let w = &h.state_mut().times.work;
    w.iter().sum::<f32>() / w.len().max(1) as f32 * 1000.0
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
    let closed = work(&mut h, 20);
    h.state_mut().teleport_open = true;
    work(&mut h, 5);
    let open = work(&mut h, 30);
    h.state_mut().teleport_filter = "many-01".into();
    work(&mut h, 40); // (the filter's first pass over the hits, a little each frame)
    let filtered = work(&mut h, 30);
    eprintln!(
        "menu cost: closed {closed:.2} ms, open {open:.2} ms, filtered {filtered:.2} ms a frame"
    );
    // (debug builds are several times slower than the release's 2 ms)
    let limit = if cfg!(debug_assertions) { 25.0 } else { 2.0 };
    assert!(
        open - closed < limit,
        "open {open:.2} vs closed {closed:.2}"
    );
    assert!(
        filtered - closed < limit,
        "filtered {filtered:.2} vs closed {closed:.2}"
    );
}
