//! One element of an XML file set in place (Cemu's `settings.xml`): the element at a path from
//! the document element has its text replaced, or is created where it is missing, indented
//! like its neighbours. No parser: attributes, comments, the declaration and everything else
//! stay byte for byte. Enough for settings files; not for documents with mixed content.
use std::ops::Range;

/// `text` with the element at `path` (document element first) set to `value`; `Ok(None)` when
/// it already is. An empty file becomes a document with just that element.
pub fn set(text: &str, path: &[&str], value: &str) -> Result<Option<String>, String> {
    let Some((root, _)) = path.split_first() else {
        return Err("an empty element path".into());
    };
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unit = indent_unit(text);
    if text.trim().is_empty() {
        let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{nl}");
        out.push_str(&tree(path, value, "", &unit, nl));
        return Ok(Some(out));
    }
    let (mut lo, mut hi) = (0, text.len());
    let mut parent: Option<Element> = None;
    for (i, name) in path.iter().enumerate() {
        match find(text, lo..hi, name)? {
            Some(el) if i + 1 == path.len() => {
                let inner = &text[el.inner.clone()];
                if inner.contains('<') {
                    return Err(format!("<{name}> holds elements, not a value"));
                }
                if !el.self_closing && inner.trim() == value {
                    return Ok(None);
                }
                let mut out = String::with_capacity(text.len() + value.len());
                if el.self_closing {
                    out.push_str(&text[..el.open.start]);
                    out.push_str(&opened(&text[el.open.clone()]));
                    out.push_str(value);
                    out.push_str(&format!("</{name}>"));
                    out.push_str(&text[el.open.end..]);
                } else {
                    out.push_str(&text[..el.inner.start]);
                    out.push_str(value);
                    out.push_str(&text[el.inner.end..]);
                }
                return Ok(Some(out));
            }
            Some(el) => {
                lo = el.inner.start;
                hi = el.inner.end;
                parent = Some(el);
            }
            None => {
                let Some(p) = parent else {
                    return Err(format!("no <{root}> element"));
                };
                let parent_indent = line_indent(text, p.open.start);
                let branch = tree(
                    &path[i..],
                    value,
                    &format!("{parent_indent}{unit}"),
                    &unit,
                    nl,
                );
                let mut out = String::with_capacity(text.len() + branch.len() + 16);
                if p.self_closing {
                    out.push_str(&text[..p.open.start]);
                    out.push_str(&opened(&text[p.open.clone()]));
                    out.push_str(nl);
                    out.push_str(&branch);
                    out.push_str(&parent_indent);
                    out.push_str(&format!("</{}>", path[i - 1]));
                    out.push_str(&text[p.open.end..]);
                } else {
                    let before = &text[..p.inner.end];
                    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
                    if before[line_start..].trim().is_empty() && line_start > p.inner.start {
                        // The close tag starts its line: the branch goes in front of it.
                        out.push_str(&text[..line_start]);
                        out.push_str(&branch);
                        out.push_str(&text[line_start..]);
                    } else {
                        out.push_str(before);
                        out.push_str(nl);
                        out.push_str(&branch);
                        out.push_str(&parent_indent);
                        out.push_str(&text[p.inner.end..]);
                    }
                }
                return Ok(Some(out));
            }
        }
    }
    Err("an empty element path".into())
}

struct Element {
    /// The open tag, `<name …>` or `<name/>`.
    open: Range<usize>,
    /// Between the open and close tags; empty for a self-closing element.
    inner: Range<usize>,
    self_closing: bool,
}

/// `<name a="b"/>` → `<name a="b">`.
fn opened(tag: &str) -> String {
    let t = tag.trim_end_matches('>').trim_end_matches('/').trim_end();
    format!("{t}>")
}

/// `<a>`, `<b>` … `<last>value</last>` … `</b>`, `</a>`, one per line, from `indent`.
fn tree(path: &[&str], value: &str, indent: &str, unit: &str, nl: &str) -> String {
    let mut out = String::new();
    let depth = path.len() - 1;
    for (i, name) in path[..depth].iter().enumerate() {
        out.push_str(&format!("{indent}{}<{name}>{nl}", unit.repeat(i)));
    }
    out.push_str(&format!(
        "{indent}{}<{leaf}>{value}</{leaf}>{nl}",
        unit.repeat(depth),
        leaf = path[depth]
    ));
    for (i, name) in path[..depth].iter().enumerate().rev() {
        out.push_str(&format!("{indent}{}</{name}>{nl}", unit.repeat(i)));
    }
    out
}

/// The shortest leading whitespace of any indented line; a tab when nothing is indented.
fn indent_unit(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| &l[..l.len() - l.trim_start_matches([' ', '\t']).len()])
        .filter(|ws| !ws.is_empty())
        .min_by_key(|ws| ws.len())
        .unwrap_or("\t")
        .to_string()
}

fn line_indent(text: &str, pos: usize) -> String {
    let start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
    let line = &text[start..];
    line[..line.len() - line.trim_start_matches([' ', '\t']).len()].to_string()
}

