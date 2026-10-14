//! Minimal JSON value and writer (std only). Parsing is done by `tools/bench_compare.py`.

use std::fmt::Write as _;

/// A JSON value. Objects keep insertion order so that files are stable and diffable.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// Integer.
    Int(i64),
    /// Finite floating-point number (non-finite values are written as `null`).
    Num(f64),
    /// String.
    Str(String),
    /// Array.
    Arr(Vec<Json>),
    /// Object.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// String value from anything string-like.
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    /// Object from `(key, value)` pairs.
    pub fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Self {
        Self::Obj(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    /// Pretty-printed text with a trailing newline.
    #[must_use]
    pub fn to_pretty_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Self::Num(x) if x.is_finite() => {
                // `{:?}` prints the shortest representation that round-trips.
                let _ = write!(out, "{x:?}");
            }
            Self::Num(_) => out.push_str("null"),
            Self::Str(s) => write_str(out, s),
            Self::Arr(items) if items.is_empty() => out.push_str("[]"),
            Self::Arr(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push(']');
            }
            Self::Obj(pairs) if pairs.is_empty() => out.push_str("{}"),
            Self::Obj(pairs) => {
                out.push_str("{\n");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    indent(out, depth + 1);
                    write_str(out, k);
                    out.push_str(": ");
                    v.write(out, depth + 1);
                    out.push_str(if i + 1 < pairs.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_nesting() {
        let v = Json::obj([
            ("a", Json::str("q\"\\\n\u{1}")),
            (
                "b",
                Json::Arr(vec![Json::Int(-3), Json::Num(0.5), Json::Null]),
            ),
            ("c", Json::Arr(vec![])),
            ("d", Json::Num(f64::NAN)),
        ]);
        let expected = "{\n  \"a\": \"q\\\"\\\\\\n\\u0001\",\n  \"b\": [\n    -3,\n    0.5,\n    null\n  ],\n  \"c\": [],\n  \"d\": null\n}\n";
        assert_eq!(v.to_pretty_string(), expected);
    }

    #[test]
    fn floats_round_trip() {
        let s = Json::Num(1234.5678e3).to_pretty_string();
        assert_eq!(s.trim().parse::<f64>().unwrap(), 1_234_567.8);
    }
}
