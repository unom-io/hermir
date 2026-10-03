//! `key = value` under `[section]` headers, patched in place: PCSX2, DuckStation, Dolphin,
//! PPSSPP, Qt's `qt-config.ini`, flat TOML (xemu, Xenia, melonDS) and RetroArch's cfg are all
//! this shape to a line editor. Nothing is parsed beyond the lines looked at, so comments,
//! order, unknown keys and the file's own spelling stay as they were.
//!
//! A header is `[name]`, or TOML's `[[name]]`, starting its line after optional whitespace,
//! followed by nothing but a comment, with a name that has no `,` and no brackets: `  [1, 2]`
//! is a value, not a section. Quotes are allowed, for TOML's `["quoted"]` tables. A value that
//! opens a TOML array (`key = [`) owns the lines up to its closing bracket, unless an unindented
//! header comes first. A `# comment` after a TOML-shaped value (a quoted string, an array, a
//! number, a boolean) is the comment, kept when the value changes; after anything else `#` is
//! part of the value, as INI readers take it.

/// How [`set_with`] and [`get_with`] read and write a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IniOpts {
    /// How a new entry spells `=` when the file has none to copy: ` = ` or `=`.
    pub sep: &'static str,
    /// Section and key names match whatever their ASCII case, as Dolphin and SimpleIni read them.
    pub case_insensitive: bool,
}

impl Default for IniOpts {
    fn default() -> Self {
        IniOpts {
            sep: " = ",
            case_insensitive: false,
        }
    }
}

/// `text` with `key` in `[section]` set to `value`; `Ok(None)` when it already is. Every other
/// byte stays. A missing key goes after its section's last entry, a missing section at the
/// end, spelled as the file spells `key = value` (or `key=value`) and with its line endings.
/// An empty `section` is the part before any header (a RetroArch cfg has nothing else).
///
/// Readers trim a value, so `value` is written trimmed. A line break or NUL anywhere, a key a
/// reader would not take back as one, or a value it would read as a value and a comment is an
/// error: free text for a Qt file goes through [`qt_value`] first.
pub fn set(text: &str, section: &str, key: &str, value: &str) -> Result<Option<String>, String> {
    set_with(text, section, key, value, IniOpts::default())
}

/// [`set`], with `opts` for the separator of a file that has no entry to copy yet, and for
/// names matched case-insensitively.
pub fn set_with(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
    opts: IniOpts,
) -> Result<Option<String>, String> {
    check(section, key, value)?;
    let value = trim(value);
    let (bom, text) = split_bom(text);
    let nl = newline(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let kinds = classify(&lines);
    let same = |a: &str, b: &str| names_match(a, b, opts);
    let sep = kinds
        .iter()
        .find_map(|k| match k {
            Line::Entry { left, .. } if left.ends_with(' ') => Some(" = "),
            Line::Entry { .. } => Some("="),
            _ => None,
        })
        .unwrap_or(opts.sep);
    let mut inside = section.is_empty();
    let mut seen = inside;
    let mut table = false;
    let mut last: Option<usize> = None;
    for (i, kind) in kinds.iter().enumerate() {
        match *kind {
            Line::Header { name, table: t } => {
                inside = !t && same(name, section);
                table |= t && same(name, section);
                if inside {
                    seen = true;
                    last = Some(i);
                }
            }
            Line::More if inside => last = Some(i),
            Line::Entry { left, right } if inside => {
                last = Some(i);
                if !same(trim(left), key) {
                    continue;
                }
                if matches!(kinds.get(i + 1), Some(Line::More)) {
                    return Err(format!("{key} spans several lines, not one value"));
                }
                let body = right.trim_end_matches(['\r', '\n']);
                let ending = &right[body.len()..];
                let lead = &body[..body.len() - body.trim_start_matches([' ', '\t']).len()];
                let (old, tail) = split_comment(&body[lead.len()..]);
                if old == value {
                    return Ok(None);
                }
                // The comment stays where it still reads as one after the new value.
                let joined = format!("{value}{tail}");
                let tail = if split_comment(&joined).0 == value {
                    tail
                } else {
                    ""
                };
                let mut out = format!("{bom}{}", lines[..i].concat());
                out.push_str(&format!("{left}={lead}{value}{tail}{ending}"));
                out.push_str(&lines[i + 1..].concat());
                return Ok(Some(out));
            }
            _ => {}
        }
    }
    let line = format!("{key}{sep}{value}");
    if seen {
        // After the section's last entry; before everything when the part before any header
        // has none yet.
        let at = last.map_or(0, |l| l + 1);
        return Ok(Some(format!("{bom}{}", splice(&lines, at, 0, &[line], nl))));
    }
    if table {
        return Err(format!(
            "[[{section}]] is an array of tables, not a section"
        ));
    }
    let new = if text.is_empty() {
        vec![format!("[{section}]"), line]
    } else {
        vec![String::new(), format!("[{section}]"), line]
    };
    Ok(Some(format!(
        "{bom}{}",
        splice(&lines, lines.len(), 0, &new, nl)
    )))
}

/// The value of `key` in `[section]`, trimmed and unquoted, without a trailing comment; `None`
/// when it is not there, or spans several lines.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    get_with(text, section, key, IniOpts::default())
}

