//! Two-level YAML (RPCS3's `config.yml`, Vita3K's) and BML (ares's `settings.bml`) patched in
//! place: a section at column 0 with its keys indented under it, or a key at column 0 on its
//! own. Nothing is parsed beyond the lines looked at, so comments, order, quoting and
//! everything else stay as they were.
//!
//! A section is a key at column 0 with nothing after its colon but a comment, or with `{}`,
//! which becomes a block when a key is added. Its keys are the lines at the indentation of its
//! first one, quoted or not. Whatever sits deeper under a key (a list, a map, the rest of a
//! long scalar) belongs to that key: it is no value to set, and a new key goes after it.
use std::ops::Range;

use super::ini::{newline, splice, split_bom};

/// `text` with `key` under `section` set to `value`; `Ok(None)` when it already is. An empty
/// `section` is a top-level key. A missing key goes after the section's last entry, a missing
/// section at the end. `bml` spells a section header without the colon.
///
/// `value` is written as given, trimmed: a plain scalar, or one already quoted (see
/// [`quote`]). A line break, or anything that would end a plain scalar early or make it
/// something else (`: `, ` #`, a leading `[`, `&` or `- `…), is an error. BML takes the rest
/// of the line as the value, so there only a line break is.
pub fn set(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
    bml: bool,
) -> Result<Option<String>, String> {
    if !section.is_empty() {
        name(section, bml)?;
    }
    name(key, bml)?;
    let value = scalar(value, bml)?;
    let (bom, text) = split_bom(text);
    let nl = newline(text);
    let raw: Vec<&str> = text.split_inclusive('\n').collect();
    let lines: Vec<Line> = raw.iter().map(|l| Line::read(l, bml)).collect();
    let end = doc_end(&lines);
    let new_key = |indent: &str| match (literal(key, bml), value) {
        (k, "") => format!("{indent}{k}:"),
        (k, v) => format!("{indent}{k}: {v}"),
    };
    let done = |out: String| Ok(Some(format!("{bom}{out}")));

    let Some(block) = section_at(&lines, section, bml, end)? else {
        let colon = if bml { "" } else { ":" };
        let new = [
            format!("{}{colon}", literal(section, bml)),
            new_key(&unit(&lines)),
        ];
        return done(splice(&raw, end, 0, &new, nl));
    };
    let entries = entries(&lines, &block);
    if let Some(&(e, k)) = entries.iter().find(|(_, k)| k.name == key) {
        if owned_end(&lines, e, &block) > e {
            return Err(format!("{key} holds a block, not a value"));
        }
        let body = lines[e].body;
        if &body[k.value.clone()] == value {
            return Ok(None);
        }
        let gap = &body[k.colon + 1..k.value.start];
        let tail = &body[k.value.end..];
        let mut line = body[..=k.colon].to_string();
        if value.is_empty() {
            if !tail.trim().is_empty() {
                line.push(' ');
                line.push_str(tail.trim_start());
            }
        } else {
            line.push_str(if gap.is_empty() { " " } else { gap });
            line.push_str(value);
            if !tail.is_empty() && !tail.starts_with([' ', '\t']) {
                line.push(' ');
            }
            line.push_str(tail);
        }
        let ending = &raw[e][body.len()..];
        return done(format!(
            "{}{line}{ending}{}",
            raw[..e].concat(),
            raw[e + 1..].concat()
        ));
    }
    match (block.header, entries.last()) {
        (Some(h), _) if block.flow => {
            // `Video: {}` becomes `Video:` with the key under it; a comment stays.
            let Some(k) = lines[h].key() else {
                return Err(format!("{section} is not a section"));
            };
            let body = lines[h].body;
            let mut header = body[..=k.colon].to_string();
            let tail = body[k.value.end..].trim();
            if !tail.is_empty() {
                header.push(' ');
                header.push_str(tail);
            }
            let new = [header, new_key(&unit(&lines))];
            done(splice(&raw, h, 1, &new, nl))
        }
        (_, Some(&(last, _))) => {
            let indent = block.indent.as_ref().map_or("", |(_, s)| s.as_str());
            let at = owned_end(&lines, last, &block) + 1;
            done(splice(&raw, at, 0, &[new_key(indent)], nl))
        }
        (Some(h), None) => {
            let indent = block
                .indent
                .as_ref()
                .map_or_else(|| unit(&lines), |(_, s)| s.clone());
            done(splice(&raw, h + 1, 0, &[new_key(&indent)], nl))
        }
        (None, None) => done(splice(&raw, end, 0, &[new_key("")], nl)),
    }
}

