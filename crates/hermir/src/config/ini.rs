//! `key = value` under `[section]` headers, patched in place: PCSX2, DuckStation, Dolphin,
//! PPSSPP, Qt's `qt-config.ini`, flat TOML (xemu, Xenia, melonDS) and RetroArch's cfg are all
//! this shape to a line editor. Nothing is parsed beyond the line being changed, so comments,
//! order, unknown keys and the file's own spelling stay as they were.

/// `text` with `key` in `[section]` set to `value`; `None` when it already is. Every other
/// byte stays. A missing key goes after its section's last entry, a missing section at the
/// end, spelled as the file spells `key = value` (or `key=value`) and with its line endings.
/// An empty `section` is the part before any header (a RetroArch cfg has nothing else).
pub fn set(text: &str, section: &str, key: &str, value: &str) -> Option<String> {
    set_with(text, section, key, value, " = ")
}

/// [`set`], with `default_sep` (` = ` or `=`) for a file that has no entry to copy yet.
pub fn set_with(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
    default_sep: &str,
) -> Option<String> {
    let crlf = text.contains("\r\n");
    let nl = if crlf { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let sep = match lines.iter().find_map(|l| entry(l)) {
        Some((k, _)) if k.ends_with(' ') => " = ",
        Some(_) => "=",
        None => default_sep,
    };
    let mut current: Option<String> = Some(String::new());
    let mut last_entry: Option<usize> = None;
    let mut section_seen = section.is_empty();
    for (i, l) in lines.iter().enumerate() {
        if let Some(h) = header(l) {
            current = Some(h);
            if current.as_deref() == Some(section) {
                section_seen = true;
                last_entry = Some(i);
            }
            continue;
        }
        if current.as_deref() != Some(section) {
            continue;
        }
        let Some((left, right)) = entry(l) else {
            continue;
        };
        last_entry = Some(i);
        if left.trim() != key {
            continue;
        }
        let body = right.trim_end_matches(['\r', '\n']);
        if body.trim() == value {
            return None;
        }
        let lead = &body[..body.len() - body.trim_start().len()];
        let ending = &right[body.len()..];
        let mut out: String = lines[..i].concat();
        out.push_str(&format!("{left}={lead}{value}{ending}"));
        out.push_str(&lines[i + 1..].concat());
        return Some(out);
    }
    let line = format!("{key}{sep}{value}{nl}");
    if section_seen {
        let Some(at) = last_entry else {
            return Some(format!("{line}{text}"));
        };
        let mut out: String = lines[..=at].concat();
        if !out.ends_with('\n') {
            out.push_str(nl);
        }
        out.push_str(&line);
        out.push_str(&lines[at + 1..].concat());
        return Some(out);
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    if !out.is_empty() {
        out.push_str(nl);
    }
    out.push_str(&format!("[{section}]{nl}{line}"));
    Some(out)
}

/// The value of `key` in `[section]`, trimmed and unquoted; `None` when it is not there.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current: Option<String> = Some(String::new());
    for l in text.lines() {
        if let Some(h) = header(l) {
            current = Some(h);
            continue;
        }
        if current.as_deref() != Some(section) {
            continue;
        }
        if let Some((k, v)) = entry(l)
            && k.trim() == key
        {
            let v = v.trim();
            let unquoted = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            return Some(unquoted.to_string());
        }
    }
    None
}

/// `[ Global ]` (Supermodel) names the section Global.
fn header(line: &str) -> Option<String> {
    let t = line.trim();
    t.strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .map(|s| s.trim().to_string())
}

/// `(left of =, right of =)` for a `key = value` line; `None` for comments, headers, blanks.
fn entry(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with([';', '#', '[']) {
        return None;
    }
    line.split_once('=')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_one_value_and_nothing_else() {
        let t = "; top\n[UI]\nSetupWizardIncomplete = true\nTheme = dark\n\n[Filenames]\nBIOS = \n";
        let out = set(t, "UI", "SetupWizardIncomplete", "false").unwrap();
        assert_eq!(out, t.replace("Incomplete = true", "Incomplete = false"));
        assert_eq!(set(&out, "UI", "SetupWizardIncomplete", "false"), None);
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
        assert_eq!(
            set_with("", "UI", "fullscreen", "true", "=").unwrap(),
            "[UI]\nfullscreen=true\n"
        );
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
    }
}