/// [`get`], with names matched as `opts` says.
pub fn get_with(text: &str, section: &str, key: &str, opts: IniOpts) -> Option<String> {
    let (_, text) = split_bom(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let kinds = classify(&lines);
    let mut inside = section.is_empty();
    for (i, kind) in kinds.iter().enumerate() {
        match *kind {
            Line::Header { name, table } => inside = !table && names_match(name, section, opts),
            Line::Entry { left, right } if inside && names_match(trim(left), key, opts) => {
                if matches!(kinds.get(i + 1), Some(Line::More)) {
                    return None;
                }
                let (v, _) = split_comment(trim(right.trim_end_matches(['\r', '\n'])));
                let unquoted = v
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(v);
                return Some(unquoted.to_string());
            }
            _ => {}
        }
    }
    None
}

/// `s` as QSettings writes a string into its ini, so Qt reads it back whole: `\` and `"`
/// escaped, control characters spelled out (`\n`, `\x1f`), and the whole in double quotes
/// when it holds `,` (a list otherwise), `;` or `=`, or starts or ends with a space. A
/// leading `@` is doubled, as Qt does to tell text from `@ByteArray(…)`. Unlike Qt it also
/// quotes a `#`, which this editor would otherwise take for a comment after a number.
pub fn qt_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    if s.starts_with('@') {
        out.push('@');
    }
    let mut quote = false;
    // After `\0` or `\x…`, a hex digit is escaped too, or it would extend the escape.
    let mut hex_next = false;
    for c in s.chars() {
        quote |= matches!(c, ',' | ';' | '=' | '#');
        if hex_next && c.is_ascii_hexdigit() {
            out.push_str(&format!("\\x{:x}", u32::from(c)));
            continue;
        }
        hex_next = false;
        match c {
            '\0' => {
                out.push_str("\\0");
                hex_next = true;
            }
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if u32::from(c) < 0x20 => {
                out.push_str(&format!("\\x{:x}", u32::from(c)));
                hex_next = true;
            }
            c => out.push(c),
        }
    }
    if quote || out.starts_with(' ') || out.ends_with(' ') {
        format!("\"{out}\"")
    } else {
        out
    }
}

/// What one line is to the editor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line<'a> {
    /// `[name]`, or TOML's `[[name]]` (`table`).
    Header { name: &'a str, table: bool },
    /// `key = value`: left of the first `=`, and right of it with the line ending.
    Entry { left: &'a str, right: &'a str },
    /// A line of the multi-line value above it.
    More,
    /// A comment, a blank, anything else.
    Other,
}

fn classify<'a>(lines: &[&'a str]) -> Vec<Line<'a>> {
    let mut out = Vec::with_capacity(lines.len());
    // Brackets a TOML array value still has open.
    let mut open = 0;
    for l in lines {
        if open > 0 && !(l.starts_with('[') && header(l).is_some()) {
            open = brackets(l, open);
            out.push(Line::More);
            continue;
        }
        open = 0;
        if let Some((name, table)) = header(l) {
            out.push(Line::Header { name, table });
        } else if let Some((left, right)) = entry(l) {
            let v = right.trim_start();
            if v.starts_with('[') {
                open = brackets(v, 0);
            }
            out.push(Line::Entry { left, right });
        } else {
            out.push(Line::Other);
        }
    }
    out
}

/// `(name, table)` for a `[name]` or `[[name]]` header line; `[ Global ]` (Supermodel) names
/// the section Global.
fn header(line: &str) -> Option<(&str, bool)> {
    let t = line.trim_matches([' ', '\t', '\r', '\n']);
    let (inner, rest, table) = if let Some(r) = t.strip_prefix("[[") {
        let end = r.find("]]")?;
        (&r[..end], &r[end + 2..], true)
    } else {
        let r = t.strip_prefix('[')?;
        let end = r.find(']')?;
        (&r[..end], &r[end + 1..], false)
    };
    let rest = trim(rest);
    if !(rest.is_empty() || rest.starts_with([';', '#'])) {
        return None;
    }
    let name = trim(inner);
    (!name.is_empty() && !name.contains(['[', ']', ','])).then_some((name, table))
}

/// `(left of =, right of =)` for a `key = value` line; `None` for comments, headers, blanks.
fn entry(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start_matches([' ', '\t']);
    if trim(t.trim_end_matches(['\r', '\n'])).is_empty() || t.starts_with([';', '#', '[']) {
        return None;
    }
    line.split_once('=')
}