/// The first `name` element in `range`, comments skipped, same-named children nested.
fn find(text: &str, range: Range<usize>, name: &str) -> Result<Option<Element>, String> {
    let Some(s) = open_tag(text, range.start, range.end, name) else {
        return Ok(None);
    };
    let Some(gt) = text[s..range.end].find('>') else {
        return Err(format!("unterminated <{name}>"));
    };
    let open = s..s + gt + 1;
    if text[open.clone()].ends_with("/>") {
        return Ok(Some(Element {
            inner: open.end..open.end,
            open,
            self_closing: true,
        }));
    }
    let mut depth = 0usize;
    let mut cur = open.end;
    loop {
        let next_open = open_tag(text, cur, range.end, name);
        let next_close = close_tag(text, cur, range.end, name);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                let end = text[o..].find('>').map_or(range.end, |g| o + g + 1);
                if !text[o..end].ends_with("/>") {
                    depth += 1;
                }
                cur = end;
            }
            (_, Some(c)) => {
                if depth == 0 {
                    return Ok(Some(Element {
                        open: open.clone(),
                        inner: open.end..c,
                        self_closing: false,
                    }));
                }
                depth -= 1;
                cur = text[c..].find('>').map_or(range.end, |g| c + g + 1);
            }
            (_, None) => return Err(format!("unclosed <{name}>")),
        }
    }
}

/// Start of the next `<name` open tag at or after `from`, skipping comments.
fn open_tag(text: &str, from: usize, to: usize, name: &str) -> Option<usize> {
    let mut i = from;
    while i < to {
        let rel = text[i..to].find('<')?;
        let s = i + rel;
        if text[s..].starts_with("<!--") {
            i = text[s..].find("-->").map_or(to, |e| s + e + 3);
            continue;
        }
        let rest = &text[s + 1..to];
        if rest.starts_with(name)
            && rest[name.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        {
            return Some(s);
        }
        i = s + 1;
    }
    None
}

/// Start of the next `</name>` at or after `from`, skipping comments.
fn close_tag(text: &str, from: usize, to: usize, name: &str) -> Option<usize> {
    let mut i = from;
    while i < to {
        let rel = text[i..to].find('<')?;
        let s = i + rel;
        if text[s..].starts_with("<!--") {
            i = text[s..].find("-->").map_or(to, |e| s + e + 3);
            continue;
        }
        let rest = &text[s..to];
        if let Some(after) = rest.strip_prefix("</").and_then(|r| r.strip_prefix(name))
            && after.trim_start().starts_with('>')
        {
            return Some(s);
        }
        i = s + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEMU: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<fullscreen>false</fullscreen>\n\t<console_region>255</console_region>\n\t<Graphic>\n\t\t<api>1</api>\n\t\t<VSync>0</VSync>\n\t</Graphic>\n\t<Audio>\n\t\t<api>3</api>\n\t</Audio>\n</content>\n";

    #[test]
    fn replaces_text_at_a_path_and_nothing_else() {
        let out = set(CEMU, &["content", "Graphic", "VSync"], "1")
            .unwrap()
            .unwrap();
        assert_eq!(out, CEMU.replace("<VSync>0</VSync>", "<VSync>1</VSync>"));
        assert_eq!(set(&out, &["content", "Graphic", "VSync"], "1"), Ok(None));
        // `api` under Graphic, not the one under Audio.
        let out = set(CEMU, &["content", "Audio", "api"], "0")
            .unwrap()
            .unwrap();
        assert!(
            out.contains("<Graphic>\n\t\t<api>1</api>")
                && out.contains("<Audio>\n\t\t<api>0</api>")
        );
        let out = set(CEMU, &["content", "fullscreen"], "true")
            .unwrap()
            .unwrap();
        assert!(out.contains("<fullscreen>true</fullscreen>"));
        assert!(
            matches!(set(CEMU, &["content", "Graphic"], "x"), Err(e) if e.contains("holds elements"))
        );
    }

    #[test]
    fn creates_missing_elements_indented_like_their_neighbours() {
        let out = set(CEMU, &["content", "Graphic", "FullscreenScaling"], "1")
            .unwrap()
            .unwrap();
        assert!(
            out.contains(
                "\t\t<VSync>0</VSync>\n\t\t<FullscreenScaling>1</FullscreenScaling>\n\t</Graphic>\n"
            ),
            "{out}"
        );
        let out = set(CEMU, &["content", "Input", "Pad", "x"], "7")
            .unwrap()
            .unwrap();
        assert!(
            out.ends_with("\t</Audio>\n\t<Input>\n\t\t<Pad>\n\t\t\t<x>7</x>\n\t\t</Pad>\n\t</Input>\n</content>\n"),
            "{out}"
        );
    }

    #[test]
    fn a_seeded_or_empty_file_grows_the_document() {
        let seed = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content/>\n";
        let out = set(seed, &["content", "console_region"], "4")
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<console_region>4</console_region>\n</content>\n"
        );
        let out = set(&out, &["content", "Graphic", "VSync"], "1")
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<console_region>4</console_region>\n\t<Graphic>\n\t\t<VSync>1</VSync>\n\t</Graphic>\n</content>\n"
        );
        let out = set("", &["content", "fullscreen"], "true")
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<content>\n\t<fullscreen>true</fullscreen>\n</content>\n"
        );
        let inline = "<content><a>1</a></content>";
        assert_eq!(
            set(inline, &["content", "b"], "2").unwrap().unwrap(),
            "<content><a>1</a>\n\t<b>2</b>\n</content>"
        );
        assert!(
            matches!(set("<other/>", &["content", "x"], "1"), Err(e) if e.contains("no <content>"))
        );
    }

    #[test]
    fn comments_and_self_closing_leaves() {
        let t = "<content>\n\t<!-- <VSync>9</VSync> -->\n\t<VSync/>\n</content>\n";
        let out = set(t, &["content", "VSync"], "1").unwrap().unwrap();
        assert_eq!(
            out,
            "<content>\n\t<!-- <VSync>9</VSync> -->\n\t<VSync>1</VSync>\n</content>\n"
        );
    }
}
