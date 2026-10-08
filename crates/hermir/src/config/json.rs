//! One key of a JSON object file set in place (Ryujinx's `Config.json`, shadPS4's
//! `config.json`): a key of the root object, or of one object directly under it. A byte
//! scanner that knows strings, escapes and nesting finds each member's key and value span, so
//! compact, pretty and mixed files are all the same to it. Only the value's bytes change, or
//! one member is added first in its object, laid out like its siblings. Scalars only; an
//! object or array value is refused.
use std::ops::Range;

use serde_json::Value;

use super::ini::{newline, split_bom};

/// `text` with `key` set to `value` (a JSON literal, quotes included for a string) in the
/// root object, or in the object `section` names under it; `Ok(None)` when it already is.
/// A missing key goes first in its object, a missing section first in the root; an empty
/// file becomes an object holding just that.
///
/// `value` must be one JSON scalar (`1`, `true`, `null`, `"text"`; see [`string`]), so it
/// can never add a member of its own. A file that is not valid JSON is refused.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> Result<Option<String>, String> {
    let value = scalar(value)?;
    let (bom, text) = split_bom(text);
    let nl = newline(text);
    if text.trim().is_empty() {
        let k = string(key);
        return Ok(Some(if section.is_empty() {
            format!("{bom}{{{nl}  {k}: {value}{nl}}}{nl}")
        } else {
            let s = string(section);
            format!("{bom}{{{nl}  {s}: {{{nl}    {k}: {value}{nl}  }}{nl}}}{nl}")
        }));
    }
    serde_json::from_str::<Value>(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let root = root(text).ok_or("not a JSON object")??;
    let style = Style::of(text, &root, nl);
    let done = |out: String| Ok(Some(format!("{bom}{out}")));
    let inner;
    let target = if section.is_empty() {
        &root
    } else {
        match find(text, &root, section) {
            Some(m) if text.as_bytes()[m.value.start] == b'{' => {
                inner = object(text, m.value.start)?;
                &inner
            }
            Some(_) => return Err(format!("\"{section}\" is not an object")),
            None => {
                let new = New {
                    name: section,
                    inner: Some(key),
                    value,
                };
                return done(insert_first(text, &root, &root, &style, &new));
            }
        }
    };
    match find(text, target, key) {
        Some(m) => {
            let old = &text[m.value.clone()];
            if old.starts_with(['{', '[']) {
                return Err(format!("\"{key}\" is not a scalar"));
            }
            if old == value {
                return Ok(None);
            }
            done(format!(
                "{}{value}{}",
                &text[..m.value.start],
                &text[m.value.end..]
            ))
        }
        None => {
            let new = New {
                name: key,
                inner: None,
                value,
            };
            done(insert_first(text, target, &root, &style, &new))
        }
    }
}

/// `text` with the root member `key` replaced by `value` whole, an array or object included:
/// an adapter's own structure (Ryujinx's `input_config`), never a knob's. Written on one line;
/// `Ok(None)` when the file already holds an equal value. A file that is not valid JSON is
/// refused.
pub fn set_value(text: &str, key: &str, value: &Value) -> Result<Option<String>, String> {
    let (bom, text) = split_bom(text);
    let nl = newline(text);
    let compact = value.to_string();
    if text.trim().is_empty() {
        let k = string(key);
        return Ok(Some(format!("{bom}{{{nl}  {k}: {compact}{nl}}}{nl}")));
    }
    serde_json::from_str::<Value>(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let root = root(text).ok_or("not a JSON object")??;
    let out = match find(text, &root, key) {
        Some(m) => {
            let old = serde_json::from_str::<Value>(&text[m.value.clone()]).ok();
            if old.as_ref() == Some(value) {
                return Ok(None);
            }
            format!(
                "{}{compact}{}",
                &text[..m.value.start],
                &text[m.value.end..]
            )
        }
        None => {
            let style = Style::of(text, &root, nl);
            let new = New {
                name: key,
                inner: None,
                value: &compact,
            };
            insert_first(text, &root, &root, &style, &new)
        }
    };
    Ok(Some(format!("{bom}{out}")))
}

/// The value of `key` in the root object, or in the object `section` names under it, as the
/// file spells it: a string with its quotes, an object or array whole. `None` when it is
/// missing.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    let (_, text) = split_bom(text);
    let root = root(text)?.ok()?;
    let target = if section.is_empty() {
        root
    } else {
        let m = find(text, &root, section)?;
        if text.as_bytes()[m.value.start] != b'{' {
            return None;
        }
        object(text, m.value.start).ok()?
    };
    find(text, &target, key).map(|m| text[m.value.clone()].to_string())
}

