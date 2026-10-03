//! One element of an XML file set in place (Cemu's `settings.xml`): the element at a path from
//! the document element, each step a direct child of the last, has its text replaced, or is
//! created where it is missing, indented like its neighbours. No parser: attributes, comments,
//! the declaration and everything else stay byte for byte. Enough for settings files; not for
//! documents with mixed content.
use std::ops::Range;

/// `text` with the element at `path` (document element first) set to `value`; `Ok(None)` when
/// it already is. An empty file becomes a document with just that element. `value` is written
/// escaped (see [`escape`]); a character XML cannot hold at all, such as NUL, is an error, as
/// is a path step that is no element name.
pub fn set(text: &str, path: &[&str], value: &str) -> Result<Option<String>, String> {
    let Some((root, _)) = path.split_first() else {
        return Err("an empty element path".into());
    };
    for name in path {
        check_name(name)?;
    }
    if let Some(c) = value.chars().find(|&c| !allowed(c)) {
        return Err(format!("{value:?} holds {c:?}, which XML cannot"));
    }
    let escaped = escape(value);
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unit = indent_unit(text);
    if text.trim().is_empty() {
        let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{nl}");
        out.push_str(&tree(path, &escaped, "", &unit, nl));
        return Ok(Some(out));
    }
    let (mut lo, mut hi) = (0, text.len());
    let mut parent: Option<Element> = None;
    for (i, name) in path.iter().enumerate() {
        match child(text, lo..hi, name)? {
            Some(el) if i + 1 == path.len() => {
                let inner = &text[el.inner.clone()];
                if inner.contains('<') {
                    return Err(format!("<{name}> holds elements, not a value"));
                }
                if unescape(inner) == value {
                    return Ok(None);
                }
                let mut out = String::with_capacity(text.len() + escaped.len());
                if el.self_closing {
                    out.push_str(&text[..el.open.start]);
                    out.push_str(&opened(&text[el.open.clone()]));
                    out.push_str(&escaped);
                    out.push_str(&format!("</{name}>"));
                    out.push_str(&text[el.open.end..]);
                } else {
                    out.push_str(&text[..el.inner.start]);
                    out.push_str(&escaped);
                    out.push_str(&text[el.inner.end..]);
                }
                return Ok(Some(out));
            }
            Some(el) => {
                let inner = &text[el.inner.clone()];
                if !inner.trim().is_empty() && !inner.contains('<') {
                    return Err(format!("<{name}> holds a value, not elements"));
                }
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
                    &escaped,
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

/// The text of the element at `path`, unescaped; `None` when it is missing or holds elements.
/// A self-closing element holds the empty text.
pub fn get(text: &str, path: &[&str]) -> Option<String> {
    let (mut lo, mut hi) = (0, text.len());
    let mut found = None;
    for name in path {
        let el = child(text, lo..hi, name).ok()??;
        lo = el.inner.start;
        hi = el.inner.end;
        found = Some(el);
    }
    let inner = &text[found?.inner];
    (!inner.contains('<')).then(|| unescape(inner))
}

/// `s` as element text: `&`, `<` and `>` as entities, and a line break as a character
/// reference, so the element stays on its line and a reader gets the same characters back.
/// Characters XML cannot hold at all (NUL and the other C0 controls but tab) are left for the
/// caller to refuse; [`set`] does.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// Element text with its entities and character references resolved; anything else that
/// starts with `&` is left as it is.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let r = &rest[at..];
        let entity = r.find(';').and_then(|semi| {
            let name = &r[1..semi];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| name.strip_prefix('#').map(str::parse::<u32>))
                    .and_then(Result::ok)
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, semi + 1))
        });
        match entity {
            Some((c, len)) => {
                out.push(c);
                rest = &r[len..];
            }
            None => {
                out.push('&');
                rest = &r[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A character XML 1.0 can hold, escaped or not.
fn allowed(c: char) -> bool {
    !matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}')
}

/// A step of a path is an element name: a letter, `_` or `:` first, then those, digits, `-`
/// and `.`; no spaces, quotes, `<`, `>` or `/`.
fn check_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(format!("{name:?} is not an XML element name"))
    }
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

/// The first `name` element directly in `range` (an element's content, or the whole
/// document): elements nested deeper, comments, CDATA, processing instructions and the
/// doctype are stepped over.
fn child(text: &str, range: Range<usize>, name: &str) -> Result<Option<Element>, String> {
    let mut depth = 0usize;
    // The open tag of the match, while its content is being stepped over.
    let mut found: Option<Range<usize>> = None;
    let mut i = range.start;
    while let Some(rel) = text[i..range.end].find('<') {
        let s = i + rel;
        let rest = &text[s..range.end];
        let past = |end: &str| {
            rest.find(end)
                .map(|e| s + e + end.len())
                .ok_or_else(|| format!("unterminated markup at byte {s}"))
        };
        if rest.starts_with("<!--") {
            i = past("-->")?;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = past("]]>")?;
            continue;
        }
        if rest.starts_with("<?") {
            i = past("?>")?;
            continue;
        }
        if rest.starts_with("<!") {
            i = past(">")?;
            continue;
        }
        let end =
            tag_end(text, s, range.end).ok_or_else(|| format!("unterminated tag at byte {s}"))?;
        if rest.starts_with("</") {
            depth = depth
                .checked_sub(1)
                .ok_or_else(|| format!("a close tag without its open tag at byte {s}"))?;
            if depth == 0
                && let Some(open) = found
            {
                return Ok(Some(Element {
                    inner: open.end..s,
                    open,
                    self_closing: false,
                }));
            }
        } else {
            let self_closing = text[..end].ends_with("/>");
            let tag = &text[s + 1..end];
            let tag_name = tag
                .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .next()
                .unwrap_or_default();
            if depth == 0 && tag_name == name {
                if self_closing {
                    return Ok(Some(Element {
                        inner: end..end,
                        open: s..end,
                        self_closing: true,
                    }));
                }
                found = Some(s..end);
            }
            if !self_closing {
                depth += 1;
            }
        }
        i = end;
    }
    match found {
        Some(_) => Err(format!("unclosed <{name}>")),
        None => Ok(None),
    }
}

/// One past the `>` that ends the tag starting at `s`; a `>` in a quoted attribute value is
/// not it.
fn tag_end(text: &str, s: usize, to: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &b) in text.as_bytes()[..to].iter().enumerate().skip(s + 1) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(i + 1),
            None => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::ini::testkit::{changed, same_endings};
    use super::*;
    use proptest::prelude::*;

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
        assert!(
            matches!(set(CEMU, &["content", "fullscreen", "x"], "1"), Err(e) if e.contains("holds a value"))
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
        assert_eq!(get(t, &["content", "VSync"]).as_deref(), Some(""));
        assert_eq!(set(t, &["content", "VSync"], ""), Ok(None));
    }

    #[test]
    fn a_step_is_a_child_not_any_descendant() {
        let t = "<content>\n\t<G>\n\t\t<api>1</api>\n\t</G>\n\t<api>2</api>\n</content>\n";
        assert_eq!(get(t, &["content", "api"]).as_deref(), Some("2"));
        assert_eq!(
            set(t, &["content", "api"], "3").unwrap().unwrap(),
            t.replace("<api>2</api>", "<api>3</api>")
        );
        // Without its own `api`, `content` gets one; `G`'s stays.
        let t = "<content>\n\t<G>\n\t\t<api>1</api>\n\t</G>\n</content>\n";
        assert_eq!(get(t, &["content", "api"]), None);
        assert_eq!(
            set(t, &["content", "api"], "3").unwrap().unwrap(),
            "<content>\n\t<G>\n\t\t<api>1</api>\n\t</G>\n\t<api>3</api>\n</content>\n"
        );
        // The document element is the one at the top, not a nested namesake.
        let t = "<root>\n\t<content>\n\t\t<api>1</api>\n\t</content>\n</root>\n";
        assert_eq!(get(t, &["content", "api"]), None);
        assert!(set(t, &["content", "api"], "2").is_err());
        // Same-named elements nest; the inner one is not the outer one's end.
        let t = "<a><a><b>1</b></a><b>2</b></a>";
        assert_eq!(get(t, &["a", "b"]).as_deref(), Some("2"));
        assert_eq!(get(t, &["a", "a", "b"]).as_deref(), Some("1"));
        // Markup that looks like a tag is not one.
        let t = "<?xml version=\"1.0\"?>\n<!DOCTYPE content>\n<content note=\"<api>9</api>\">\n\t<!-- <api>8</api> -->\n\t<x><![CDATA[<api>7</api>]]></x>\n\t<api>1</api>\n</content>\n";
        assert_eq!(get(t, &["content", "api"]).as_deref(), Some("1"));
    }

    #[test]
    fn element_names_and_values_are_checked_and_escaped() {
        for bad in [
            "", "a b", "a<b", "a>b", "a/b", "a\"b", "1a", "-a", "a'b", "a=b",
        ] {
            assert!(set(CEMU, &["content", bad], "1").is_err(), "{bad:?}");
        }
        assert!(set(CEMU, &["content", "a\0"], "1").is_err());
        assert!(set(CEMU, &["content", "x"], "a\0b").is_err());
        assert!(set(CEMU, &["content", "x"], "\u{1b}").is_err());
        let out = set(CEMU, &["content", "Pad"], "A & B <1>\n\"2\"")
            .unwrap()
            .unwrap();
        assert!(
            out.contains("\t<Pad>A &amp; B &lt;1&gt;&#10;\"2\"</Pad>\n</content>"),
            "{out}"
        );
        assert_eq!(
            get(&out, &["content", "Pad"]).as_deref(),
            Some("A & B <1>\n\"2\"")
        );
        assert_eq!(set(&out, &["content", "Pad"], "A & B <1>\n\"2\""), Ok(None));
        assert_eq!(escape("a&b<c>d\r\n"), "a&amp;b&lt;c&gt;d&#13;&#10;");
        assert_eq!(
            unescape("&lt;&#x41;&#66;&quot;&apos;&bogus;&"),
            "<AB\"'&bogus;&"
        );
    }

    // Property tests over generated files: a document element with leaves and groups (some
    // self-closing, some with a `>` in an attribute), few names reused at every depth,
    // comments, tabs or spaces, LF or CRLF. The model renders the file, and renders what `set`
    // should leave: the two must match byte for byte.

    #[derive(Clone, Debug)]
    enum Node {
        /// Name, text, and whether empty text is written `<name/>`.
        Leaf(String, String, bool),
        Group {
            name: String,
            children: Vec<Node>,
            /// ` kind="a > b"` in the open tag.
            attr: bool,
            /// A comment before the first child.
            remark: bool,
        },
    }

    impl Node {
        fn name(&self) -> &str {
            match self {
                Node::Leaf(name, ..) | Node::Group { name, .. } => name,
            }
        }
    }

    #[derive(Clone, Debug)]
    struct Doc {
        nodes: Vec<Node>,
        remark: bool,
        unit: &'static str,
        crlf: bool,
    }

    fn render_nodes(nodes: &[Node], depth: usize, unit: &str, nl: &str, out: &mut String) {
        let ind = unit.repeat(depth);
        for n in nodes {
            match n {
                Node::Leaf(name, v, true) if v.is_empty() => {
                    out.push_str(&format!("{ind}<{name}/>{nl}"));
                }
                Node::Leaf(name, v, _) => {
                    out.push_str(&format!("{ind}<{name}>{}</{name}>{nl}", escape(v)));
                }
                Node::Group {
                    name,
                    children,
                    attr,
                    remark,
                } => {
                    let attr = if *attr { " kind=\"a > b\"" } else { "" };
                    if children.is_empty() {
                        out.push_str(&format!("{ind}<{name}{attr}/>{nl}"));
                        continue;
                    }
                    out.push_str(&format!("{ind}<{name}{attr}>{nl}"));
                    if *remark {
                        out.push_str(&format!("{ind}{unit}<!-- <api>9</api> -->{nl}"));
                    }
                    render_nodes(children, depth + 1, unit, nl, out);
                    out.push_str(&format!("{ind}</{name}>{nl}"));
                }
            }
        }
    }

    impl Doc {
        fn render(&self) -> String {
            let nl = if self.crlf { "\r\n" } else { "\n" };
            let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{nl}<content>{nl}");
            if self.remark {
                out.push_str(&format!("{}<!-- <x>0</x> -->{nl}", self.unit));
            }
            render_nodes(&self.nodes, 1, self.unit, nl, &mut out);
            out.push_str(&format!("</content>{nl}"));
            out
        }

        /// Every leaf's path and text.
        fn leaves(&self) -> Vec<(Vec<String>, String)> {
            fn walk(nodes: &[Node], at: &[String], out: &mut Vec<(Vec<String>, String)>) {
                for n in nodes {
                    let p = [at, &[n.name().to_string()]].concat();
                    match n {
                        Node::Leaf(_, v, _) => out.push((p, v.clone())),
                        Node::Group { children, .. } => walk(children, &p, out),
                    }
                }
            }
            let mut out = Vec::new();
            walk(&self.nodes, &["content".to_string()], &mut out);
            out
        }

        /// Every group's path, the document element's first, with its children.
        fn groups(&self) -> Vec<(Vec<String>, &[Node])> {
            fn walk<'a>(
                nodes: &'a [Node],
                at: &[String],
                out: &mut Vec<(Vec<String>, &'a [Node])>,
            ) {
                for n in nodes {
                    if let Node::Group { name, children, .. } = n {
                        let p = [at, std::slice::from_ref(name)].concat();
                        out.push((p.clone(), children));
                        walk(children, &p, out);
                    }
                }
            }
            let root = vec!["content".to_string()];
            let mut out = vec![(root.clone(), self.nodes.as_slice())];
            walk(&self.nodes, &root, &mut out);
            out
        }

        /// The document `set(path, value)` should leave.
        fn after(&self, path: &[&str], value: &str) -> Doc {
            let mut d = self.clone();
            // With nothing indented yet, the editor indents by a tab.
            if self.nodes.is_empty() && !self.remark {
                d.unit = "\t";
            }
            put(&mut d.nodes, &path[1..], value);
            d
        }
    }

    /// `value` at `path` under `nodes`: the leaf's text, or a new branch at the end.
    fn put(nodes: &mut Vec<Node>, path: &[&str], value: &str) {
        let Some((step, rest)) = path.split_first() else {
            return;
        };
        let Some(at) = nodes.iter().position(|n| n.name() == *step) else {
            let (leaf, groups) = path.split_last().unwrap_or((step, &[]));
            let mut branch = Node::Leaf((*leaf).into(), value.into(), false);
            for name in groups.iter().rev() {
                branch = Node::Group {
                    name: (*name).into(),
                    children: vec![branch],
                    attr: false,
                    remark: false,
                };
            }
            nodes.push(branch);
            return;
        };
        match &mut nodes[at] {
            Node::Leaf(_, v, short) => {
                // `<name/>` stays only when it already holds the empty text asked for.
                *short &= v.is_empty() && value.is_empty();
                *v = value.into();
            }
            Node::Group { children, .. } => put(children, rest, value),
        }
    }

    /// Few names, so an element and one nested deeper often share one.
    const NAMES: [&str; 6] = ["api", "VSync", "Graphic", "Pad", "x", "y"];
    const NAME: &str = "(api|VSync|Graphic|Pad|x|y)";

    fn unique(nodes: Vec<Node>) -> Vec<Node> {
        let mut seen = std::collections::BTreeSet::new();
        nodes
            .into_iter()
            .filter(|n| seen.insert(n.name().to_string()))
            .collect()
    }

    fn text() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9 &<>\"'\\n\\r\\t;#é]{0,10}"
    }

    fn nodes(flags: [bool; 3]) -> impl Strategy<Value = Vec<Node>> {
        let [short, attr, remark] = flags;
        let leaf = (NAME, text()).prop_map(move |(n, v)| Node::Leaf(n, v, short));
        let node = leaf.prop_recursive(2, 12, 4, move |inner| {
            (NAME, prop::collection::vec(inner, 0..4)).prop_map(move |(name, c)| {
                let children = unique(c);
                Node::Group {
                    name,
                    attr,
                    remark: remark && !children.is_empty(),
                    children,
                }
            })
        });
        prop::collection::vec(node, 0..5).prop_map(unique)
    }

    fn doc() -> impl Strategy<Value = Doc> {
        any::<[bool; 6]>().prop_flat_map(|[short, attr, remark, spaces, crlf, top_remark]| {
            nodes([short, attr, remark]).prop_map(move |nodes| Doc {
                nodes,
                remark: top_remark,
                unit: if spaces { "  " } else { "\t" },
                crlf,
            })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn set_changes_one_element_and_get_reads_it_back(
            d in doc(), pick in any::<usize>(), fresh in any::<bool>(),
            extra in prop::collection::vec(NAME, 0..2), v in text(),
        ) {
            let text = d.render();
            let all = d.leaves();
            // An existing leaf, or a new branch under a group, named as no child of the group
            // is, though an element deeper down may be.
            let path: Vec<String> = if fresh || all.is_empty() {
                let groups = d.groups();
                let (parent, children) = &groups[pick % groups.len()];
                let first = NAMES
                    .iter()
                    .find(|n| children.iter().all(|c| c.name() != **n))
                    .map_or("xNew".to_string(), |n| n.to_string());
                [parent.clone(), vec![first], extra.clone()].concat()
            } else {
                all[pick % all.len()].0.clone()
            };
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let old = all.iter().find(|(p, _)| *p == path).map(|(_, v)| v.as_str());
            let r = set(&text, &path, &v).unwrap();
            let out = r.clone().unwrap_or_else(|| text.clone());
            prop_assert_eq!(&out, &d.after(&path, &v).render());
            prop_assert_eq!(r.is_none(), old == Some(v.as_str()));
            let back = get(&out, &path);
            prop_assert_eq!(back.as_deref(), Some(v.as_str()));
            prop_assert_eq!(set(&out, &path, &v), Ok(None));
            for (other, was) in &all {
                let other: Vec<&str> = other.iter().map(String::as_str).collect();
                if other != path {
                    let now = get(&out, &other);
                    prop_assert_eq!(now.as_deref(), Some(was.as_str()));
                }
            }
            // The element's line, or one line per tag of the new branch whatever the text; a
            // self-closing parent's line becomes an open and a close tag around it.
            let (gone, new) = changed(&text, &out);
            if r.is_some() && old.is_some() {
                prop_assert_eq!((gone.len(), new.len()), (1, 1));
            } else if r.is_some() {
                prop_assert!(gone.len() <= 1);
                prop_assert_eq!(new.len(), 2 * (extra.len() + 1) - 1 + 2 * gone.len());
            }
            prop_assert!(same_endings(&text, &out));
        }
    }
}