/// The characters of `s` outside quoted strings, up to a `#` comment.
fn structural(s: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    s.char_indices()
        .filter(move |&(_, c)| match quote {
            Some(_) if escaped => {
                escaped = false;
                false
            }
            Some('"') if c == '\\' => {
                escaped = true;
                false
            }
            Some(q) => {
                if c == q {
                    quote = None;
                }
                false
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                false
            }
            None => true,
        })
        .take_while(|&(_, c)| c != '#')
}

/// `open` after the brackets of `s`.
fn brackets(s: &str, open: usize) -> usize {
    structural(s).fold(open, |open, (_, c)| match c {
        '[' => open + 1,
        ']' => open.saturating_sub(1),
        _ => open,
    })
}

/// One past the bracket closing the array `v` opens.
fn array_end(v: &str) -> Option<usize> {
    let mut open = 0usize;
    structural(v).find_map(|(i, c)| {
        match c {
            '[' => open += 1,
            ']' => open = open.saturating_sub(1),
            _ => return None,
        }
        (open == 0).then_some(i + 1)
    })
}

/// `(value, rest)` of the text right of `=`, leading whitespace already gone: a trailing
/// `# comment` (with the whitespace before it) is split off a TOML-shaped value; otherwise the
/// rest is just trailing whitespace.
fn split_comment(v: &str) -> (&str, &str) {
    let end = match v.as_bytes().first() {
        Some(b'"') => closing(v, '"'),
        Some(b'\'') => closing(v, '\''),
        Some(b'[') => array_end(v),
        _ => {
            let token = v.find([' ', '\t']).unwrap_or(v.len());
            toml_scalar(&v[..token]).then_some(token)
        }
    };
    match end {
        Some(e) if trim(&v[e..]).starts_with('#') => (&v[..e], &v[e..]),
        _ => {
            let t = v.trim_end_matches([' ', '\t']);
            (t, &v[t.len()..])
        }
    }
}

