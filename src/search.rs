//! Search by name across everything walked: a plain text matches any part
//! of a name (any case), a pattern with `*`, `?` or `[...]` is a glob over
//! the whole name (any case; `[!...]` negates, `a-z` ranges). It runs a few
//! milliseconds a frame (the trees go on growing meanwhile), so a search
//! over millions of names never stalls the window.

use crate::tree::{NodeId, Tree};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Char(char),
    Any,
    Star,
    Class(Vec<(char, char)>, bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Substring(String),
    Glob(Vec<Tok>),
}

impl Pattern {
    pub fn parse(query: &str) -> Pattern {
        let q = query.trim();
        if !q.contains(['*', '?', '[']) {
            return Pattern::Substring(q.to_lowercase());
        }
        let mut toks = Vec::new();
        let chars: Vec<char> = q.to_lowercase().chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '*' => {
                    if toks.last() != Some(&Tok::Star) {
                        toks.push(Tok::Star);
                    }
                }
                '?' => toks.push(Tok::Any),
                '[' => {
                    // a class up to the next ']' (none: a plain '[')
                    let close = chars[i + 1..]
                        .iter()
                        .skip(1)
                        .position(|&c| c == ']')
                        .map(|p| p + i + 2);
                    let Some(end) = close else {
                        toks.push(Tok::Char('['));
                        i += 1;
                        continue;
                    };
                    let mut body = &chars[i + 1..end];
                    let negate = matches!(body.first(), Some('!') | Some('^'));
                    if negate {
                        body = &body[1..];
                    }
                    let mut ranges = Vec::new();
                    let mut k = 0;
                    while k < body.len() {
                        if k + 2 < body.len() && body[k + 1] == '-' {
                            ranges.push((body[k], body[k + 2]));
                            k += 3;
                        } else {
                            ranges.push((body[k], body[k]));
                            k += 1;
                        }
                    }
                    toks.push(Tok::Class(ranges, negate));
                    i = end;
                }
                c => toks.push(Tok::Char(c)),
            }
            i += 1;
        }
        Pattern::Glob(toks)
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Pattern::Substring(s) => s.is_empty(),
            Pattern::Glob(t) => t.is_empty(),
        }
    }

    /// Whether `name` (already lower case) matches.
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Pattern::Substring(s) => name.contains(s.as_str()),
            Pattern::Glob(toks) => glob(toks, &name.chars().collect::<Vec<_>>()),
        }
    }
}

fn one(tok: &Tok, c: char) -> bool {
    match tok {
        Tok::Char(x) => *x == c,
        Tok::Any => true,
        Tok::Class(ranges, negate) => ranges.iter().any(|(a, b)| (*a..=*b).contains(&c)) != *negate,
        Tok::Star => true,
    }
}

/// A glob match with one point to go back to (linear for one star after
/// another - no exponential backtracking).
fn glob(toks: &[Tok], s: &[char]) -> bool {
    let (mut t, mut i) = (0usize, 0usize);
    let mut back: Option<(usize, usize)> = None;
    while i < s.len() {
        if t < toks.len() && toks[t] == Tok::Star {
            back = Some((t, i));
            t += 1;
        } else if t < toks.len() && one(&toks[t], s[i]) {
            t += 1;
            i += 1;
        } else if let Some((bt, bi)) = back {
            t = bt + 1;
            i = bi + 1;
            back = Some((bt, bi + 1));
        } else {
            return false;
        }
    }
    while t < toks.len() && toks[t] == Tok::Star {
        t += 1;
    }
    t == toks.len()
}

/// A search running over the islands' trees.
#[derive(Clone, Debug)]
pub struct Search {
    pub query: String,
    pattern: Pattern,
    /// The hits: (island, node).
    pub hits: Vec<(usize, NodeId)>,
    island: usize,
    /// How far each island's nodes were looked at.
    seen: Vec<usize>,
    pub done: bool,
    pub max_hits: usize,
    /// How many names were looked at.
    pub looked: u64,
}

impl Search {
    pub fn new(query: &str) -> Search {
        Search {
            query: query.to_string(),
            pattern: Pattern::parse(query),
            hits: Vec::new(),
            island: 0,
            seen: Vec::new(),
            done: query.trim().is_empty(),
            // (no limit: every hit is kept - eight bytes each - and the
            // window pages through them)
            max_hits: usize::MAX,
            looked: 0,
        }
    }