/// The value of `key` under `section` as the file spells it, quotes included, without a
/// comment; `None` when it is not there or holds a block rather than a value.
pub fn get(text: &str, section: &str, key: &str, bml: bool) -> Option<String> {
    let (_, text) = split_bom(text);
    let raw: Vec<&str> = text.split_inclusive('\n').collect();
    let lines: Vec<Line> = raw.iter().map(|l| Line::read(l, bml)).collect();
    let block = section_at(&lines, section, bml, doc_end(&lines)).ok()??;
    let (e, k) = entries(&lines, &block)
        .into_iter()
        .find(|(_, k)| k.name == key)?;
    if owned_end(&lines, e, &block) > e {
        return None;
    }
    Some(lines[e].body[k.value.clone()].to_string())
}

/// `s` as a double-quoted YAML scalar: `"` and `\` escaped, and every control character and
/// line break spelled as an escape, so any text is one value on one line.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\0"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\u{1b}' => out.push_str("\\e"),
            '\u{85}' => out.push_str("\\N"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            c if c.is_control() => out.push_str(&format!("\\x{:02X}", u32::from(c))),
            '\u{feff}' | '\u{fffe}' | '\u{ffff}' => {
                out.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line, as far as the editor reads it.
struct Line<'a> {
    /// Without its line ending.
    body: &'a str,
    /// Leading spaces and tabs.
    indent: usize,
    kind: Kind,
}

enum Kind {
    Blank,
    Comment,
    /// `- item`.
    Item,
    Key(Key),
    /// A BML node without a value, a document marker, anything else.
    Other,
}

/// `name: value` on one line.
struct Key {
    /// Unquoted.
    name: String,
    /// Where its `:` is.
    colon: usize,
    /// The value as written, without a comment or the space around it; empty for none.
    value: Range<usize>,
}

impl<'a> Line<'a> {
    fn read(all: &'a str, bml: bool) -> Line<'a> {
        let body = all.trim_end_matches(['\r', '\n']);
        let t = body.trim_start_matches([' ', '\t']);
        let indent = body.len() - t.len();
        let kind = if t.trim().is_empty() {
            Kind::Blank
        } else if (!bml && t.starts_with('#')) || (bml && t.starts_with("//")) {
            Kind::Comment
        } else if !bml && (t == "-" || t.starts_with("- ") || t.starts_with("-\t")) {
            Kind::Item
        } else {
            key(body, indent, bml).map_or(Kind::Other, Kind::Key)
        };
        Line { body, indent, kind }
    }

    fn key(&self) -> Option<&Key> {
        match &self.kind {
            Kind::Key(k) => Some(k),
            _ => None,
        }
    }

    fn content(&self) -> bool {
        !matches!(self.kind, Kind::Blank | Kind::Comment)
    }
}

/// The key on a line whose text starts at `at`: quoted or plain, up to a `:` that ends the
/// line or has a space after it, so `Aspect ratio: 16:9` keeps its value whole.
fn key(body: &str, at: usize, bml: bool) -> Option<Key> {
    let t = &body[at..];
    let (name, colon) = if !bml && t.starts_with(['"', '\'']) {
        let end = quoted_len(t)?;
        let after = &t[end..];
        let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
        (unquote(&t[..end]), at + end + gap)
    } else {
        let b = t.as_bytes();
        let i = (0..b.len())
            .find(|&i| b[i] == b':' && matches!(b.get(i + 1), None | Some(b' ' | b'\t')))?;
        (t[..i].trim_end_matches([' ', '\t']).to_string(), at + i)
    };
    let b = body.as_bytes();
    if name.is_empty()
        || b.get(colon) != Some(&b':')
        || !matches!(b.get(colon + 1), None | Some(b' ' | b'\t'))
    {
        return None;
    }
    let rest = &body[colon + 1..];
    let start = colon + 1 + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
    let len = if bml {
        body[start..].trim_end().len()
    } else {
        value_len(&body[start..])
    };
    Some(Key {
        name,
        colon,
        value: start..start + len,
    })
}

/// How long the value at the start of `rest` is, without a comment or the space before it.
fn value_len(rest: &str) -> usize {
    if rest.starts_with('#') {
        return 0;
    }
    if rest.starts_with(['"', '\''])
        && let Some(n) = quoted_len(rest)
    {
        return n;
    }
    let b = rest.as_bytes();
    let cut = (1..b.len())
        .find(|&i| b[i] == b'#' && matches!(b[i - 1], b' ' | b'\t'))
        .unwrap_or(b.len());
    rest[..cut].trim_end_matches([' ', '\t']).len()
}

/// One past the quote that closes the quoted scalar `v` starts with.
fn quoted_len(v: &str) -> Option<usize> {
    let q = v.chars().next()?;
    let mut chars = v.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if q == '"' && c == '\\' {
            chars.next();
        } else if c == q {
            if q == '\'' && chars.peek().is_some_and(|&(_, n)| n == '\'') {
                chars.next();
                continue;
            }
            return Some(i + 1);
        }
    }
    None
}

/// The text of a quoted scalar.
fn unquote(lit: &str) -> String {
    if let Some(inner) = lit.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    let Some(inner) = lit.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return lit.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            out.push('\\');
            break;
        };
        let simple = match e {
            '0' => '\0',
            'a' => '\u{7}',
            'b' => '\u{8}',
            't' | '\t' => '\t',
            'n' => '\n',
            'v' => '\u{b}',
            'f' => '\u{c}',
            'r' => '\r',
            'e' => '\u{1b}',
            'N' => '\u{85}',
            '_' => '\u{a0}',
            'L' => '\u{2028}',
            'P' => '\u{2029}',
            'x' | 'u' | 'U' => {
                let n = match e {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let hex: String = chars.by_ref().take(n).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        out.push('\\');
                        out.push(e);
                        out.push_str(&hex);
                    }
                }
                continue;
            }
            other => other,
        };
        out.push(simple);
    }
    out
}

