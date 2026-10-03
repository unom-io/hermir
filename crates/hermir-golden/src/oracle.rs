//! Independent readers for the formats hermir patches. Each flattens a file into keys, so two
//! versions of it compare value by value, and says what in it is not well-formed. None of them
//! shares code with hermir's own editors: a bug there must not hide here.
use std::collections::BTreeMap;

use hermir::Format;
use serde::{Deserialize, Serialize};

/// How a fixture file is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Oracle {
    /// `[section]` and `key = value` lines; also Qt's ini and RetroArch's cfg.
    Ini,
    /// TOML, parsed by the `toml` crate; tables are sections, dotted as written.
    Toml,
    Yaml,
    /// ares' BML: an indented tree of `name` and `name: value` lines.
    Bml,
    Xml,
    Json,
}

/// `(section, key)` → value. Nested formats join the path above the key with `/`.
pub type Flat = BTreeMap<(String, String), String>;

impl Oracle {
    /// The reader for a catalog file of `format` at `path`.
    pub fn for_format(format: Format, path: &str) -> Oracle {
        match format {
            Format::Ini if path.ends_with(".toml") => Oracle::Toml,
            Format::Ini | Format::Qt => Oracle::Ini,
            Format::Yaml => Oracle::Yaml,
            Format::Bml => Oracle::Bml,
            Format::Xml => Oracle::Xml,
            Format::Json => Oracle::Json,
            _ => Oracle::for_path(path).unwrap_or(Oracle::Ini),
        }
    }

    /// The reader for a file nobody named, by its extension.
    pub fn for_path(path: &str) -> Option<Oracle> {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        Some(match ext.as_str() {
            "ini" | "cfg" | "conf" => Oracle::Ini,
            "toml" => Oracle::Toml,
            "yml" | "yaml" => Oracle::Yaml,
            "bml" => Oracle::Bml,
            "xml" => Oracle::Xml,
            "json" => Oracle::Json,
            _ => return None,
        })
    }

    /// The file's keys, or why it is not well-formed.
    pub fn read(self, text: &str) -> Result<Flat, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        match self {
            Oracle::Ini => ini(text),
            Oracle::Toml => toml(text),
            Oracle::Yaml => yaml(text),
            Oracle::Bml => Ok(bml(text)),
            Oracle::Xml => xml(text),
            Oracle::Json => json(text),
        }
    }
}

fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .enumerate()
        .map(|(i, l)| (i + 1, l))
}

fn ini(text: &str) -> Result<Flat, String> {
    let mut out = Flat::new();
    let mut section = String::new();
    let mut bad = Vec::new();
    for (n, line) in lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
            continue;
        }
        if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            section = inner.trim().to_string();
            continue;
        }
        match t.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => {
                out.insert(
                    (section.clone(), k.trim().to_string()),
                    v.trim().to_string(),
                );
            }
            _ => bad.push(format!("line {n}: {t}")),
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(format!("not a section, key or comment: {}", bad.join("; ")))
    }
}

fn toml(text: &str) -> Result<Flat, String> {
    let table: toml::Table = text.parse().map_err(|e| format!("TOML: {e}"))?;
    let mut out = Flat::new();
    fn walk(prefix: &str, table: &toml::Table, out: &mut Flat) {
        for (k, v) in table {
            match v {
                toml::Value::Table(t) => {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&p, t, out);
                }
                other => {
                    out.insert((prefix.to_string(), k.clone()), other.to_string());
                }
            }
        }
    }
    walk("", &table, &mut out);
    Ok(out)
}

fn insert_path(out: &mut Flat, path: &[String], value: String) {
    let (key, above) = path.split_last().expect("a path has a key");
    out.insert((above.join("/"), key.clone()), value);
}

fn yaml(text: &str) -> Result<Flat, String> {
    use yaml_rust2::{Yaml, YamlLoader};
    let docs = YamlLoader::load_from_str(text).map_err(|e| format!("YAML: {e}"))?;
    let mut out = Flat::new();
    fn scalar(y: &Yaml) -> String {
        match y {
            Yaml::String(s) | Yaml::Real(s) => s.clone(),
            Yaml::Integer(i) => i.to_string(),
            Yaml::Boolean(b) => b.to_string(),
            Yaml::Null => String::new(),
            other => format!("{other:?}"),
        }
    }
    fn walk(path: &mut Vec<String>, y: &Yaml, out: &mut Flat) {
        match y {
            Yaml::Hash(h) => {
                for (k, v) in h {
                    path.push(scalar(k));
                    walk(path, v, out);
                    path.pop();
                }
            }
            other if !path.is_empty() => insert_path(out, path, scalar(other)),
            _ => {}
        }
    }
    if let Some(doc) = docs.first() {
        walk(&mut Vec::new(), doc, &mut out);
    }
    Ok(out)
}