    /// Looks at names for at most `budget`; the trees as they are now
    /// (`None` for an island without one).
    pub fn step(&mut self, trees: &[Option<&Tree>], budget: Duration) {
        if self.done || self.pattern.is_empty() {
            self.done = true;
            return;
        }
        if self.seen.len() < trees.len() {
            self.seen.resize(trees.len(), 1);
        }
        let start = Instant::now();
        let mut lower = String::new();
        while self.island < trees.len() {
            let Some(tree) = trees[self.island] else {
                self.island += 1;
                continue;
            };
            let mut next = self.seen[self.island].max(1);
            while next < tree.len() {
                let n = &tree.nodes[next];
                lower.clear();
                for c in n.name.to_string_lossy().chars() {
                    lower.extend(c.to_lowercase());
                }
                if self.pattern.matches(&lower) {
                    self.hits.push((self.island, next as NodeId));
                }
                next += 1;
                self.looked += 1;
                if self.hits.len() >= self.max_hits {
                    self.seen[self.island] = next;
                    self.done = true;
                    return;
                }
                if next.is_multiple_of(2048) && start.elapsed() >= budget {
                    self.seen[self.island] = next;
                    return;
                }
            }
            self.seen[self.island] = next;
            self.island += 1;
        }
        self.done = true;
    }

    /// The trees grew: look at what came since (each island from where it
    /// was left).
    pub fn resume(&mut self) {
        if self.hits.len() < self.max_hits && !self.pattern.is_empty() {
            self.done = false;
            self.island = 0;
        }
    }

    /// An island's tree was replaced (a new walk): its hits go, it is
    /// searched again from its start.
    pub fn reset_island(&mut self, island: usize) {
        self.hits.retain(|(i, _)| *i != island);
        if let Some(s) = self.seen.get_mut(island) {
            *s = 1;
        }
        self.resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Entry, Kind};
    use std::ffi::OsStr;

    fn m(p: &str, name: &str) -> bool {
        Pattern::parse(p).matches(&name.to_lowercase())
    }

    #[test]
    fn substrings_and_globs() {
        assert!(m("report", "Q3_Report.pdf"));
        assert!(!m("report", "Q3_Repo.pdf"));
        assert!(m("*.RS", "main.rs"));
        assert!(!m("*.rs", "main.rsx"));
        assert!(m("ma?n.*", "main.rs"));
        assert!(m("file[0-9][!a-z]*", "file12.txt"));
        assert!(!m("file[0-9][!0-9]*", "file12.txt"));
        assert!(m("*[*", "a[b"));
        assert!(!m("[", "a[b"));
        assert!(m("*a*b*c*", "xxaxxbxxcxx"));
        assert!(!m("*a*b*c*d", "xxaxxbxxcxx"));
        assert!(m("*", ""));
        assert!(!m("?", ""));
        // a pathological pattern stays quick
        let long = "a".repeat(5000);
        let t = Instant::now();
        assert!(!m("*a*a*a*a*a*a*a*a*b", &long));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn searches_across_islands_in_steps() {
        let mut a = Tree::new(OsStr::new("a"), 0);
        let mut b = Tree::new(OsStr::new("b"), 0);
        for i in 0..10_000 {
            let t = if i % 2 == 0 { &mut a } else { &mut b };
            t.push(Entry {
                parent: 0,
                name: OsStr::new(&format!("f{i}.{}", if i % 100 == 0 { "rs" } else { "txt" }))
                    .into(),
                kind: Kind::File,
                size: 0,
                alloc: 0,
                mtime: 0,
            });
        }
        let mut s = Search::new("*.rs");
        let trees = [Some(&a), None, Some(&b)];
        let mut rounds = 0;
        while !s.done {
            s.step(&trees, Duration::from_micros(50));
            rounds += 1;
            assert!(rounds < 100_000);
        }
        assert_eq!(s.hits.len(), 100);
        assert!(s.hits.iter().all(|(i, _)| *i == 0));
        // more arrives: only the new names are looked at
        a.push(Entry {
            parent: 0,
            name: OsStr::new("late.rs").into(),
            kind: Kind::File,
            size: 0,
            alloc: 0,
            mtime: 0,
        });
        s.resume();
        let trees = [Some(&a), None, Some(&b)];
        while !s.done {
            s.step(&trees, Duration::from_millis(5));
        }
        assert_eq!(s.hits.len(), 101);
        assert!(Search::new("  ").done);
    }

    #[test]
    fn every_hit_is_kept() {
        let mut t = Tree::new(std::ffi::OsStr::new("/"), 0);
        for i in 0..12_000u32 {
            t.push(crate::tree::Entry {
                parent: 0,
                name: std::ffi::OsString::from(format!("p{i}.png")).into_boxed_os_str(),
                kind: crate::tree::Kind::File,
                size: 1,
                alloc: 1,
                mtime: 0,
            });
        }
        let mut s = Search::new("*.png");
        while !s.done {
            s.step(&[Some(&t)], Duration::from_millis(50));
        }
        assert_eq!(s.hits.len(), 12_000);
    }
}
