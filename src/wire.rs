//! Answers of axross's MCP server read into typed, bounded shapes: lists
//! keep at most N items (the rest is skipped without being built), strings
//! are cut at their limit, fields of the wrong type count as missing, and
//! nothing is ever held as a generic JSON tree - a 31 MiB answer costs about
//! its own size, not gigabytes. (serde_json itself limits nesting to 128
//! levels.)

use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;
use std::marker::PhantomData;

/// Drains whatever value comes (a map, a list, anything) without building
/// it.
fn skip<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

fn skip_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}

/// A string of at most `MAX` bytes (cut at a character), or none when the
/// field is missing, null or of another type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text<const MAX: usize>(pub Option<String>);

impl<const MAX: usize> Text<MAX> {
    pub fn get(&self) -> Option<&str> {
        self.0.as_deref()
    }
    pub fn into_inner(self) -> Option<String> {
        self.0
    }
}

fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut k = max;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        s.truncate(k);
    }
    s
}

impl<'de, const MAX: usize> Deserialize<'de> for Text<MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const MAX: usize>;
        impl<'de, const MAX: usize> Visitor<'de> for V<MAX> {
            type Value = Text<MAX>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                let end = if v.len() > MAX {
                    let mut k = MAX;
                    while !v.is_char_boundary(k) {
                        k -= 1;
                    }
                    k
                } else {
                    v.len()
                };
                Ok(Text(Some(v[..end].to_string())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(Text(Some(cut(v, MAX))))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Text(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                skip(seq).map(|_| Text(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                skip_map(map).map(|_| Text(None))
            }
        }
        d.deserialize_any(V::<MAX>)
    }
}

/// A whole number not below zero, or none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Num(pub Option<u64>);

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Num;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number")
            }
            fn visit_u64<E>(self, v: u64) -> Result<Num, E> {
                Ok(Num(Some(v)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Num, E> {
                Ok(Num(u64::try_from(v).ok()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Num, E> {
                Ok(Num((0.0..1.8e19).contains(&v).then_some(v as u64)))
            }
            fn visit_str<E>(self, _: &str) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_unit<E>(self) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_none<E>(self) -> Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Num, A::Error> {
                skip(seq).map(|_| Num(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Num, A::Error> {
                skip_map(map).map(|_| Num(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// A yes or no, or none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Flag(pub Option<bool>);

impl<'de> Deserialize<'de> for Flag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Flag;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a boolean")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Flag, E> {
                Ok(Flag(Some(v)))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_unit<E>(self) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_none<E>(self) -> Result<Flag, E> {
                Ok(Flag(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Flag, A::Error> {
                skip(seq).map(|_| Flag(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Flag, A::Error> {
                skip_map(map).map(|_| Flag(None))
            }
        }
        d.deserialize_any(V)
    }
}

impl Flag {
    pub fn or(self, d: bool) -> bool {
        self.0.unwrap_or(d)
    }
}

/// One item of a list: an object read as `T` (whose fields never fail),
/// anything else skipped (None).
#[derive(Debug)]
pub struct Item<T>(pub Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Item<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Item<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Item<T>, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(|t| Item(Some(t)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Item<T>, A::Error> {
                skip(seq).map(|_| Item(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
            fn visit_unit<E>(self) -> Result<Item<T>, E> {
                Ok(Item(None))
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}

/// A list of at most `N` items; what comes after is skipped without being
/// built, and counted (with the items that were no object).
#[derive(Debug)]
pub struct Bounded<T, const N: usize> {
    pub items: Vec<T>,
    pub dropped: usize,
}

impl<T, const N: usize> Default for Bounded<T, N> {
    fn default() -> Self {
        Bounded {
            items: Vec::new(),
            dropped: 0,
        }
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
            type Value = Bounded<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Bounded::<T, N>::default();
                loop {
                    if out.items.len() < N {
                        match seq.next_element::<Item<T>>()? {
                            Some(Item(Some(t))) => out.items.push(t),
                            Some(Item(None)) => out.dropped += 1,
                            None => break,
                        }
                    } else {
                        match seq.next_element::<IgnoredAny>()? {
                            Some(_) => out.dropped += 1,
                            None => break,
                        }
                    }
                }
                Ok(out)
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                skip_map(map).map(|_| Bounded::default())
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Bounded::default())
            }
        }
        d.deserialize_any(V::<T, N>(PhantomData))
    }
}

/// A server string made safe to show: control characters, invisible and
/// bidirectional formatting and a byte-order mark escaped (`\u{…}`), cut at
/// `max` characters.
pub fn shown(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max * 4));
    for (n, c) in s.chars().enumerate() {
        if n >= max {
            out.push('…');
            break;
        }
        if c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
        {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a name can stand as it is (no control, invisible or
/// bidirectional character, not blank).
pub fn plain_name(s: &str) -> bool {
    !s.trim().is_empty()
        && !s.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Default, Debug)]
    #[serde(default)]
    struct P {
        name: Text<8>,
        port: Num,
        on: Flag,
    }

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct L {
        items: Bounded<P, 3>,
    }

    #[test]
    fn wrong_types_are_missing_and_lists_are_bounded() {
        let l: L = serde_json::from_str(
            r#"{"items": [{"name": "abcdefghijk", "port": "x", "on": 1}, 5, {"name": null, "port": 22, "on": true}, {"name": [1,2]}, {}, {}, {}], "extra": {"deep": [1,2,3]}}"#,
        )
        .unwrap();
        assert_eq!(l.items.items.len(), 3);
        assert_eq!(l.items.items[0].name.get(), Some("abcdefgh"));
        assert_eq!(l.items.items[0].port, Num(None));
        assert_eq!(l.items.items[1].port, Num(Some(22)));
        assert_eq!(l.items.items[1].on, Flag(Some(true)));
        assert_eq!(l.items.dropped, 4, "one no object, three past the bound");
    }

    #[test]
    fn four_million_tiny_objects_cost_little() {
        // (31 MiB of `{"a":1}`: kept 3, the rest skipped unbuilt)
        let mut s = String::from(r#"{"items": ["#);
        for i in 0..4_000_000 {
            if i > 0 {
                s.push(',');
            }
            s.push_str(r#"{"a":1}"#);
        }
        s.push_str("]}");
        let t = std::time::Instant::now();
        let l: L = serde_json::from_str(&s).unwrap();
        assert_eq!(l.items.items.len(), 3);
        assert_eq!(l.items.dropped, 4_000_000 - 3);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn strings_are_escaped_and_cut() {
        assert_eq!(
            shown("a\nb\u{202e}c\u{feff}", 80),
            "a\\u{a}b\\u{202e}c\\u{feff}"
        );
        assert_eq!(shown("abcdef", 3), "abc…");
        assert!(!plain_name(" "));
        assert!(!plain_name("\u{feff}box"));
        assert!(!plain_name("a\u{200b}b"));
        assert!(plain_name("日本のサーバー 🚀"));
    }
}