fn bml(text: &str) -> Flat {
    let mut out = Flat::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (_, line) in lines(text) {
        if line.trim().is_empty() || line.trim_start().starts_with("//") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let t = line.trim();
        let (name, value) = match t.split_once(':') {
            Some((n, v)) => (n.trim().to_string(), Some(v.trim().to_string())),
            None => (t.to_string(), None),
        };
        if let Some(v) = value {
            let mut path: Vec<String> = stack.iter().map(|(_, n)| n.clone()).collect();
            path.push(name.clone());
            insert_path(&mut out, &path, v);
        }
        stack.push((indent, name));
    }
    out
}

fn xml(text: &str) -> Result<Flat, String> {
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("XML: {e}"))?;
    let mut out = Flat::new();
    fn walk(node: roxmltree::Node, path: &mut Vec<String>, out: &mut Flat) {
        for a in node.attributes() {
            path.push(format!("@{}", a.name()));
            insert_path(out, path, a.value().to_string());
            path.pop();
        }
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut children = false;
        for child in node.children().filter(|c| c.is_element()) {
            children = true;
            let name = child.tag_name().name();
            let n = seen.entry(name).or_default();
            *n += 1;
            path.push(if *n == 1 {
                name.to_string()
            } else {
                format!("{name}#{n}")
            });
            walk(child, path, out);
            path.pop();
        }
        if !children {
            insert_path(out, path, node.text().unwrap_or("").trim().to_string());
        }
    }
    let root = doc.root_element();
    walk(
        root,
        &mut vec![root.tag_name().name().to_string()],
        &mut out,
    );
    Ok(out)
}

fn json(text: &str) -> Result<Flat, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
    let mut out = Flat::new();
    fn walk(path: &mut Vec<String>, v: &serde_json::Value, out: &mut Flat) {
        match v {
            serde_json::Value::Object(o) => {
                for (k, v) in o {
                    path.push(k.clone());
                    walk(path, v, out);
                    path.pop();
                }
            }
            serde_json::Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    path.push(format!("[{i}]"));
                    walk(path, v, out);
                    path.pop();
                }
            }
            scalar if !path.is_empty() => insert_path(out, path, scalar.to_string()),
            _ => {}
        }
    }
    walk(&mut Vec::new(), &value, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(f: &'a Flat, section: &str, key: &str) -> Option<&'a str> {
        f.get(&(section.to_string(), key.to_string()))
            .map(String::as_str)
    }

    #[test]
    fn ini_reads_sections_and_flags_what_is_not_ini() {
        let f = Oracle::Ini
            .read("\u{feff}top = 1\r\n[UI]\r\n; c\r\nStartFullscreen = true\r\n")
            .unwrap();
        assert_eq!(get(&f, "", "top"), Some("1"));
        assert_eq!(get(&f, "UI", "StartFullscreen"), Some("true"));
        assert!(Oracle::Ini.read("[UI]\nnot a key\n").is_err());
    }

    #[test]
    fn nested_formats_flatten_to_their_last_key() {
        let f = Oracle::Toml
            .read("[display.window]\nfullscreen_on_startup = true\n")
            .unwrap();
        assert_eq!(
            get(&f, "display.window", "fullscreen_on_startup"),
            Some("true")
        );
        let f = Oracle::Yaml
            .read("Video:\n  VSync Mode: Full\n  Vulkan:\n    Adapter: \"\"\n")
            .unwrap();
        assert_eq!(get(&f, "Video", "VSync Mode"), Some("Full"));
        assert_eq!(get(&f, "Video/Vulkan", "Adapter"), Some(""));
        let f = Oracle::Xml
            .read("<content><Graphic><api>1</api><api>2</api></Graphic></content>")
            .unwrap();
        assert_eq!(get(&f, "content/Graphic", "api"), Some("1"));
        assert_eq!(get(&f, "content/Graphic", "api#2"), Some("2"));
        let f = Oracle::Json
            .read(r#"{"GPU": {"full_screen": false, "name": "x"}}"#)
            .unwrap();
        assert_eq!(get(&f, "GPU", "full_screen"), Some("false"));
        assert_eq!(get(&f, "GPU", "name"), Some("\"x\""));
        let f = Oracle::Bml.read("Video\n  Driver: OpenGL 3.2\n  Blocking: true\n");
        assert_eq!(get(&f.unwrap(), "Video", "Blocking"), Some("true"));
    }

    #[test]
    fn broken_files_say_so() {
        assert!(Oracle::Json.read(r#"{"a": 1} "b": 2}"#).is_err());
        assert!(Oracle::Yaml.read("a:\n  b: [\n").is_err());
        assert!(Oracle::Xml.read("<a><b></a>").is_err());
        assert!(Oracle::Toml.read("[a]\nb = \n").is_err());
    }
}