/// `s` as a JSON string literal, quoted and escaped.
pub fn string(s: &str) -> String {
    Value::from(s).to_string()
}

/// `value`, trimmed, when it is one JSON scalar.
fn scalar(value: &str) -> Result<&str, String> {
    let v = value.trim();
    match serde_json::from_str::<Value>(v) {
        Ok(Value::Array(_) | Value::Object(_)) => Err(format!("{v} is not a JSON scalar")),
        Ok(_) => Ok(v),
        Err(_) => Err(format!("{v:?} is not one JSON value")),
    }
}

/// One member of an object.
struct Member {
    /// The key, quotes included.
    key: Range<usize>,
    value: Range<usize>,
}

/// An object: where its braces are, and its members in order.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// The root object; `None` when the text holds something else.
fn root(text: &str) -> Option<Result<Object, String>> {
    let at = skip_ws(text.as_bytes(), 0);
    (text.as_bytes().get(at) == Some(&b'{')).then(|| object(text, at))
}

/// The object whose `{` is at `open`.
fn object(text: &str, open: usize) -> Result<Object, String> {
    let s = text.as_bytes();
    let bad = |at: usize| format!("unexpected JSON at byte {at}");
    let mut members = Vec::new();
    let mut i = skip_ws(s, open + 1);
    if s.get(i) == Some(&b'}') {
        return Ok(Object {
            open,
            close: i,
            members,
        });
    }
    loop {
        if s.get(i) != Some(&b'"') {
            return Err(bad(i));
        }
        let key = i..string_end(s, i).ok_or_else(|| bad(i))?;
        i = skip_ws(s, key.end);
        if s.get(i) != Some(&b':') {
            return Err(bad(i));
        }
        i = skip_ws(s, i + 1);
        let value = i..value_end(s, i).ok_or_else(|| bad(i))?;
        i = skip_ws(s, value.end);
        members.push(Member { key, value });
        match s.get(i) {
            Some(b',') => i = skip_ws(s, i + 1),
            Some(b'}') => {
                return Ok(Object {
                    open,
                    close: i,
                    members,
                });
            }
            _ => return Err(bad(i)),
        }
    }
}

