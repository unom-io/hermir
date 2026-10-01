//! Two-level YAML (RPCS3's `config.yml`, Vita3K's) and BML (ares's `settings.bml`) patched in
//! place: a section at column 0, `  Key: value` indented under it, or a key at column 0 on
//! its own. Nothing is parsed beyond the line being changed, so comments, order, quoting and
//! everything else stay as they were.

/// `text` with `key` under `section` set to `value`; `Ok(None)` when it already is. An empty
/// `section` is a top-level key. A missing key goes after the section's last entry, a missing
/// section at the end. `bml` spells a section header without the colon.
pub fn set(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
    bml: bool,
) -> Result<Option<String>, String> {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indent_of = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
    let is_header = |l: &str| -> Option<String> {
        let t = body(l).trim_end();
        if indent_of(l) != 0 || t.is_empty() || t.starts_with('#') {
            return None;
        }
        if bml {
            (!t.contains(':')).then(|| t.to_string())
        } else {
            t.strip_suffix(':').map(str::to_string)
        }
    };

    // Which lines may hold the key, and where a new one goes.
    let (candidates, insert_after, indent, section_missing): (
        Vec<usize>,
        Option<usize>,
        String,
        bool,
    ) = if section.is_empty() {
        let idx: Vec<usize> = (0..lines.len())
            .filter(|&i| indent_of(lines[i]) == 0 && kv(body(lines[i])).is_some())
            .collect();
        (idx, None, String::new(), false)
    } else {
        match (0..lines.len()).find(|&i| is_header(lines[i]).as_deref() == Some(section)) {
            None => (Vec::new(), None, "  ".into(), true),
            Some(h) => {
                let mut end = h + 1;
                while end < lines.len() {
                    let l = lines[end];
                    let blank = body(l).trim().is_empty();
                    if !blank && indent_of(l) == 0 {
                        break;
                    }
                    end += 1;
                }
                let entries: Vec<usize> = (h + 1..end)
                    .filter(|&i| {
                        indent_of(lines[i]) > 0
                            && !body(lines[i]).trim_start().starts_with('#')
                            && kv(body(lines[i])).is_some()
                    })
                    .collect();
                let indent = entries
                    .first()
                    .map(|&i| lines[i][..indent_of(lines[i])].to_string())
                    .unwrap_or_else(|| "  ".into());
                let after = entries.last().copied().unwrap_or(h);
                (entries, Some(after), indent, false)
            }
        }
    };

    for &i in &candidates {
        let l = lines[i];
        let b = body(l);
        let Some((k, v)) = kv(b) else { continue };
        if k.trim() != key {
            continue;
        }
        if v.trim() == value {
            return Ok(None);
        }
        let left = b[..b.len() - v.len()].trim_end_matches([' ', '\t']);
        let sep = if value.is_empty() { "" } else { " " };
        let ending = &l[b.len()..];
        let mut out: String = lines[..i].concat();
        out.push_str(&format!("{left}{sep}{value}{ending}"));
        out.push_str(&lines[i + 1..].concat());
        return Ok(Some(out));
    }

    let line = format!("{indent}{key}: {value}{nl}");
    if section_missing {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(nl);
        }
        let colon = if bml { "" } else { ":" };
        out.push_str(&format!("{section}{colon}{nl}{line}"));
        return Ok(Some(out));
    }
    match insert_after {
        Some(at) => {
            let mut out: String = lines[..=at].concat();
            if !out.ends_with('\n') {
                out.push_str(nl);
            }
            out.push_str(&line);
            out.push_str(&lines[at + 1..].concat());
            Ok(Some(out))
        }
        None => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(nl);
            }
            out.push_str(&line);
            Ok(Some(out))
        }
    }
}