/// Where a section's keys are.
struct Block {
    /// The header's line; `None` for the top level.
    header: Option<usize>,
    /// The line after the section's last: the next at column 0, or the document's end.
    end: usize,
    /// How far its keys are indented, and with what; `None` while it has none.
    indent: Option<(usize, String)>,
    /// `section: {}`, an empty flow mapping.
    flow: bool,
}

/// The block of `section`, or of the top level for an empty one; `None` when it is missing.
fn section_at(
    lines: &[Line],
    section: &str,
    bml: bool,
    end: usize,
) -> Result<Option<Block>, String> {
    if section.is_empty() {
        return Ok(Some(Block {
            header: None,
            end,
            indent: Some((0, String::new())),
            flow: false,
        }));
    }
    let header = |l: &Line| {
        l.indent == 0
            && if bml {
                matches!(l.kind, Kind::Other) && l.body.trim_end() == section
            } else {
                l.key().is_some_and(|k| k.name == section)
            }
    };
    let Some(h) = (0..end).find(|&i| header(&lines[i])) else {
        return Ok(None);
    };
    let mut flow = false;
    if let Some(k) = lines[h].key() {
        let v = &lines[h].body[k.value.clone()];
        let empty_flow = v
            .strip_prefix('{')
            .and_then(|r| r.strip_suffix('}'))
            .is_some_and(|inner| inner.trim().is_empty());
        if empty_flow {
            flow = true;
        } else if !v.is_empty() {
            return Err(format!("{section} holds a value, not keys"));
        }
    }
    let stop = (h + 1..end)
        .find(|&i| {
            let l = &lines[i];
            l.content() && l.indent == 0 && !matches!(l.kind, Kind::Item)
        })
        .unwrap_or(end);
    let indent = match (h + 1..stop).find(|&i| lines[i].content()) {
        Some(i) if matches!(lines[i].kind, Kind::Item) => {
            return Err(format!("{section} is a list, not keys"));
        }
        Some(_) if flow => return Err(format!("{section}: {{}} has lines under it")),
        Some(i) => Some((
            lines[i].indent,
            lines[i].body[..lines[i].indent].to_string(),
        )),
        None => None,
    };
    Ok(Some(Block {
        header: Some(h),
        end: stop,
        indent,
        flow,
    }))
}

/// The keys of `block`: the lines at its keys' indentation.
fn entries<'l>(lines: &'l [Line], block: &Block) -> Vec<(usize, &'l Key)> {
    let Some((n, _)) = block.indent else {
        return Vec::new();
    };
    let start = block.header.map_or(0, |h| h + 1);
    (start..block.end)
        .filter(|&i| lines[i].indent == n)
        .filter_map(|i| lines[i].key().map(|k| (i, k)))
        .collect()
}