/// One past the quote closing the string `v` opens; a `"` string knows `\` escapes.
fn closing(v: &str, q: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in v.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' if q == '"' => escaped = true,
            c if c == q => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// `s` without the spaces and tabs around it, all an INI reader trims.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

/// A bare TOML value a comment may follow: a boolean or something number-shaped.
fn toml_scalar(t: &str) -> bool {
    let n = t.trim_start_matches(['+', '-']);
    matches!(t, "true" | "false")
        || matches!(n, "inf" | "nan")
        || n.starts_with(|c: char| c.is_ascii_digit())
}

fn names_match(a: &str, b: &str, opts: IniOpts) -> bool {
    if opts.case_insensitive {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn check(section: &str, key: &str, value: &str) -> Result<(), String> {
    for (what, s) in [("section", section), ("key", key), ("value", value)] {
        if s.contains(['\n', '\r', '\0']) {
            return Err(format!("the {what} {s:?} holds a line break or NUL"));
        }
    }
    if trim(section) != section || section.contains(['[', ']', ',']) {
        return Err(format!("{section:?} cannot be a section name"));
    }
    if key.is_empty() || trim(key) != key || key.contains('=') || key.starts_with(['[', ';', '#']) {
        return Err(format!("{key:?} cannot be a key"));
    }
    let v = trim(value);
    if split_comment(v).0 != v {
        return Err(format!("{v:?} would read back as a value and a comment"));
    }
    Ok(())
}

/// `(byte-order mark, the rest)`: the mark stays where it was and never reaches an editor.
pub(super) fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

/// The file's line ending: CRLF when it has one, LF otherwise.
pub(super) fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// `lines` with `new` in place of `lines[at..at + gone]`, each line ending in `nl`, except that
/// a file without a final newline still has none.
pub(super) fn splice(lines: &[&str], at: usize, gone: usize, new: &[String], nl: &str) -> String {
    let mut out: String = lines[..at].concat();
    let unterminated = !out.is_empty() && !out.ends_with('\n');
    if unterminated {
        out.push_str(nl);
    }
    let rest = &lines[at + gone..];
    let open_end =
        rest.is_empty() && (unterminated || (gone > 0 && !lines[at + gone - 1].ends_with('\n')));
    for (i, l) in new.iter().enumerate() {
        out.push_str(l);
        if !(open_end && i + 1 == new.len()) {
            out.push_str(nl);
        }
    }
    out.push_str(&rest.concat());
    out
}

#[cfg(test)]
pub(super) mod testkit {
    /// The lines `after` has in place of `before`'s, once the lines they share at the start
    /// and the end are set aside: `(gone, new)`.
    pub fn changed<'a>(before: &'a str, after: &'a str) -> (Vec<&'a str>, Vec<&'a str>) {
        let a: Vec<&str> = before.lines().collect();
        let b: Vec<&str> = after.lines().collect();
        let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let tail = a[head..]
            .iter()
            .rev()
            .zip(b[head..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        (
            a[head..a.len() - tail].to_vec(),
            b[head..b.len() - tail].to_vec(),
        )
    }

    /// A CRLF file stays CRLF throughout, and a file without a final newline has none.
    pub fn same_endings(before: &str, after: &str) -> bool {
        let crlf_kept = !before.contains("\r\n") || !after.replace("\r\n", "").contains('\n');
        let final_kept = before.is_empty() || before.ends_with('\n') == after.ends_with('\n');
        crlf_kept && final_kept
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{changed, same_endings};
    use super::*;
    use proptest::prelude::*;

    fn set(t: &str, s: &str, k: &str, v: &str) -> Option<String> {
        super::set(t, s, k, v).unwrap()
    }

    #[test]
    fn changes_one_value_and_nothing_else() {
        let t = "; top\n[UI]\nSetupWizardIncomplete = true\nTheme = dark\n\n[Filenames]\nBIOS = \n";
        let out = set(t, "UI", "SetupWizardIncomplete", "false").unwrap();
        assert_eq!(out, t.replace("Incomplete = true", "Incomplete = false"));
        assert_eq!(set(&out, "UI", "SetupWizardIncomplete", "false"), None);
        let out = set(t, "Filenames", "BIOS", "scph.bin").unwrap();
        assert!(out.ends_with("[Filenames]\nBIOS = scph.bin\n"), "{out}");
    }

    #[test]
    fn adds_a_key_to_its_section_in_the_files_spelling() {
        let t = "[main_window]\ngeometry=abc\n\n[Meta]\nx=1\n";
        let out = set(t, "main_window", "infoBoxEnabledWelcome", "false").unwrap();
        assert_eq!(
            out,
            "[main_window]\ngeometry=abc\ninfoBoxEnabledWelcome=false\n\n[Meta]\nx=1\n"
        );
        let spaced = "[Main]\nA = 1\n";
        assert_eq!(
            set(spaced, "Main", "B", "2").unwrap(),
            "[Main]\nA = 1\nB = 2\n"
        );
    }

    #[test]
    fn adds_a_missing_section_and_keeps_crlf() {
        let t = "[Main]\r\nA = 1\r\n";
        assert_eq!(
            set(t, "AutoUpdater", "CheckAtStartup", "false").unwrap(),
            "[Main]\r\nA = 1\r\n\r\n[AutoUpdater]\r\nCheckAtStartup = false\r\n"
        );
        assert_eq!(
            set("", "Analytics", "PermissionAsked", "True").unwrap(),
            "[Analytics]\nPermissionAsked = True\n"
        );
        let qt = IniOpts {
            sep: "=",
            ..IniOpts::default()
        };
        assert_eq!(
            set_with("", "UI", "fullscreen", "true", qt).unwrap(),
            Some("[UI]\nfullscreen=true\n".into())
        );
    }

    #[test]
    fn a_file_without_a_final_newline_still_has_none() {
        assert_eq!(
            set("[A]\nk = 1", "A", "j", "2").unwrap(),
            "[A]\nk = 1\nj = 2"
        );
        assert_eq!(
            set("[A]\nk = 1", "B", "j", "2").unwrap(),
            "[A]\nk = 1\n\n[B]\nj = 2"
        );
        assert_eq!(set("[A]\nk = 1", "A", "k", "3").unwrap(), "[A]\nk = 3");
    }

    #[test]
    fn matches_keys_only_in_their_section() {
        let t = "[A]\nk = 1\n[B]\nk = 1\n";
        assert_eq!(set(t, "B", "k", "2").unwrap(), "[A]\nk = 1\n[B]\nk = 2\n");
        assert_eq!(get(t, "B", "k").as_deref(), Some("1"));
        assert_eq!(get(t, "C", "k"), None);
    }

    #[test]
    fn top_level_keys_live_before_any_header() {
        let t = "video_vsync = \"true\"\n";
        assert_eq!(
            set(t, "", "video_fullscreen", "\"true\"").unwrap(),
            "video_vsync = \"true\"\nvideo_fullscreen = \"true\"\n"
        );
        assert_eq!(get(t, "", "video_vsync").as_deref(), Some("true"));
        let with_sections = "a = 1\n[S]\nb = 2\n";
        assert_eq!(
            set(with_sections, "", "c", "3").unwrap(),
            "a = 1\nc = 3\n[S]\nb = 2\n"
        );
        assert_eq!(
            set("[S]\nb = 2\n", "", "c", "3").unwrap(),
            "c = 3\n[S]\nb = 2\n"
        );
    }

    #[test]
    fn toml_arrays_are_values_not_sections() {
        let t = "[GPU]\nscales = [\n  [1, 2],\n  [3]\n]\nvsync = true\n[CPU]\nx = 1\n";
        assert_eq!(get(t, "GPU", "vsync").as_deref(), Some("true"));
        assert_eq!(get(t, "GPU", "scales"), None);
        let out = set(t, "GPU", "vsync", "false").unwrap();
        assert_eq!(out, t.replace("vsync = true", "vsync = false"));
        // A new key goes after the array's last line, not into it.
        let t = "[GPU]\nscales = [\n  1,\n  2,\n]\n[CPU]\n";
        assert_eq!(
            set(t, "GPU", "vsync", "true").unwrap(),
            "[GPU]\nscales = [\n  1,\n  2,\n]\nvsync = true\n[CPU]\n"
        );
        assert!(super::set(t, "GPU", "scales", "1").is_err());
        // A stray array line is no header either: the key stays in its section.
        let t = "[A]\n  [1, 2]\nk = 1\n";
        assert_eq!(get(t, "A", "k").as_deref(), Some("1"));
        assert_eq!(set(t, "A", "k", "2").unwrap(), "[A]\n  [1, 2]\nk = 2\n");
        // One line, brackets balanced: the next line is a key of its own.
        let t = "[A]\nv = [1, [2]]\nk = 1\n";
        assert_eq!(get(t, "A", "k").as_deref(), Some("1"));
        // An INI value with an open bracket swallows no header.
        let t = "[A]\nk = [\n[B]\nj = 1\n";
        assert_eq!(get(t, "B", "j").as_deref(), Some("1"));
    }

    #[test]
    fn headers_take_comments_and_toml_array_tables_are_not_sections() {
        let t = "[UI] ; window\nTheme = dark\n[[bindings]]\nkey = 1\n[[bindings]]\nkey = 2\n";
        assert_eq!(get(t, "UI", "Theme").as_deref(), Some("dark"));
        assert_eq!(get(t, "bindings", "key"), None);
        // A new key of [UI] goes into [UI], not after the last [[bindings]].
        assert_eq!(
            set(t, "UI", "Scale", "2").unwrap(),
            t.replace("Theme = dark\n", "Theme = dark\nScale = 2\n")
        );
        assert!(
            matches!(super::set(t, "bindings", "key", "3"), Err(e) if e.contains("array of tables"))
        );
        assert_eq!(header("[a.\"b c\"]"), Some(("a.\"b c\"", false)));
        assert_eq!(header("  [ Global ]  # x\r\n"), Some(("Global", false)));
        assert_eq!(header("[[t]]"), Some(("t", true)));
        for not in [
            "[1, 2]",
            "[]",
            "[a] b",
            "[a]]",
            "[[a]",
            "key = [1]",
            "[a[b]]",
        ] {
            assert_eq!(header(not), None, "{not}");
        }
    }

    #[test]
    fn toml_comments_after_a_value_stay() {
        let t = "[GPU]\ndraw_resolution_scale_x = 1    # Integer pixel width scale.\napi = \"any\" # Graphics system.\n";
        assert_eq!(
            get(t, "GPU", "draw_resolution_scale_x").as_deref(),
            Some("1")
        );
        assert_eq!(get(t, "GPU", "api").as_deref(), Some("any"));
        assert_eq!(set(t, "GPU", "draw_resolution_scale_x", "1"), None);
        assert_eq!(
            set(t, "GPU", "draw_resolution_scale_x", "3").unwrap(),
            t.replace("= 1    #", "= 3    #")
        );
        assert_eq!(
            set(t, "GPU", "api", "\"vulkan\"").unwrap(),
            t.replace("\"any\" #", "\"vulkan\" #")
        );
        // A value after which `#` would not be a comment drops it rather than absorb it.
        assert_eq!(
            set(t, "GPU", "api", "vulkan").unwrap(),
            t.replace("\"any\" # Graphics system.", "vulkan")
        );
        // In anything else `#` is the value's own.
        let ini = "[Pad]\nDevice = SDL/0/Pad #2\n";
        assert_eq!(get(ini, "Pad", "Device").as_deref(), Some("SDL/0/Pad #2"));
        assert_eq!(set(ini, "Pad", "Device", "SDL/0/Pad #2"), None);
        assert!(matches!(
            super::set(ini, "Pad", "Device", "1 # two"),
            Err(e) if e.contains("comment")
        ));
    }

    #[test]
    fn names_match_case_insensitively_when_asked() {
        let t = "[Core]\nGFXBackend = Vulkan\n";
        let ci = IniOpts {
            case_insensitive: true,
            ..IniOpts::default()
        };
        assert_eq!(
            set_with(t, "core", "gfxbackend", "OGL", ci).unwrap(),
            Some("[Core]\nGFXBackend = OGL\n".into())
        );
        assert_eq!(
            get_with(t, "CORE", "GFXBACKEND", ci).as_deref(),
            Some("Vulkan")
        );
        assert_eq!(get(t, "core", "GFXBackend"), None);
        // Case-sensitive by default: another spelling is another key.
        assert_eq!(
            set(t, "Core", "gfxbackend", "OGL").unwrap(),
            "[Core]\nGFXBackend = Vulkan\ngfxbackend = OGL\n"
        );
    }

    #[test]
    fn line_breaks_and_unreadable_names_are_refused() {
        let t = "[Pad]\nName = x\n";
        for (s, k, v) in [
            ("Pad", "Name", "a\n[Evil]\nk = 1"),
            ("Pad", "Name", "a\rb"),
            ("Pad", "Name", "a\0"),
            ("Pad", "Na\nme", "x"),
            ("P\nad", "Name", "x"),
            ("Pad", "a=b", "x"),
            ("Pad", "", "x"),
            ("Pad", "[x", "x"),
            ("Pad", " Name", "x"),
            ("P]ad", "Name", "x"),
            ("P,ad", "Name", "x"),
        ] {
            assert!(super::set(t, s, k, v).is_err(), "{s:?} {k:?} {v:?}");
        }
        // Readers trim a value; so does the writer.
        assert_eq!(set(t, "Pad", "Name", "  x "), None);
        assert_eq!(set(t, "Pad", "Name", " y ").unwrap(), "[Pad]\nName = y\n");
    }

    #[test]
    fn a_bom_is_kept_and_seen_through() {
        let t = "\u{feff}[UI]\nk = 1\n";
        assert_eq!(get(t, "UI", "k").as_deref(), Some("1"));
        assert_eq!(set(t, "UI", "k", "2").unwrap(), "\u{feff}[UI]\nk = 2\n");
        assert_eq!(
            set(t, "UI", "j", "2").unwrap(),
            "\u{feff}[UI]\nk = 1\nj = 2\n"
        );
    }

    #[test]
    fn qt_values_quote_and_escape_like_qsettings() {
        assert_eq!(qt_value("plain text"), "plain text");
        assert_eq!(qt_value("engine:sdl,port:0"), "\"engine:sdl,port:0\"");
        assert_eq!(qt_value("C:\\Games\\x"), "C:\\\\Games\\\\x");
        assert_eq!(qt_value("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(qt_value(" pad "), "\" pad \"");
        assert_eq!(qt_value("a=b;c"), "\"a=b;c\"");
        assert_eq!(qt_value("Pad #2"), "\"Pad #2\"");
        assert_eq!(qt_value("two\nlines\t"), "two\\nlines\\t");
        assert_eq!(qt_value("\u{1}a\u{1}g"), "\\x1\\x61\\x1g");
        assert_eq!(qt_value("\0"), "\\0");
        assert_eq!(qt_value("@ByteArray(x)"), "@@ByteArray(x)");
        assert_eq!(qt_value(""), "");
        // Whatever the text, the result is one line the editor takes and reads back whole.
        let qt = IniOpts {
            sep: "=",
            ..IniOpts::default()
        };
        for s in ["a, b", " x", "1 # 2", "\"q\"", "a\r\nb", "\\", "x;y"] {
            let v = qt_value(s);
            let out = set_with("[Controls]\n", "Controls", "name", &v, qt)
                .unwrap()
                .unwrap();
            assert_eq!(out.lines().count(), 2, "{s:?} → {out}");
            let back = get_with(&out, "Controls", "name", qt).unwrap();
            assert!(
                v == back || v == format!("\"{back}\""),
                "{s:?}: {v} → {back}"
            );
        }
    }

    // Property tests over generated files: sections of entries, comments, blanks, a TOML
    // array and an array of tables here and there, LF or CRLF, a final newline or not. The
    // model renders the file, and renders what `set` should leave: the two must match byte for
    // byte, so every line the edit does not own is unchanged.

    #[derive(Clone, Debug)]
    struct Entry {
        k: String,
        v: String,
        /// `  # a number` after the value.
        note: bool,
        /// `; about k` on the line before.
        remark: bool,
    }

    #[derive(Clone, Debug)]
    struct Section {
        name: String,
        entries: Vec<Entry>,
        /// A blank line and a comment before the header.
        remark: bool,
        /// A TOML array before the entries and an array of tables after them.
        toml: bool,
        /// Added by `set`: only a blank line before the header.
        fresh: bool,
    }

    #[derive(Clone, Debug)]
    struct Doc {
        root: Vec<Entry>,
        sections: Vec<Section>,
        spaced: bool,
        crlf: bool,
        final_nl: bool,
    }

    impl Doc {
        fn render(&self) -> String {
            let nl = if self.crlf { "\r\n" } else { "\n" };
            let sep = if self.spaced { " = " } else { "=" };
            let mut out = String::new();
            let entries = |out: &mut String, es: &[Entry]| {
                for e in es {
                    if e.remark {
                        out.push_str(&format!("; about {}{nl}", e.k));
                    }
                    let note = if e.note { "  # a number" } else { "" };
                    out.push_str(&format!("{}{sep}{}{note}{nl}", e.k, e.v));
                }
            };
            entries(&mut out, &self.root);
            for s in &self.sections {
                if s.fresh {
                    if !out.is_empty() {
                        out.push_str(nl);
                    }
                } else if s.remark {
                    out.push_str(&format!("{nl}# {}{nl}", s.name));
                }
                out.push_str(&format!("[{}]{nl}", s.name));
                if s.toml {
                    // `  [3]` is a nested array, not a section.
                    out.push_str(&format!("arr-x{sep}[{nl}  [1, 2],{nl}  [3]{nl}]{nl}"));
                }
                entries(&mut out, &s.entries);
                if s.toml {
                    out.push_str(&format!("[[{}.more]]{nl}x{sep}1{nl}", s.name));
                }
            }
            if !self.final_nl {
                while out.ends_with(['\r', '\n']) {
                    out.pop();
                }
            }
            out
        }

        fn every_key(&self) -> Vec<(String, String)> {
            let root = self.root.iter().map(|e| (String::new(), e.k.clone()));
            let rest = self
                .sections
                .iter()
                .flat_map(|s| s.entries.iter().map(move |e| (s.name.clone(), e.k.clone())));
            root.chain(rest).collect()
        }

        fn value(&self, section: &str, key: &str) -> Option<&str> {
            let es = if section.is_empty() {
                &self.root
            } else {
                &self.sections.iter().find(|s| s.name == section)?.entries
            };
            es.iter().find(|e| e.k == key).map(|e| e.v.as_str())
        }

        /// The document `set(section, key, value)` should leave.
        fn after(&self, section: &str, key: &str, value: &str) -> Doc {
            let mut d = self.clone();
            // A file without CRLF gets LF; an empty one has no final newline to keep, and none
            // has an entry to copy `=` from.
            let text = self.render();
            d.crlf &= text.contains("\r\n");
            d.final_nl |= text.is_empty();
            d.spaced |= self.root.is_empty()
                && self
                    .sections
                    .iter()
                    .all(|s| s.entries.is_empty() && !s.toml);
            // A comment stays after a value it still reads as a comment after.
            let shaped = value.parse::<u32>().is_ok() || value.starts_with('"');
            let new = Entry {
                k: key.into(),
                v: value.into(),
                note: false,
                remark: false,
            };
            let es = if section.is_empty() {
                Some(&mut d.root)
            } else {
                d.sections
                    .iter_mut()
                    .find(|s| s.name == section)
                    .map(|s| &mut s.entries)
            };
            match es {
                Some(es) => match es.iter_mut().find(|e| e.k == key) {
                    Some(e) => {
                        e.v = value.into();
                        e.note &= shaped;
                    }
                    None => es.push(new),
                },
                None => d.sections.push(Section {
                    name: section.into(),
                    entries: vec![new],
                    remark: false,
                    toml: false,
                    fresh: true,
                }),
            }
            d
        }
    }

    fn value() -> impl Strategy<Value = String> {
        prop_oneof![
            // Starting with a capital, so never a number or boolean a `#` could comment.
            "[A-Z][A-Za-z0-9/:._-]{0,5}( [A-Za-z0-9/:._#-]{1,6}){0,2}",
            (0u32..1000).prop_map(|n| n.to_string()),
            "\"[a-z ,#]{0,8}\"",
            Just(String::new()),
        ]
    }

    /// Few names, so sections share keys, and a section's key is often the one in the array of
    /// tables after it (`x`).
    const KEYS: [&str; 6] = ["x", "vsync", "Name", "scale", "Pad1/Up", "Device"];
    const KEY: &str = "(x|vsync|Name|scale|Pad1/Up|Device)";

    fn entries(toml: bool) -> impl Strategy<Value = Vec<Entry>> {
        (
            prop::collection::btree_map(KEY, value(), 0..5),
            any::<bool>(),
        )
            .prop_map(move |(m, comments)| {
                m.into_iter()
                    .enumerate()
                    .map(|(i, (k, v))| Entry {
                        note: toml && v.parse::<u32>().is_ok(),
                        remark: comments && i == 1,
                        k,
                        v,
                    })
                    .collect()
            })
    }

    fn doc() -> impl Strategy<Value = Doc> {
        any::<bool>()
            .prop_flat_map(|toml| {
                (
                    entries(toml),
                    prop::collection::btree_map(
                        "(UI|Main|GPU|3D\\.GL|EmuCore/GS|Pad1)",
                        (entries(toml), any::<bool>()),
                        0..4,
                    ),
                    Just(toml),
                    any::<[bool; 3]>(),
                )
            })
            .prop_map(|(root, sections, toml, [spaced, crlf, final_nl])| Doc {
                root,
                sections: sections
                    .into_iter()
                    .enumerate()
                    .map(|(n, (name, (entries, remark)))| Section {
                        name,
                        entries,
                        remark,
                        toml: toml && n == 0,
                        fresh: false,
                    })
                    .collect(),
                spaced,
                crlf,
                final_nl,
            })
    }

    /// A section and key of the document, or new ones: a key its section does not have,
    /// though others may.
    fn target(d: &Doc, pick: usize, fresh: bool) -> (String, String) {
        let all = d.every_key();
        if fresh || all.is_empty() {
            let section = match d.sections.get(pick % (d.sections.len() + 2)) {
                Some(s) => s.name.clone(),
                None if pick.is_multiple_of(2) => String::new(),
                None => "Fresh".into(),
            };
            let key = KEYS
                .iter()
                .find(|k| d.value(&section, k).is_none())
                .map_or("fresh_key", |k| k);
            (section, key.into())
        } else {
            all[pick % all.len()].clone()
        }
    }

    /// `v` without one pair of surrounding quotes, as [`get`] reads it.
    fn unquoted(v: &str) -> &str {
        v.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn set_changes_one_key_and_get_reads_it_back(
            d in doc(), pick in any::<usize>(), fresh in any::<bool>(), v in value(),
        ) {
            let text = d.render();
            let (s, k) = target(&d, pick, fresh);
            let old = d.value(&s, &k);
            let r = super::set(&text, &s, &k, &v).unwrap();
            let out = r.clone().unwrap_or_else(|| text.clone());
            prop_assert_eq!(&out, &d.after(&s, &k, &v).render());
            prop_assert_eq!(r.is_none(), old == Some(v.as_str()));
            let back = get(&out, &s, &k);
            prop_assert_eq!(back.as_deref(), Some(unquoted(&v)));
            prop_assert_eq!(super::set(&out, &s, &k, &v), Ok(None));
            for (os, ok) in d.every_key() {
                if (&os, &ok) != (&s, &k) {
                    let was = d.value(&os, &ok).map(unquoted);
                    let now = get(&out, &os, &ok);
                    prop_assert_eq!(now.as_deref(), was);
                }
            }
            // At most the one key, and a header with a blank line before it.
            let (gone, new) = changed(&text, &out);
            let section_existed = s.is_empty() || d.sections.iter().any(|x| x.name == s);
            match (old.is_some(), section_existed) {
                _ if r.is_none() => {}
                (true, _) => prop_assert_eq!((gone.len(), new.len()), (1, 1)),
                (false, true) => prop_assert_eq!((gone.len(), new.len()), (0, 1)),
                (false, false) => prop_assert!(gone.is_empty() && new.len() <= 3),
            }
            prop_assert!(same_endings(&text, &out));
        }

        #[test]
        fn a_line_break_never_adds_a_key(
            d in doc(), pick in any::<usize>(), fresh in any::<bool>(),
            a in "[a-z]{0,4}", b in "[a-z]{1,4}", cr in any::<bool>(),
        ) {
            let text = d.render();
            let (s, k) = target(&d, pick, fresh);
            let brk = if cr { "\r" } else { "\n" };
            let v = format!("{a}{brk}[{b}]{brk}{b} = 1");
            let broken_key = format!("{k}{brk}{b}");
            prop_assert!(super::set(&text, &s, &k, &v).is_err());
            prop_assert!(super::set(&text, &s, &broken_key, "1").is_err());
        }

        #[test]
        fn names_match_whatever_their_case_when_asked(
            d in doc(), pick in any::<usize>(), v in value(),
        ) {
            let all = d.every_key();
            prop_assume!(!all.is_empty());
            let (s, k) = &all[pick % all.len()];
            // Names in the generated files differ in more than case, so the flipped spelling
            // finds the one key and changes it as the exact spelling would.
            let flip = |n: &str| {
                n.chars()
                    .map(|c| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
                    .collect::<String>()
            };
            let lower: std::collections::BTreeSet<String> =
                all.iter().map(|(s, k)| format!("{}/{}", s.to_lowercase(), k.to_lowercase())).collect();
            prop_assume!(lower.len() == all.len());
            let ci = IniOpts { case_insensitive: true, ..IniOpts::default() };
            let text = d.render();
            prop_assert_eq!(
                set_with(&text, &flip(s), &flip(k), &v, ci),
                super::set(&text, s, k, &v)
            );
            prop_assert_eq!(get_with(&text, &flip(s), &flip(k), ci), get(&text, s, k));
        }

        #[test]
        fn qt_values_always_read_back_whole(s in "\\PC{0,12}", ctl in "[\\x00-\\x1f]{0,2}") {
            let v = qt_value(&format!("{s}{ctl}"));
            prop_assert!(!v.contains(['\n', '\r', '\0']));
            let opts = IniOpts { sep: "=", ..IniOpts::default() };
            let out = set_with("[Controls]\n", "Controls", "name", &v, opts).unwrap();
            let out = out.unwrap_or_else(|| "[Controls]\n".into());
            prop_assert_eq!(out.lines().count(), 2);
            let back = get_with(&out, "Controls", "name", opts);
            prop_assert_eq!(back.as_deref(), Some(unquoted(&v)));
        }
    }
}
