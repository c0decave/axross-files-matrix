//! The walk cache from the outside: a second start shows the city at once
//! from the cache, and a folder that cannot be read any more keeps its
//! cached city.

mod support;

use axross_files_matrix::app::App;
use std::time::Duration;
use support::{Scratch, options};

fn start(s: &Scratch, folder: &std::path::Path) -> App {
    let mut opts = options(s);
    opts.paths = vec![folder.to_path_buf()];
    opts.use_cache = true;
    opts.hardware = false;
    App::with_context(&egui::Context::default(), opts, None)
}

#[test]
fn the_cache_brings_the_city_back_at_once() {
    let s = Scratch::new("cache-start");
    let folder = s.0.join("data");
    std::fs::create_dir_all(folder.join("deep/er")).unwrap();
    for k in 0..200 {
        std::fs::write(folder.join(format!("deep/er/f{k}.txt")), vec![b'x'; k]).unwrap();
    }
    let mut first = start(&s, &folder);
    assert!(first.settle(Duration::from_secs(30)));
    let nodes = first.islands[0].tree.len();
    assert_eq!(nodes, 1 + 2 + 200);
    first.save_all();
    drop(first);
    let cache_files: Vec<_> = std::fs::read_dir(s.0.join("cache"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(cache_files.len(), 1, "one cache file for the one island");
    // the folder is gone (unmounted, say): the cached city still stands
    std::fs::remove_dir_all(&folder).unwrap();
    let mut second = start(&s, &folder);
    assert!(second.settle(Duration::from_secs(30)));
    let isl = &second.islands[0];
    assert_eq!(isl.tree.len(), nodes, "the cached tree");
    assert!(isl.from_cache.is_some());
    assert!(isl.status.error.is_some());
    assert!(isl.city.is_some());
}

#[test]
fn a_corrupt_cache_is_ignored() {
    let s = Scratch::new("cache-corrupt");
    let folder = s.0.join("data");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "a").unwrap();
    let mut first = start(&s, &folder);
    assert!(first.settle(Duration::from_secs(30)));
    first.save_all();
    drop(first);
    for f in std::fs::read_dir(s.0.join("cache")).unwrap().flatten() {
        let mut b = std::fs::read(f.path()).unwrap();
        let n = b.len();
        b[n / 2] ^= 0xff;
        std::fs::write(f.path(), b).unwrap();
    }
    let mut second = start(&s, &folder);
    assert!(second.settle(Duration::from_secs(30)));
    assert!(second.islands[0].from_cache.is_none());
    assert_eq!(second.islands[0].tree.len(), 2);
}