/// The last line that belongs to the key on line `e`: deeper lines and list items under it.
fn owned_end(lines: &[Line], e: usize, block: &Block) -> usize {
    let n = lines[e].indent;
    let mut last = e;
    for (i, l) in lines.iter().enumerate().take(block.end).skip(e + 1) {
        if !l.content() {
            continue;
        }
        if l.indent > n || (l.indent == n && matches!(l.kind, Kind::Item)) {
            last = i;
        } else {
            break;
        }
    }
    last
}

/// The line of a `...` document end marker, or the end of the text.
fn doc_end(lines: &[Line]) -> usize {
    lines
        .iter()
        .position(|l| l.indent == 0 && l.body.trim_end() == "...")
        .unwrap_or(lines.len())
}

/// The file's indentation step: that of its first indented key, or two spaces.
fn unit(lines: &[Line]) -> String {
    lines
        .iter()
        .find(|l| l.indent > 0 && l.key().is_some())
        .map_or_else(|| "  ".into(), |l| l.body[..l.indent].to_string())
}

/// A name as a key spells it: plain where it can be, quoted where it must be.
fn literal(name: &str, bml: bool) -> String {
    if bml || (plain(name) && !name.starts_with(['"', '\''])) {
        name.to_string()
    } else {
        quote(name)
    }
}

/// A character that would end a line or a scalar.
fn breaks(c: char) -> bool {
    (c.is_control() && c != '\t') || c == '\u{2028}' || c == '\u{2029}'
}

fn name(n: &str, bml: bool) -> Result<(), String> {
    if n.is_empty() || n.chars().any(breaks) {
        return Err(format!("{n:?} cannot be a key"));
    }
    if bml && n.contains(|c: char| c.is_whitespace() || c == ':' || c == '=') {
        return Err(format!("{n:?} cannot be a BML node name"));
    }
    Ok(())
}

/// `value`, trimmed, when it reads back as one value on one line.
fn scalar(value: &str, bml: bool) -> Result<&str, String> {
    if value.chars().any(breaks) {
        return Err(format!(
            "{value:?} holds a line break or control character; quote it with yaml::quote"
        ));
    }
    let v = value.trim_matches([' ', '\t']);
    let whole = bml
        || match v.as_bytes().first() {
            Some(b'"' | b'\'') => quoted_len(v) == Some(v.len()),
            _ => plain(v),
        };
    if whole {
        Ok(v)
    } else {
        Err(format!(
            "{v:?} would not read back as one value; quote it with yaml::quote"
        ))
    }
}

/// Whether `v` reads back as itself, one plain scalar in a block mapping.
fn plain(v: &str) -> bool {
    let mut chars = v.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    let spaced = |c: Option<char>| c.is_none_or(|c| c == ' ' || c == '\t');
    !(v.trim_matches([' ', '\t']) != v
        || "[]{},#&*!|>%@`\"'".contains(first)
        || (matches!(first, '-' | '?' | ':') && spaced(chars.next()))
        || v.contains(": ")
        || v.contains(":\t")
        || v.ends_with(':')
        || v.contains(" #")
        || v.contains("\t#"))
}

