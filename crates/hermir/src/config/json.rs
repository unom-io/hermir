//! One key of a JSON object file set in place (Ryujinx's `Config.json`, shadPS4's
//! `config.json`): a key of the root object, or of one object directly under it, has the
//! value on its line replaced, or is added first. Scalars only; an object or array value is
//! refused. Order, indentation and every other line stay as they were.

/// `text` with `key` set to `value` (a JSON literal, quotes included for a string) in the
/// root object, or in the object `section` names under it; `Ok(None)` when it already is.
/// A missing key goes first in its object, a missing section first in the root; an empty
/// file becomes an object holding just that.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> Result<Option<String>, String> {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if text.trim().is_empty() {
        return Ok(Some(if section.is_empty() {
            format!("{{{nl}  \"{key}\": {value}{nl}}}{nl}")
        } else {
            format!("{{{nl}  \"{section}\": {{{nl}    \"{key}\": {value}{nl}  }}{nl}}}{nl}")
        }));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let depths = depth_at_line_start(text);
    let brace = text.find('{').ok_or("not a JSON object")?;
    if !text[..brace].trim().is_empty() {
        return Err("not a JSON object".into());
    }
    let root = text[..brace].matches('\n').count();
    let (open, depth) = if section.is_empty() {
        (root, 1)
    } else {
        match find_key(&lines, &depths, root, 1, section) {
            Some((i, v)) if v.starts_with('{') => (i, 2),
            Some(_) => return Err(format!("\"{section}\" is not an object")),
            None => {
                return Ok(Some(insert_first(&lines, &depths, root, 1, nl, |indent| {
                    format!(
                        "{indent}\"{section}\": {{{nl}{indent}  \"{key}\": {value}{nl}{indent}}}"
                    )
                })));
            }
        }
    };
    if let Some((i, v)) = find_key(&lines, &depths, open, depth, key) {
        let (v, comma) = match v.strip_suffix(',') {
            Some(s) => (s.trim_end(), ","),
            None => (v, ""),
        };
        if v.starts_with('{') || v.starts_with('[') || v.is_empty() {
            return Err(format!("\"{key}\" is not a scalar"));
        }
        if v == value {
            return Ok(None);
        }
        let l = lines[i];
        let b = l.trim_end_matches(['\r', '\n']);
        let indent = &b[..b.len() - b.trim_start().len()];
        let ending = &l[b.len()..];
        let mut out: String = lines[..i].concat();
        out.push_str(&format!("{indent}\"{key}\": {value}{comma}{ending}"));
        out.push_str(&lines[i + 1..].concat());
        return Ok(Some(out));
    }
    Ok(Some(insert_first(
        &lines,
        &depths,
        open,
        depth,
        nl,
        |indent| format!("{indent}\"{key}\": {value}"),
    )))
}

/// The key lines of the object opened on line `open`, whose keys sit at `depth`: the line and
/// what follows its colon, trimmed. Stops where the object ends.
fn keys<'a>(
    lines: &'a [&'a str],
    depths: &'a [usize],
    open: usize,
    depth: usize,
) -> impl Iterator<Item = (usize, &'a str, &'a str)> + 'a {
    lines
        .iter()
        .enumerate()
        .skip(open + 1)
        .take_while(move |(i, _)| depths.get(*i).is_some_and(|d| *d >= depth))
        .filter(move |(i, _)| depths[*i] == depth)
        .filter_map(|(i, l)| {
            let t = l.trim_end_matches(['\r', '\n']).trim_start();
            let rest = t.strip_prefix('"')?;
            let k_end = end_of_string(rest)?;
            let after = rest[k_end + 1..].trim_start();
            let v = after.strip_prefix(':')?;
            Some((i, &rest[..k_end], v.trim()))
        })
}

fn find_key<'a>(
    lines: &'a [&'a str],
    depths: &'a [usize],
    open: usize,
    depth: usize,
    key: &str,
) -> Option<(usize, &'a str)> {
    keys(lines, depths, open, depth).find_map(|(i, k, v)| (k == key).then_some((i, v)))
}

/// The text with `make(indent)` as the first entry of the object opened on line `open`.
fn insert_first(
    lines: &[&str],
    depths: &[usize],
    open: usize,
    depth: usize,
    nl: &str,
    make: impl Fn(&str) -> String,
) -> String {
    let indent_of = |l: &str| l[..l.len() - l.trim_start().len()].to_string();
    if let Some((i, _, _)) = keys(lines, depths, open, depth).next() {
        let indent = indent_of(lines[i]);
        let mut out: String = lines[..i].concat();
        out.push_str(&format!("{},{nl}", make(&indent)));
        out.push_str(&lines[i..].concat());
        return out;
    }
    // An empty object: `{}` on the open line, or `{` with its `}` further down.
    let open_line = lines[open];
    let outer = indent_of(open_line);
    let inner = format!("{outer}  ");
    let mut out: String = lines[..open].concat();
    if let Some(at) = open_line.find("{}") {
        out.push_str(&open_line[..at]);
        out.push_str(&format!("{{{nl}{}{nl}{outer}}}", make(&inner)));
        out.push_str(&open_line[at + 2..]);
    } else {
        out.push_str(open_line);
        if !open_line.ends_with('\n') {
            out.push_str(nl);
        }
        out.push_str(&format!("{}{nl}", make(&inner)));
    }
    out.push_str(&lines[open + 1..].concat());
    out
}

/// Nesting depth at the start of each line, strings and escapes respected.
fn depth_at_line_start(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for c in text.chars() {
        match (in_str, c) {
            (true, '\\') if !esc => esc = true,
            (true, '"') if !esc => in_str = false,
            (true, _) => esc = false,
            (false, '"') => in_str = true,
            (false, '{' | '[') => depth += 1,
            (false, '}' | ']') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if c == '\n' {
            out.push(depth);
        }
    }
    out
}

/// Index of the closing quote of a string that starts right after an opening one.
fn end_of_string(s: &str) -> Option<usize> {
    let mut esc = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if !esc => esc = true,
            '"' if !esc => return Some(i),
            _ => esc = false,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