/// The last member of `obj` named `name`, as a reader that takes the last duplicate sees it.
fn find<'o>(text: &str, obj: &'o Object, name: &str) -> Option<&'o Member> {
    obj.members
        .iter()
        .rev()
        .find(|m| serde_json::from_str::<String>(&text[m.key.clone()]).is_ok_and(|k| k == name))
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while matches!(s.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// One past the closing quote of the string that opens at `i`.
fn string_end(s: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while let Some(&c) = s.get(j) {
        match c {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// One past the end of the value that starts at `i`.
fn value_end(s: &[u8], i: usize) -> Option<usize> {
    match *s.get(i)? {
        b'"' => string_end(s, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while let Some(&c) = s.get(j) {
                match c {
                    b'"' => {
                        j = string_end(s, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let n = s[i..]
                .iter()
                .take_while(|c| !matches!(c, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                .count();
            (n > 0).then_some(i + n)
        }
    }
}

/// How the file lays out its members.
struct Style {
    nl: &'static str,
    /// One step of indentation.
    unit: String,
    /// Members on lines of their own, where the root has them (or is empty).
    pretty: bool,
}

impl Style {
    fn of(text: &str, root: &Object, nl: &'static str) -> Style {
        let unit = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
            .filter(|ws| !ws.is_empty())
            .min_by_key(|ws| ws.len())
            .unwrap_or("  ")
            .to_string();
        let pretty = root.members.is_empty() || text[root.open..root.close].contains('\n');
        Style { nl, unit, pretty }
    }
}

/// A member to add: `"name": value`, or `"name": {"inner": value}` for a new section.
struct New<'a> {
    name: &'a str,
    inner: Option<&'a str>,
    value: &'a str,
}

impl New<'_> {
    /// The member on lines of its own from `indent`, or inline when there is none.
    fn render(&self, colon: &str, style: &Style, indent: Option<&str>) -> String {
        let name = string(self.name);
        let value = self.value;
        match (self.inner, indent) {
            (None, Some(i)) => format!("{i}{name}{colon}{value}"),
            (None, None) => format!("{name}{colon}{value}"),
            (Some(k), Some(i)) => {
                let (nl, unit, k) = (style.nl, &style.unit, string(k));
                format!("{i}{name}{colon}{{{nl}{i}{unit}{k}{colon}{value}{nl}{i}}}")
            }
            (Some(k), None) => format!("{name}{colon}{{{}{colon}{value}}}", string(k)),
        }
    }
}

/// The text with `new` as the first member of `obj`, laid out like the members it has, or,
/// in an empty object, like the file.
fn insert_first(text: &str, obj: &Object, root: &Object, style: &Style, new: &New) -> String {
    let line_start = |pos: usize| text[..pos].rfind('\n').map_or(0, |n| n + 1);
    let indent_at = |pos: usize| {
        let l = &text[line_start(pos)..];
        &l[..l.len() - l.trim_start_matches([' ', '\t']).len()]
    };
    // `:` as the file spells it: `": "`, `":"`…
    let colon = obj
        .members
        .first()
        .or(root.members.first())
        .map_or(": ", |m| &text[m.key.end..m.value.start]);
    let nl = style.nl;
    if let Some(first) = obj.members.first() {
        let ls = line_start(first.key.start);
        let lead = &text[ls..first.key.start];
        if ls > obj.open && lead.chars().all(|c| c == ' ' || c == '\t') {
            // Members on lines of their own: one more, indented like them.
            let member = new.render(colon, style, Some(lead));
            return format!("{}{member},{nl}{}", &text[..ls], &text[ls..]);
        }
        // Members inline: one more, with the separator they have.
        let sep = match obj.members.get(1) {
            Some(second) => &text[first.value.end..second.key.start],
            None if colon.ends_with(' ') => ", ",
            None => ",",
        };
        let member = new.render(colon, style, None);
        let at = first.key.start;
        return format!("{}{member}{sep}{}", &text[..at], &text[at..]);
    }
    let outer = indent_at(obj.open);
    let inner = format!("{outer}{}", style.unit);
    if text[obj.open..obj.close].contains('\n') {
        // `{` and `}` on lines of their own: the member on one between them.
        let ls = line_start(obj.close);
        let member = new.render(colon, style, Some(&inner));
        return format!("{}{member}{nl}{}", &text[..ls], &text[ls..]);
    }
    let (head, tail) = (&text[..obj.open], &text[obj.close + 1..]);
    if style.pretty {
        let member = new.render(colon, style, Some(&inner));
        format!("{head}{{{nl}{member}{nl}{outer}}}{tail}")
    } else {
        format!("{head}{{{}}}{tail}", new.render(colon, style, None))
    }
}

#[cfg(test)]
mod tests {
    use super::super::ini::testkit::{changed, same_endings};
    use super::*;
    use proptest::prelude::*;
    use serde_json::{Map, json};

    const RYU: &str = "{\n  \"version\": 60,\n  \"res_scale\": 1,\n  \"aspect_ratio\": \"Fixed16x9\",\n  \"start_fullscreen\": false,\n  \"input_config\": [\n    {\n      \"res_scale\": 9\n    }\n  ],\n  \"system_region\": \"USA\"\n}\n";
    const SHAD: &str = "{\n  \"General\": {\n    \"volume_slider\": 100\n  },\n  \"GPU\": {\n    \"vblank_frequency\": 60,\n    \"full_screen\": false,\n    \"full_screen_mode\": \"Windowed\"\n  }\n}\n";

    #[test]
    fn replaces_top_level_scalars_only() {
        let out = set(RYU, "", "res_scale", "3").unwrap().unwrap();
        assert!(
            out.contains("  \"res_scale\": 3,\n  \"aspect_ratio\""),
            "{out}"
        );
        assert!(
            out.contains("      \"res_scale\": 9\n"),
            "the nested one stays: {out}"
        );
        assert_eq!(set(&out, "", "res_scale", "3"), Ok(None));
        let out = set(RYU, "", "system_region", "\"Europe\"")
            .unwrap()
            .unwrap();
        assert!(
            out.ends_with("  \"system_region\": \"Europe\"\n}\n"),
            "no comma on the last key: {out}"
        );
        let out = set(RYU, "", "start_fullscreen", "true").unwrap().unwrap();
        assert!(out.contains("\"start_fullscreen\": true,\n"));
        assert!(matches!(set(RYU, "", "input_config", "1"), Err(e) if e.contains("not a scalar")));
        assert!(
            matches!(set(RYU, "input_config", "x", "1"), Err(e) if e.contains("not an object"))
        );
    }

    #[test]
    fn adds_a_missing_key_first_and_grows_an_empty_object() {
        let out = set(RYU, "", "vsync_mode", "\"Switch\"").unwrap().unwrap();
        assert!(
            out.starts_with("{\n  \"vsync_mode\": \"Switch\",\n  \"version\": 60,\n"),
            "{out}"
        );
        assert_eq!(
            set("{}\n", "", "start_fullscreen", "true")
                .unwrap()
                .unwrap(),
            "{\n  \"start_fullscreen\": true\n}\n"
        );
        assert_eq!(
            set("", "", "start_fullscreen", "true").unwrap().unwrap(),
            "{\n  \"start_fullscreen\": true\n}\n"
        );
        assert_eq!(
            set("{\r\n  \"a\": 1\r\n}\r\n", "", "b", "2")
                .unwrap()
                .unwrap(),
            "{\r\n  \"b\": 2,\r\n  \"a\": 1\r\n}\r\n"
        );
        assert!(set("[1]", "", "a", "1").is_err());
    }

    #[test]
    fn keys_of_one_object_under_the_root() {
        let out = set(SHAD, "GPU", "full_screen", "true").unwrap().unwrap();
        assert_eq!(
            out,
            SHAD.replace("\"full_screen\": false", "\"full_screen\": true")
        );
        assert_eq!(set(&out, "GPU", "full_screen", "true"), Ok(None));
        let out = set(SHAD, "GPU", "present_mode", "\"Fifo\"")
            .unwrap()
            .unwrap();
        assert!(
            out.contains(
                "  \"GPU\": {\n    \"present_mode\": \"Fifo\",\n    \"vblank_frequency\": 60,\n"
            ),
            "{out}"
        );
        let out = set(SHAD, "Input", "cursor_state", "1").unwrap().unwrap();
        assert!(
            out.starts_with("{\n  \"Input\": {\n    \"cursor_state\": 1\n  },\n  \"General\": {\n"),
            "{out}"
        );
        assert_eq!(
            set("{\n  \"GPU\": {}\n}\n", "GPU", "full_screen", "true")
                .unwrap()
                .unwrap(),
            "{\n  \"GPU\": {\n    \"full_screen\": true\n  }\n}\n"
        );
        assert_eq!(
            set("{\n  \"GPU\": {\n  }\n}\n", "GPU", "full_screen", "true")
                .unwrap()
                .unwrap(),
            "{\n  \"GPU\": {\n    \"full_screen\": true\n  }\n}\n"
        );
        assert_eq!(
            set("", "GPU", "full_screen", "true").unwrap().unwrap(),
            "{\n  \"GPU\": {\n    \"full_screen\": true\n  }\n}\n"
        );
        // The same key name in another object is not the one asked for.
        let out = set(SHAD, "General", "full_screen", "true")
            .unwrap()
            .unwrap();
        assert!(
            out.contains(
                "  \"General\": {\n    \"full_screen\": true,\n    \"volume_slider\": 100\n"
            ),
            "{out}"
        );
        assert!(out.contains("    \"full_screen\": false,\n"), "{out}");
    }

    #[test]
    fn compact_and_one_line_objects_stay_valid() {
        let t = "{\"GPU\": {\"full_screen\": false}}";
        let out = set(t, "GPU", "full_screen", "true").unwrap().unwrap();
        assert_eq!(out, "{\"GPU\": {\"full_screen\": true}}");
        let out = set(t, "GPU", "vsync", "1").unwrap().unwrap();
        assert_eq!(out, "{\"GPU\": {\"vsync\": 1, \"full_screen\": false}}");
        let out = set(t, "Input", "x", "1").unwrap().unwrap();
        assert_eq!(
            out,
            "{\"Input\": {\"x\": 1}, \"GPU\": {\"full_screen\": false}}"
        );
        let out = set(t, "", "version", "2").unwrap().unwrap();
        assert_eq!(out, "{\"version\": 2, \"GPU\": {\"full_screen\": false}}");
        assert_eq!(
            set("{\"a\":1,\"b\":2}", "", "c", "3").unwrap().unwrap(),
            "{\"c\":3,\"a\":1,\"b\":2}"
        );
        assert_eq!(
            set("{\"a\":1}", "", "c", "3").unwrap().unwrap(),
            "{\"c\":3,\"a\":1}"
        );
        assert_eq!(
            set("{\"GPU\":{}}", "GPU", "x", "true").unwrap().unwrap(),
            "{\"GPU\":{\"x\":true}}"
        );
        assert_eq!(
            set("{}", "", "a", "1").unwrap().unwrap(),
            "{\n  \"a\": 1\n}"
        );
        // Mixed: the first member on the brace's line, the rest on their own.
        assert_eq!(
            set("{\"a\": 1,\n \"b\": 2}", "", "c", "3")
                .unwrap()
                .unwrap(),
            "{\"c\": 3,\n \"a\": 1,\n \"b\": 2}"
        );
        for out in [
            set(
                "{\"a\":[1,{\"b\":\"}\"}],\"c\":{\"d\":\"x\\\"y\"}}",
                "c",
                "d",
                "null",
            ),
            set("  {\"s\": \"{[\\\\\"}  ", "", "s", "\"v\""),
        ] {
            let out = out.unwrap().unwrap();
            assert!(serde_json::from_str::<Value>(&out).is_ok(), "{out}");
        }
    }

    #[test]
    fn an_adapter_replaces_a_whole_array_in_place() {
        let v: Value =
            serde_json::from_str(r#"[{"id":"0-x","led":{"enable_led":false}}]"#).unwrap();
        let out = set_value(RYU, "input_config", &v).unwrap().unwrap();
        let doc: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["input_config"], v);
        assert_eq!(doc["version"], 60, "the rest stays");
        assert!(out.starts_with("{\n  \"version\": 60,\n"), "{out}");
        assert_eq!(set_value(&out, "input_config", &v), Ok(None));
        let added = set_value("{\n  \"a\": 1\n}\n", "b", &v).unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&added).unwrap()["b"], v);
        assert!(set_value("[1]", "b", &v).is_err());
    }

    #[test]
    fn only_one_scalar_is_a_value() {
        for bad in [
            "1, \"evil\": true",
            "[1]",
            "{}",
            "{\"a\": 1}",
            "",
            "x",
            "1 2",
            "\"a\"\n,\"b\": 1",
            "'a'",
        ] {
            assert!(set(SHAD, "GPU", "full_screen", bad).is_err(), "{bad:?}");
        }
        for good in ["1", "-1.5e3", "true", "null", "\"a, \\\"b\\\": c\"", " 2 "] {
            let out = set(SHAD, "GPU", "full_screen", good).unwrap().unwrap();
            assert!(serde_json::from_str::<Value>(&out).is_ok(), "{out}");
            assert_eq!(
                get(&out, "GPU", "full_screen").as_deref(),
                Some(good.trim())
            );
        }
        assert!(set("{\"a\": 1,}", "", "a", "2").is_err(), "not JSON");
        assert!(set("{\"a\": 1} x", "", "a", "2").is_err(), "not JSON");
    }

    #[test]
    fn get_reads_the_literal_as_written() {
        assert_eq!(get(RYU, "", "version").as_deref(), Some("60"));
        assert_eq!(
            get(RYU, "", "aspect_ratio").as_deref(),
            Some("\"Fixed16x9\"")
        );
        assert_eq!(get(RYU, "", "nope"), None);
        assert_eq!(get(SHAD, "GPU", "full_screen").as_deref(), Some("false"));
        assert_eq!(get(SHAD, "General", "full_screen"), None);
        assert_eq!(get(SHAD, "Nope", "full_screen"), None);
        assert_eq!(get(RYU, "version", "x"), None);
        assert!(get(RYU, "", "input_config").unwrap().starts_with('['));
        let t = "\u{feff}{\"k\\u0065y\": 1, \"key\": 2}";
        assert_eq!(get(t, "", "key").as_deref(), Some("2"), "the last of two");
        assert_eq!(
            set(t, "", "key", "3").unwrap().unwrap(),
            "\u{feff}{\"k\\u0065y\": 1, \"key\": 3}"
        );
    }

    #[test]
    fn names_are_escaped_and_strings_quoted() {
        assert_eq!(string("a \"b\"\n\\"), "\"a \\\"b\\\"\\n\\\\\"");
        let out = set("{}", "a\"b", "c\nd", &string("x\"y")).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, json!({"a\"b": {"c\nd": "x\"y"}}));
        assert_eq!(get(&out, "a\"b", "c\nd"), Some(string("x\"y")));
    }

    // Property tests over generated files: a root with scalars and objects under it (holding
    // scalars, arrays and objects whose keys repeat the names asked for), written pretty,
    // compact or mixed, LF or CRLF.

    fn scalar_value() -> impl Strategy<Value = Value> {
        prop_oneof![
            any::<bool>().prop_map(Value::from),
            any::<i32>().prop_map(Value::from),
            Just(Value::Null),
            "\\PC{0,6}".prop_map(Value::from),
        ]
    }

    const KEY: &str = "(a|b|res_scale|full_screen|GPU|x y)";

    fn section() -> impl Strategy<Value = Value> {
        prop::collection::btree_map(
            KEY,
            prop_oneof![
                4 => scalar_value(),
                1 => prop::collection::vec(scalar_value(), 0..3).prop_map(Value::from),
                1 => prop::collection::btree_map(KEY, scalar_value(), 0..3)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ],
            0..4,
        )
        .prop_map(|m| Value::Object(m.into_iter().collect()))
    }

    fn doc() -> impl Strategy<Value = Map<String, Value>> {
        prop::collection::btree_map(
            "(version|res_scale|full_screen|General|GPU|Input)",
            prop_oneof![2 => scalar_value(), 1 => section()],
            0..5,
        )
        .prop_map(|m| m.into_iter().collect())
    }

    /// `v` on one line with a space after each `:` and `,`.
    fn spaced(v: &Value) -> String {
        match v {
            Value::Object(m) => {
                let members: Vec<String> = m
                    .iter()
                    .map(|(k, v)| format!("{}: {}", string(k), spaced(v)))
                    .collect();
                format!("{{{}}}", members.join(", "))
            }
            Value::Array(a) => {
                let items: Vec<String> = a.iter().map(spaced).collect();
                format!("[{}]", items.join(", "))
            }
            v => v.to_string(),
        }
    }

    /// `doc` pretty, compact, one member a line with the rest compact, or spaced on one line.
    fn render(doc: &Map<String, Value>, style: u8, crlf: bool) -> String {
        let v = Value::Object(doc.clone());
        let text = match style {
            0 => serde_json::to_string_pretty(&v).unwrap(),
            1 => v.to_string(),
            2 => {
                let members: Vec<String> = doc
                    .iter()
                    .map(|(k, v)| format!("\t{}:{}", string(k), v))
                    .collect();
                format!("{{\n{}\n}}", members.join(",\n"))
            }
            _ => spaced(&v),
        };
        if crlf {
            text.replace('\n', "\r\n")
        } else {
            text
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn set_changes_one_member_and_the_file_stays_json(
            d in doc(), style in 0u8..4, crlf in any::<bool>(),
            section_pick in any::<usize>(), key in KEY, v in scalar_value(),
        ) {
            let text = render(&d, style, crlf);
            let sections: Vec<&String> = d.iter().filter(|(_, v)| v.is_object()).map(|(k, _)| k).collect();
            let section = match section_pick % (sections.len() + 2) {
                0 => String::new(),
                1 => "Fresh".to_string(),
                n => sections[n - 2].clone(),
            };
            let literal = v.to_string();
            let mut expected = d.clone();
            let slot = if section.is_empty() {
                Some(&mut expected)
            } else {
                expected
                    .entry(section.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
            };
            let old = slot.as_ref().and_then(|m| m.get(&key)).cloned();
            let r = set(&text, &section, &key, &literal);
            if old.as_ref().is_some_and(|o| o.is_object() || o.is_array()) {
                prop_assert!(r.is_err());
                return Ok(());
            }
            let r = r.unwrap();
            if let Some(m) = slot {
                m.insert(key.clone(), v.clone());
            }
            let out = r.clone().unwrap_or_else(|| text.clone());
            let parsed: Value = serde_json::from_str(&out).unwrap();
            prop_assert_eq!(parsed, Value::Object(expected));
            // Of the file's bytes only the old literal goes, or, for a new member, at most the
            // blank inside an empty `{ }`.
            let (a, b) = (text.as_bytes(), out.as_bytes());
            let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            let tail = a[head..]
                .iter()
                .rev()
                .zip(b[head..].iter().rev())
                .take_while(|(x, y)| x == y)
                .count();
            let gone = &a[head..a.len() - tail];
            match &old {
                Some(o) => prop_assert!(gone.len() <= o.to_string().len()),
                None => prop_assert!(gone.iter().all(u8::is_ascii_whitespace)),
            }
            let back = get(&out, &section, &key);
            prop_assert_eq!(back.as_deref(), Some(literal.as_str()));
            prop_assert_eq!(set(&out, &section, &key, &literal), Ok(None));
            let (gone, new) = changed(&text, &out);
            if style == 0 && r.is_some() && !d.is_empty() {
                let in_section = d.get(&section).and_then(Value::as_object);
                let counts = (gone.len(), new.len());
                match (old.is_some(), section.is_empty(), in_section) {
                    (true, ..) => prop_assert_eq!(counts, (1, 1)),
                    (false, true, _) => prop_assert_eq!(counts, (0, 1)),
                    (false, false, Some(m)) if m.is_empty() => prop_assert_eq!(counts, (1, 3)),
                    (false, false, Some(_)) => prop_assert_eq!(counts, (0, 1)),
                    (false, false, None) => prop_assert_eq!(counts, (0, 3)),
                }
            }
            prop_assert!(same_endings(&text, &out));
        }

        #[test]
        fn nothing_but_one_scalar_is_written(
            d in doc(), style in 0u8..4, key in KEY,
            junk in prop_oneof![
                "[0-9]{1,3}, \"[a-z]{1,4}\": (true|1)",
                "\\[[0-9]{0,2}\\]",
                "\\{\\}",
                "[a-z]{1,5}",
                "\"[a-z]{0,3}\" *, *\"[a-z]{1,3}\"",
            ],
        ) {
            let text = render(&d, style, false);
            prop_assert!(set(&text, "", &key, &junk).is_err());
            prop_assert!(set(&text, "GPU", &key, &junk).is_err());
        }
    }
}