#[cfg(test)]
mod tests {
    use super::super::ini::testkit::{changed, same_endings};
    use super::*;
    use proptest::prelude::*;

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
        assert_eq!(
            get(ares, "Video", "Driver", true).as_deref(),
            Some("OpenGL 3.2")
        );
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
        assert!(set(ares, "Video", "Two words", "x", true).is_err());
        assert!(set(ares, "Video", "Driver", "a\nb", true).is_err());
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
        assert_eq!(
            set("Video:\n  a: 1", "Video", "b", "2", false).unwrap(),
            Some("Video:\n  a: 1\n  b: 2".into()),
            "no final newline, and still none"
        );
    }

    const NESTED: &str = "Video:\n  Renderer: Vulkan\n  Vulkan:\n    Adapter: \"\"\n    Enabled: true\n  Keys:\n  - a\n  - b\nAudio:\n  Enabled: false\n";

    #[test]
    fn keys_match_only_at_their_sections_own_indent() {
        assert_eq!(get(NESTED, "Video", "Enabled", false), None);
        assert_eq!(get(NESTED, "Video", "Adapter", false), None);
        assert_eq!(
            get(NESTED, "Audio", "Enabled", false).as_deref(),
            Some("false")
        );
        // `Video/Enabled` is new, not the `Enabled` under `Video/Vulkan`.
        let out = set(NESTED, "Video", "Enabled", "true", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            NESTED.replace("  - b\nAudio:", "  - b\n  Enabled: true\nAudio:")
        );
        // A section is found only at column 0.
        assert_eq!(get(NESTED, "Vulkan", "Enabled", false), None);
        let out = set(NESTED, "Vulkan", "Enabled", "false", false)
            .unwrap()
            .unwrap();
        assert!(out.ends_with("Audio:\n  Enabled: false\nVulkan:\n  Enabled: false\n"));
    }

    #[test]
    fn a_key_with_a_block_under_it_is_not_a_value_and_new_keys_go_after_the_block() {
        for key in ["Vulkan", "Keys"] {
            assert!(
                matches!(set(NESTED, "Video", key, "1", false), Err(e) if e.contains("block")),
                "{key}"
            );
            assert_eq!(get(NESTED, "Video", key, false), None);
        }
        assert!(set(NESTED, "", "Video", "1", false).is_err());
        // The new key goes after the list, not between its items.
        let out = set(NESTED, "Video", "VSync", "false", false)
            .unwrap()
            .unwrap();
        assert!(
            out.contains("  - a\n  - b\n  VSync: false\nAudio:\n"),
            "{out}"
        );
        // Indented lists and long scalars too.
        let t = "A:\n  k: 1\n  l:\n    - x\n    - y\n  m: a long\n    scalar\n  # trailing\nB:\n";
        let out = set(t, "A", "n", "2", false).unwrap().unwrap();
        assert_eq!(out, t.replace("    scalar\n", "    scalar\n  n: 2\n"));
        assert!(set(t, "A", "m", "short", false).is_err());
        // A top-level key goes after the last one's block, before a document end.
        let t = "---\na: 1\nb:\n  - 1\n...\n";
        assert_eq!(
            set(t, "", "c", "2", false).unwrap().unwrap(),
            "---\na: 1\nb:\n  - 1\nc: 2\n...\n"
        );
        assert_eq!(
            set(t, "S", "c", "2", false).unwrap().unwrap(),
            "---\na: 1\nb:\n  - 1\nS:\n  c: 2\n...\n"
        );
    }

    #[test]
    fn headers_with_comments_flow_maps_and_values() {
        let t = "Video: # the picture\n  VSync: false # no tearing\nAudio: {}\nNet: {} # later\nMisc: 1\nList:\n- a\n";
        assert_eq!(get(t, "Video", "VSync", false).as_deref(), Some("false"));
        let out = set(t, "Video", "VSync", "true", false).unwrap().unwrap();
        assert_eq!(out, t.replace("VSync: false # no", "VSync: true # no"));
        assert_eq!(set(t, "Video", "VSync", "false", false), Ok(None));
        // `{}` becomes a block holding the new key.
        assert_eq!(get(t, "Audio", "Volume", false), None);
        let out = set(t, "Audio", "Volume", "100", false).unwrap().unwrap();
        assert_eq!(out, t.replace("Audio: {}\n", "Audio:\n  Volume: 100\n"));
        let out = set(t, "Net", "Port", "1", false).unwrap().unwrap();
        assert_eq!(
            out,
            t.replace("Net: {} # later\n", "Net: # later\n  Port: 1\n")
        );
        assert!(matches!(set(t, "Misc", "k", "1", false), Err(e) if e.contains("value")));
        assert!(matches!(set(t, "List", "k", "1", false), Err(e) if e.contains("list")));
        assert!(set("A: {x: 1}\n", "A", "k", "1", false).is_err());
        // A comment after an empty value keeps its distance.
        assert_eq!(
            set("A:\n  k: # unset\n", "A", "k", "1", false).unwrap(),
            Some("A:\n  k: 1 # unset\n".into())
        );
        assert_eq!(
            set("A:\n  k: 1 # set\n", "A", "k", "", false).unwrap(),
            Some("A:\n  k: # set\n".into())
        );
    }

    #[test]
    fn quoted_keys_match_their_names() {
        let t = "Video:\n  \"VSync Mode\": Off\n  'it''s': 1\n  \"a\\tb\": 2\n";
        assert_eq!(get(t, "Video", "VSync Mode", false).as_deref(), Some("Off"));
        assert_eq!(get(t, "Video", "it's", false).as_deref(), Some("1"));
        assert_eq!(get(t, "Video", "a\tb", false).as_deref(), Some("2"));
        let out = set(t, "Video", "VSync Mode", "Full", false)
            .unwrap()
            .unwrap();
        assert_eq!(out, t.replace("Off", "Full"));
        // A name a plain key cannot carry is written quoted, and found again.
        let out = set(t, "Video", "a: b", "1", false).unwrap().unwrap();
        assert!(out.ends_with("  \"a: b\": 1\n"), "{out}");
        assert_eq!(set(&out, "Video", "a: b", "1", false), Ok(None));
        let out = set("", "#x", "- y", "1", false).unwrap().unwrap();
        assert_eq!(out, "\"#x\":\n  \"- y\": 1\n");
        assert_eq!(get(&out, "#x", "- y", false).as_deref(), Some("1"));
    }

    #[test]
    fn values_that_would_not_read_back_are_refused_unless_quoted() {
        for bad in [
            "a\nb",
            "a\rb",
            "a\u{2028}b",
            "x: y",
            "x #y",
            "[1]",
            "{a: 1}",
            "&anchor",
            "*alias",
            "!tag",
            "- item",
            "-",
            "? q",
            "|",
            ">",
            "@x",
            "`x",
            "%x",
            "end:",
            "\"open",
            "\"a\" b",
            "'a'b'",
        ] {
            assert!(
                set(RPCS3, "Video", "Renderer", bad, false).is_err(),
                "{bad:?}"
            );
        }
        for good in [
            "16:9",
            "a#b",
            "-1",
            "C:\\x",
            "a, b",
            "\"x: y\"",
            "'it''s'",
            "",
            "OpenGL 3.2",
        ] {
            let out = set(RPCS3, "Video", "Renderer", good, false)
                .unwrap()
                .unwrap();
            assert_eq!(get(&out, "Video", "Renderer", false).as_deref(), Some(good));
        }
        // Readers trim a plain value; so does the writer.
        assert_eq!(set(RPCS3, "Video", "Renderer", " Vulkan ", false), Ok(None));
    }

    #[test]
    fn quote_makes_any_text_one_value() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("a \"b\" \\c"), "\"a \\\"b\\\" \\\\c\"");
        assert_eq!(quote("1\n2\r\t\0"), "\"1\\n2\\r\\t\\0\"");
        assert_eq!(quote("\u{1}\u{7f}\u{85}\u{2028}"), "\"\\x01\\x7F\\N\\L\"");
        for s in ["Pad: \"one\" # 2\n- x", "\u{feff}é😀", "\\\\\"", "'"] {
            let q = quote(s);
            assert_eq!(unquote(&q), s);
            let out = set(RPCS3, "Input", "Name", &q, false).unwrap().unwrap();
            assert_eq!(get(&out, "Input", "Name", false), Some(q));
        }
    }

    #[test]
    fn a_bom_is_kept_and_seen_through() {
        let t = "\u{feff}Video:\n  VSync: false\n";
        assert_eq!(get(t, "Video", "VSync", false).as_deref(), Some("false"));
        assert_eq!(
            set(t, "Video", "VSync", "true", false).unwrap(),
            Some("\u{feff}Video:\n  VSync: true\n".into())
        );
    }

    // Property tests over generated files: top-level keys and sections whose keys may own a
    // nested map or a list (with names that also appear as section keys), comments, blanks,
    // quoted keys, `{}` sections, LF or CRLF, a final newline or not.

    #[derive(Clone, Debug)]
    enum Entry {
        Scalar(String, String),
        Map(String, Vec<String>),
        List(String, Vec<String>),
    }

    #[derive(Clone, Debug)]
    struct Section {
        name: String,
        entries: Vec<Entry>,
        comment: bool,
    }

    #[derive(Clone, Debug)]
    struct Doc {
        top: Vec<(String, String)>,
        sections: Vec<Section>,
        indent: &'static str,
        quoted: bool,
        crlf: bool,
        final_nl: bool,
    }

    impl Doc {
        fn render(&self) -> String {
            let nl = if self.crlf { "\r\n" } else { "\n" };
            let ind = self.indent;
            let mut out = String::new();
            let kv = |k: &str, v: &str| {
                if v.is_empty() {
                    format!("{k}:")
                } else {
                    format!("{k}: {v}")
                }
            };
            for (k, v) in &self.top {
                out.push_str(&format!("{}{nl}", kv(k, v)));
            }
            for s in &self.sections {
                if s.comment {
                    out.push_str(&format!("# {}{nl}", s.name));
                }
                if s.entries.is_empty() {
                    out.push_str(&format!("{}: {{}}{nl}", s.name));
                    continue;
                }
                let note = if s.comment { " # settings" } else { "" };
                out.push_str(&format!("{}:{note}{nl}", s.name));
                for (i, e) in s.entries.iter().enumerate() {
                    if s.comment && i == 1 {
                        out.push_str(&format!("{nl}{ind}# more{nl}"));
                    }
                    match e {
                        Entry::Scalar(k, v) if self.quoted => {
                            out.push_str(&format!("{ind}{}{nl}", kv(&quote(k), v)));
                        }
                        Entry::Scalar(k, v) => out.push_str(&format!("{ind}{}{nl}", kv(k, v))),
                        Entry::Map(k, children) => {
                            out.push_str(&format!("{ind}{k}:{nl}"));
                            for c in children {
                                out.push_str(&format!("{ind}{ind}{c}: 1{nl}"));
                            }
                        }
                        Entry::List(k, items) => {
                            out.push_str(&format!("{ind}{k}:{nl}"));
                            let item_ind = if self.quoted { ind } else { "" };
                            for it in items {
                                out.push_str(&format!("{ind}{item_ind}- {it}{nl}"));
                            }
                        }
                    }
                }
            }
            if !self.final_nl {
                while out.ends_with(['\r', '\n']) {
                    out.pop();
                }
            }
            out
        }

        /// Every key that holds a value: `(section, key)`.
        fn scalars(&self) -> Vec<(String, String)> {
            let top = self.top.iter().map(|(k, _)| (String::new(), k.clone()));
            let nested = self.sections.iter().flat_map(|s| {
                s.entries.iter().filter_map(move |e| match e {
                    Entry::Scalar(k, _) => Some((s.name.clone(), k.clone())),
                    _ => None,
                })
            });
            top.chain(nested).collect()
        }
    }

    const KEY: &str = "[A-Z][a-z]{0,4}( [A-Z][a-z]{0,4})?";

    fn plain_value() -> impl Strategy<Value = String> {
        "[A-Za-z0-9][A-Za-z0-9._/()]{0,4}(:[0-9]{1,2})?( [A-Za-z0-9._/()-][A-Za-z0-9._/()#-]{0,3}){0,2}"
    }

    fn value() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => plain_value(),
            1 => "\\PC{0,8}".prop_map(|s| quote(&s)),
            1 => "[a-z\\n\\t\"\\\\#: ]{0,8}".prop_map(|s| quote(&s)),
            1 => Just(String::new()),
        ]
    }

    fn entry() -> impl Strategy<Value = Entry> {
        let names = prop::collection::vec(KEY, 1..3);
        prop_oneof![
            4 => (KEY, value()).prop_map(|(k, v)| Entry::Scalar(k, v)),
            1 => (KEY, names.clone()).prop_map(|(k, c)| Entry::Map(k, c)),
            1 => (KEY, names).prop_map(|(k, c)| Entry::List(k, c)),
        ]
    }

    fn entries() -> impl Strategy<Value = Vec<Entry>> {
        prop::collection::vec(entry(), 0..5).prop_map(|es| {
            let mut seen = std::collections::BTreeSet::new();
            es.into_iter()
                .filter(|e| {
                    let (Entry::Scalar(k, _) | Entry::Map(k, _) | Entry::List(k, _)) = e;
                    seen.insert(k.clone())
                })
                .collect()
        })
    }

    fn doc() -> impl Strategy<Value = Doc> {
        (
            prop::collection::btree_map("[a-z][a-z-]{1,8}", plain_value(), 0..3),
            prop::collection::btree_map("[A-Z][a-z]{1,6}", (entries(), any::<bool>()), 0..4),
            prop_oneof![Just("  "), Just("    ")],
            any::<[bool; 3]>(),
        )
            .prop_map(|(top, sections, indent, [quoted, crlf, final_nl])| Doc {
                top: top.into_iter().collect(),
                sections: sections
                    .into_iter()
                    .map(|(name, (entries, comment))| Section {
                        name,
                        entries,
                        comment,
                    })
                    .collect(),
                indent,
                quoted,
                crlf,
                final_nl,
            })
    }

    /// A key of the document that holds a value, or a new one.
    fn target(d: &Doc, pick: usize, fresh: bool) -> (String, String) {
        let all = d.scalars();
        if fresh || all.is_empty() {
            let section = match d.sections.get(pick % (d.sections.len() + 2)) {
                Some(s) => s.name.clone(),
                None if pick.is_multiple_of(2) => String::new(),
                None => "Fresh".into(),
            };
            let key = if section.is_empty() {
                "fresh-key"
            } else {
                "Fresh-Key"
            };
            (section, key.into())
        } else {
            all[pick % all.len()].clone()
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn set_changes_one_key_and_get_reads_it_back(
            d in doc(), pick in any::<usize>(), fresh in any::<bool>(), v in value(),
        ) {
            let text = d.render();
            let (s, k) = target(&d, pick, fresh);
            let existed = get(&text, &s, &k, false).is_some();
            let section = d.sections.iter().find(|x| x.name == s);
            let r = set(&text, &s, &k, &v, false).unwrap();
            let out = r.clone().unwrap_or_else(|| text.clone());
            let back = get(&out, &s, &k, false);
            prop_assert_eq!(back.as_deref(), Some(v.as_str()));
            prop_assert_eq!(set(&out, &s, &k, &v, false), Ok(None));
            for (os, ok) in d.scalars() {
                if (os.as_str(), ok.as_str()) != (s.as_str(), k.as_str()) {
                    prop_assert_eq!(get(&out, &os, &ok, false), get(&text, &os, &ok, false));
                }
            }
            let (gone, new) = changed(&text, &out);
            let counts = (gone.len(), new.len());
            if r.is_none() {
                prop_assert!(existed);
            } else if existed {
                prop_assert_eq!(counts, (1, 1));
            } else {
                match section {
                    Some(x) if x.entries.is_empty() => prop_assert_eq!(counts, (1, 2)),
                    None if !s.is_empty() => prop_assert_eq!(counts, (0, 2)),
                    _ => prop_assert_eq!(counts, (0, 1)),
                }
            }
            prop_assert!(same_endings(&text, &out));
        }

        #[test]
        fn a_line_break_never_adds_a_key(
            d in doc(), pick in any::<usize>(), fresh in any::<bool>(),
            a in "[a-z]{0,4}", b in "[A-Z][a-z]{0,4}", brk in "[\\n\\r\\x00\\u{2028}]",
        ) {
            let text = d.render();
            let (s, k) = target(&d, pick, fresh);
            let v = format!("{a}{brk}{b}: 1");
            let broken_key = format!("{k}{brk}{b}");
            prop_assert!(set(&text, &s, &k, &v, false).is_err());
            prop_assert!(set(&text, &s, &broken_key, "1", false).is_err());
            prop_assert!(set(&text, &s, &k, &v, true).is_err());
        }

        #[test]
        fn bml_set_changes_one_key_and_get_reads_it_back(
            sections in prop::collection::btree_map(
                "[A-Z][a-z]{1,6}",
                prop::collection::btree_map("[A-Z][A-Za-z0-9]{0,6}", "[!-~]{1,4}( [!-~]{1,4}){0,2}", 0..4),
                0..4,
            ),
            pick in any::<usize>(), fresh in any::<bool>(), v in "[!-~]{1,4}( [!-~]{1,4}){0,2}",
        ) {
            let mut text = String::new();
            for (name, es) in &sections {
                text.push_str(&format!("{name}\n"));
                for (k, val) in es {
                    text.push_str(&format!("  {k}: {val}\n"));
                }
                text.push('\n');
            }
            let all: Vec<(String, String)> = sections
                .iter()
                .flat_map(|(s, es)| es.keys().map(move |k| (s.clone(), k.clone())))
                .collect();
            let (s, k) = if fresh || all.is_empty() {
                let names: Vec<&String> = sections.keys().collect();
                let s = names.get(pick % (names.len() + 1)).map_or("Fresh".into(), |s| (*s).clone());
                (s, "FreshKey".to_string())
            } else {
                all[pick % all.len()].clone()
            };
            let existed = get(&text, &s, &k, true).is_some();
            let r = set(&text, &s, &k, &v, true).unwrap();
            let out = r.clone().unwrap_or_else(|| text.clone());
            let back = get(&out, &s, &k, true);
            prop_assert_eq!(back.as_deref(), Some(v.as_str()));
            prop_assert_eq!(set(&out, &s, &k, &v, true), Ok(None));
            for (os, ok) in &all {
                if (os, ok) != (&s, &k) {
                    prop_assert_eq!(get(&out, os, ok, true), get(&text, os, ok, true));
                }
            }
            let (gone, new) = changed(&text, &out);
            let counts = (gone.len(), new.len());
            match (r.is_some(), existed, sections.contains_key(&s)) {
                (false, ..) => prop_assert!(existed),
                (true, true, _) => prop_assert_eq!(counts, (1, 1)),
                (true, false, true) => prop_assert_eq!(counts, (0, 1)),
                (true, false, false) => prop_assert_eq!(counts, (0, 2)),
            }
        }
    }
}