fn body(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

/// `(key, value)` split at the first `: ` (or a trailing `:`), so `Aspect ratio: 16:9` keeps
/// its value whole. `None` for comments and lines without a key.
fn kv(body: &str) -> Option<(&str, &str)> {
    let t = body.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with('-') {
        return None;
    }
    let bytes = body.as_bytes();
    let mut at = None;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' ' || bytes[i + 1] == b'\t') {
            at = Some(i);
            break;
        }
    }
    let i = at?;
    let key = &body[..i];
    if key.trim().is_empty() {
        return None;
    }
    Some((key, body[i + 1..].trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RPCS3: &str = "Core:\n  PPU Decoder: Recompiler (LLVM)\nVideo:\n  Renderer: Vulkan\n  Resolution Scale: 100\n  Aspect ratio: 16:9\n  VSync: false\nAudio:\n  Renderer: Cubeb\n";

    #[test]
    fn replaces_a_nested_value_and_nothing_else() {
        let out = set(RPCS3, "Video", "Resolution Scale", "300", false)
            .unwrap()
            .unwrap();
        assert_eq!(out, RPCS3.replace("Scale: 100", "Scale: 300"));
        assert_eq!(
            set(&out, "Video", "Resolution Scale", "300", false),
            Ok(None)
        );
        let out = set(RPCS3, "Video", "Aspect ratio", "4:3", false)
            .unwrap()
            .unwrap();
        assert!(
            out.contains("  Aspect ratio: 4:3\n  VSync: false\n"),
            "{out}"
        );
        assert!(!out.contains("16:9"));
    }

    #[test]
    fn adds_a_key_after_the_sections_last_entry_and_a_missing_section_at_the_end() {
        let out = set(RPCS3, "Video", "Stretch To Display Area", "true", false)
            .unwrap()
            .unwrap();
        assert!(
            out.contains("  VSync: false\n  Stretch To Display Area: true\nAudio:\n"),
            "{out}"
        );
        let out = set(RPCS3, "System", "License Area", "SCEE", false)
            .unwrap()
            .unwrap();
        assert!(out.ends_with("Audio:\n  Renderer: Cubeb\nSystem:\n  License Area: SCEE\n"));
        assert_eq!(
            set("", "Video", "VSync", "true", false).unwrap().unwrap(),
            "Video:\n  VSync: true\n"
        );
    }

    #[test]
    fn top_level_keys_and_bml_headers() {
        let vita = "resolution-multiplier: 1\nv-sync: true\n";
        assert_eq!(
            set(vita, "", "resolution-multiplier", "2", false)
                .unwrap()
                .unwrap(),
            "resolution-multiplier: 2\nv-sync: true\n"
        );
        assert_eq!(
            set(vita, "", "fullscreen", "true", false).unwrap().unwrap(),
            "resolution-multiplier: 1\nv-sync: true\nfullscreen: true\n"
        );
        let ares = "Video\n  Driver: OpenGL 3.2\n  Blocking: false\n\nAudio\n  Driver: SDL\n";
        let out = set(ares, "Video", "Blocking", "true", true)
            .unwrap()
            .unwrap();
        assert_eq!(out, ares.replace("Blocking: false", "Blocking: true"));
        let out = set(ares, "Boot", "Prefer", "PAL", true).unwrap().unwrap();
        assert!(
            out.ends_with("Audio\n  Driver: SDL\nBoot\n  Prefer: PAL\n"),
            "{out}"
        );
        // A blank line inside the section does not end it; the new key still goes after
        // the last entry, not after the blank.
        let out = set(ares, "Video", "Multiplier", "2", true)
            .unwrap()
            .unwrap();
        assert!(
            out.contains("  Blocking: false\n  Multiplier: 2\n\nAudio\n"),
            "{out}"
        );
    }

    #[test]
    fn keeps_crlf_and_the_sections_own_indentation() {
        let t = "Video:\r\n    VSync: false\r\n";
        let out = set(t, "Video", "Resolution Scale", "200", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "Video:\r\n    VSync: false\r\n    Resolution Scale: 200\r\n"
        );
    }
}
