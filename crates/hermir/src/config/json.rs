//! One top-level key of a JSON object file set in place (Ryujinx's `Config.json`): the value
//! on the key's line replaced, or the key added first. Scalars only; an object or array value
//! is refused. Order, indentation and every other line stay as they were.

/// `text` with top-level `key` set to `value` (a JSON literal, quotes included for a string);
/// `Ok(None)` when it already is. An empty file becomes `{ key: value }`.
pub fn set(text: &str, key: &str, value: &str) -> Result<Option<String>, String> {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if text.trim().is_empty() {
        return Ok(Some(format!("{{{nl}  \"{key}\": {value}{nl}}}{nl}")));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let depths = depth_at_line_start(text);
    let mut first_key_line = None;
    for (i, l) in lines.iter().enumerate() {
        if depths[i] != 1 {
            continue;
        }
        let b = l.trim_end_matches(['\r', '\n']);
        let t = b.trim_start();
        let Some(rest) = t.strip_prefix('"') else {
            continue;
        };
        let Some(k_end) = end_of_string(rest) else {
            continue;
        };
        let k = &rest[..k_end];
        let after = rest[k_end + 1..].trim_start();
        let Some(v) = after.strip_prefix(':') else {
            continue;
        };
        first_key_line.get_or_insert(i);
        if k != key {
            continue;
        }
        let v = v.trim();
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
        let indent = &b[..b.len() - t.len()];
        let ending = &l[b.len()..];
        let mut out: String = lines[..i].concat();
        out.push_str(&format!("{indent}\"{key}\": {value}{comma}{ending}"));
        out.push_str(&lines[i + 1..].concat());
        return Ok(Some(out));
    }
    let Some(brace) = text.find('{') else {
        return Err("not a JSON object".into());
    };
    if !text[..brace].trim().is_empty() {
        return Err("not a JSON object".into());
    }
    match first_key_line {
        Some(i) => {
            let l = lines[i];
            let indent = &l[..l.len() - l.trim_start().len()];
            let mut out: String = lines[..i].concat();
            out.push_str(&format!("{indent}\"{key}\": {value},{nl}"));
            out.push_str(&lines[i..].concat());
            Ok(Some(out))
        }
        None => {
            let close = text.rfind('}').ok_or("not a JSON object")?;
            if !text[brace + 1..close].trim().is_empty() {
                return Err("not a flat JSON object".into());
            }
            Ok(Some(format!(
                "{}{{{nl}  \"{key}\": {value}{nl}}}{}",
                &text[..brace],
                &text[close + 1..]
            )))
        }
    }
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

    #[test]
    fn replaces_top_level_scalars_only() {
        let out = set(RYU, "res_scale", "3").unwrap().unwrap();
        assert!(
            out.contains("  \"res_scale\": 3,\n  \"aspect_ratio\""),
            "{out}"
        );
        assert!(
            out.contains("      \"res_scale\": 9\n"),
            "the nested one stays: {out}"
        );
        assert_eq!(set(&out, "res_scale", "3"), Ok(None));
        let out = set(RYU, "system_region", "\"Europe\"").unwrap().unwrap();
        assert!(
            out.ends_with("  \"system_region\": \"Europe\"\n}\n"),
            "no comma on the last key: {out}"
        );
        let out = set(RYU, "start_fullscreen", "true").unwrap().unwrap();
        assert!(out.contains("\"start_fullscreen\": true,\n"));
        assert!(matches!(set(RYU, "input_config", "1"), Err(e) if e.contains("not a scalar")));
    }

    #[test]
    fn adds_a_missing_key_first_and_grows_an_empty_object() {
        let out = set(RYU, "vsync_mode", "\"Switch\"").unwrap().unwrap();
        assert!(
            out.starts_with("{\n  \"vsync_mode\": \"Switch\",\n  \"version\": 60,\n"),
            "{out}"
        );
        assert_eq!(
            set("{}\n", "start_fullscreen", "true").unwrap().unwrap(),
            "{\n  \"start_fullscreen\": true\n}\n"
        );
        assert_eq!(
            set("", "start_fullscreen", "true").unwrap().unwrap(),
            "{\n  \"start_fullscreen\": true\n}\n"
        );
        assert_eq!(
            set("{\r\n  \"a\": 1\r\n}\r\n", "b", "2").unwrap().unwrap(),
            "{\r\n  \"b\": 2,\r\n  \"a\": 1\r\n}\r\n"
        );
        assert!(set("[1]", "a", "1").is_err());
    }
}
