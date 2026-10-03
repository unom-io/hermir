//! The neutral knobs of a patch turned into edits, by the catalog's bindings. No emulator is
//! named here: a knob is a file, a section, a key and a spelling, and the catalog has them.
use super::{Cx, Edit, txn};
use crate::model::{Binding, Entry, KNOBS, Knob, KnobChange, KnobValue, Os, Patch, Support};

/// What a neutral knob is asked to be.
#[derive(Clone, Copy)]
enum Neutral<'a> {
    Bool(bool),
    Scale(u8),
    Choice(&'static str),
    /// A device's name: `default`, or a name written into the binding's `text`.
    Text(&'a str),
    /// Milliseconds, within the binding's `range`.
    Millis(u32),
}

/// The neutral spellings a `values` map may carry for `knob`.
pub(crate) fn neutral_values(knob: &str) -> &'static [&'static str] {
    match knob {
        "video.aspect" => &["auto", "4:3", "16:9", "stretch"],
        "region" => &["auto", "jp", "us", "eu"],
        "audio.device" => &["default"],
        _ => &[],
    }
}

/// One `KnobChange` per knob the patch carries, each with the edits behind it (none when it
/// is unsupported).
pub(crate) fn plan(entry: &Entry, cx: &Cx, patch: &Patch) -> Vec<(KnobChange, Vec<Edit>)> {
    let mut wanted: Vec<(&str, Neutral)> = Vec::new();
    if let Some(v) = &patch.video {
        if let Some(f) = v.fullscreen {
            wanted.push(("video.fullscreen", Neutral::Bool(f)));
        }
        if let Some(n) = v.scale {
            wanted.push(("video.scale", Neutral::Scale(n)));
        }
        if let Some(s) = v.vsync {
            wanted.push(("video.vsync", Neutral::Bool(s)));
        }
        if let Some(a) = v.aspect {
            wanted.push(("video.aspect", Neutral::Choice(a.as_str())));
        }
    }
    if let Some(r) = patch.region {
        wanted.push(("region", Neutral::Choice(r.as_str())));
    }
    if let Some(a) = &patch.audio {
        if let Some(d) = &a.device {
            wanted.push(("audio.device", Neutral::Text(d)));
        }
        if let Some(ms) = a.latency_ms {
            wanted.push(("audio.latency_ms", Neutral::Millis(ms)));
        }
    }
    let mut out = Vec::new();
    for (name, neutral) in wanted {
        let binding = entry.config.as_ref().and_then(|c| c.knobs.get(name));
        // A device no file names still reaches an emulator that plays through Pulse on Linux.
        if name == "audio.device" && !matches!(binding, Some(Knob::Bound(_))) && cx.os == Os::Linux
        {
            out.push((
                KnobChange {
                    knob: name.into(),
                    support: Support::Partial,
                    note: Some(PULSE_SINK.into()),
                    file: None,
                },
                Vec::new(),
            ));
            continue;
        }
        let planned = match binding {
            None => Err(format!("not described for {} yet", entry.id)),
            Some(Knob::Unsupported { unsupported }) => Err(unsupported.clone()),
            Some(Knob::Launch { note, .. }) => {
                out.push((
                    KnobChange {
                        knob: name.into(),
                        support: Support::Applied,
                        note: Some(on_launch(note.as_deref())),
                        file: None,
                    },
                    Vec::new(),
                ));
                continue;
            }
            Some(Knob::Bound(b)) => render(entry, cx, name, b, neutral),
        };
        out.push(match planned {
            Ok((edits, file, note)) => (
                KnobChange {
                    knob: name.into(),
                    support: if note.is_some() {
                        Support::Partial
                    } else {
                        Support::Applied
                    },
                    note,
                    file: Some(file),
                },
                edits,
            ),
            Err(note) => (
                KnobChange {
                    knob: name.into(),
                    support: Support::Unsupported,
                    note: Some(note),
                    file: None,
                },
                Vec::new(),
            ),
        });
    }
    out
}

/// How `audio.device` reaches an emulator with no device setting, on Linux.
pub(crate) const PULSE_SINK: &str = "no setting names the device: on Linux `launch` sets \
    PULSE_SINK, which PulseAudio and PipeWire's Pulse server honour for an emulator that plays \
    through Pulse (one on PipeWire's own API or ALSA does not)";

type Rendered = Result<(Vec<Edit>, std::path::PathBuf, Option<String>), String>;

fn render(entry: &Entry, cx: &Cx, knob: &str, b: &Binding, neutral: Neutral) -> Rendered {
    let (path, file) = cx.file(&b.file)?;
    let literal = match neutral {
        Neutral::Text(_) | Neutral::Millis(_) => text(entry, knob, b, neutral)?,
        _ => spell(
            entry,
            knob,
            neutral,
            b.bool.as_ref(),
            b.values.as_ref(),
            b.scale.as_ref(),
            None,
        )?,
    };
    let mut edits = Edit::set(&path, file, &b.section, &b.key, &literal);
    for a in &b.also {
        let (a_path, a_file) = match &a.file {
            Some(name) => cx.file(name)?,
            None => (path.clone(), file),
        };
        let value = spell(
            entry,
            knob,
            neutral,
            a.bool.as_ref(),
            a.values.as_ref(),
            None,
            a.value.as_deref().map(|v| v.replace("{value}", &literal)),
        )?;
        let section = a.section.as_deref().unwrap_or(&b.section);
        edits.extend(Edit::set(&a_path, a_file, section, &a.key, &value));
    }
    Ok((edits, path, b.note.clone()))
}

/// A device's name or a number of milliseconds, written into the binding's `text`; a device
/// the binding's `values` names (`default`) as spelled there.
fn text(entry: &Entry, knob: &str, b: &Binding, neutral: Neutral) -> Result<String, String> {
    let template = b
        .text
        .as_deref()
        .ok_or_else(|| format!("{knob} of {} has no text", entry.id))?;
    let value = match neutral {
        Neutral::Text(name) => {
            if let Some(l) = b.values.as_ref().and_then(|v| v.get(name)) {
                return Ok(l.clone());
            }
            name.to_string()
        }
        Neutral::Millis(ms) => {
            if let Some([lo, hi]) = b.range
                && !(lo..=hi).contains(&ms)
            {
                return Err(format!("{ms} ms is outside {}'s {lo}–{hi} ms", entry.id));
            }
            ms.to_string()
        }
        _ => return Err(format!("{knob} is not text")),
    };
    Ok(template.replace("{value}", &value))
}

/// The emulator's literal for `neutral`, by whichever spelling the binding carries.
fn spell(
    entry: &Entry,
    knob: &str,
    neutral: Neutral,
    bool_: Option<&[String; 2]>,
    values: Option<&std::collections::BTreeMap<String, String>>,
    scale: Option<&crate::model::Scale>,
    literal: Option<String>,
) -> Result<String, String> {
    if let Some(l) = literal {
        return Ok(l);
    }
    match neutral {
        Neutral::Bool(v) => bool_
            .map(|[t, f]| if v { t.clone() } else { f.clone() })
            .ok_or_else(|| format!("{knob} of {} has no true/false spelling", entry.id)),
        Neutral::Scale(n) => {
            let s = scale.ok_or_else(|| format!("{knob} of {} has no scale", entry.id))?;
            let (min, max) = s.range();
            s.render(n)
                .ok_or_else(|| format!("{}× is outside {}'s {min}×–{max}×", n, entry.id))
        }
        Neutral::Choice(c) => values
            .and_then(|m| m.get(c))
            .cloned()
            .ok_or_else(|| format!("{} has no {knob} {c}", entry.id)),
        Neutral::Text(_) | Neutral::Millis(_) => Err(format!("{knob} of {} is text", entry.id)),
    }
}

/// What each knob reads in this copy's files, spelled back in its neutral form.
pub(crate) fn read(entry: &Entry, cx: &Cx) -> Vec<KnobValue> {
    KNOBS
        .iter()
        .map(|&name| {
            let mut v = KnobValue {
                knob: name.into(),
                value: None,
                literal: None,
                file: None,
                note: None,
            };
            let binding = entry.config.as_ref().and_then(|c| c.knobs.get(name));
            let b = match binding {
                None => {
                    v.note = Some(format!("not described for {} yet", entry.id));
                    return v;
                }
                Some(Knob::Unsupported { unsupported }) => {
                    v.note = Some(unsupported.clone());
                    return v;
                }
                Some(Knob::Launch { .. }) => {
                    v.note = Some("set at launch; no file keeps it".into());
                    return v;
                }
                Some(Knob::Bound(b)) => b,
            };
            let (path, file) = match cx.file(&b.file) {
                Ok(f) => f,
                Err(why) => {
                    v.note = Some(why);
                    return v;
                }
            };
            v.file = Some(path.clone());
            match txn::read(&path, file, &b.section, &b.key) {
                Ok(Some(literal)) => {
                    v.value = neutral(name, b, &literal);
                    if v.value.is_none() {
                        v.note = Some("a value hermir has no neutral name for".into());
                    }
                    v.literal = Some(literal);
                }
                Ok(None) => v.note = Some("not set: the emulator's default".into()),
                Err(why) => v.note = Some(why),
            }
            v
        })
        .collect()
}

/// `literal` spelled back as the knob's neutral value, by the binding that writes it. The
/// catalog spells a value with the quotes its format wants (`"true"` in a RetroArch cfg,
/// `"16x9"` in TOML); a reader may hand it back without them, so quotes do not count.
fn neutral(knob: &str, b: &Binding, literal: &str) -> Option<String> {
    let unquote = |s: &str| -> String {
        ['"', '\'']
            .iter()
            .find_map(|q| s.strip_prefix(*q).and_then(|r| r.strip_suffix(*q)))
            .unwrap_or(s)
            .to_string()
    };
    let read = unquote(literal);
    let is = |spelled: &str| unquote(spelled) == read;
    if let Some([t, f]) = &b.bool {
        return if is(t) {
            Some("true".into())
        } else if is(f) {
            Some("false".into())
        } else {
            None
        };
    }
    if let Some(values) = &b.values
        && let Some(n) = neutral_values(knob)
            .iter()
            .find(|n| values.get(**n).is_some_and(|l| is(l)))
    {
        return Some(n.to_string());
    }
    if let Some(template) = &b.text {
        // The template's text around `{value}` peeled off what was read.
        let t = unquote(template);
        let (before, after) = t.split_once("{value}")?;
        let value = read.strip_prefix(before)?.strip_suffix(after)?;
        return match knob {
            "audio.latency_ms" => value.trim().parse::<u32>().ok().map(|n| n.to_string()),
            _ => (!value.is_empty()).then(|| value.to_string()),
        };
    }
    if b.values.is_some() {
        return None;
    }
    if let Some(scale) = &b.scale {
        let (min, max) = scale.range();
        return (min..=max)
            .find(|n| scale.render(*n).is_some_and(|l| is(&l)))
            .map(|n| n.to_string());
    }
    None
}

/// The note of a knob that travels on the command line.
pub(crate) fn on_launch(note: Option<&str>) -> String {
    let base = "on launch: `launch` adds it when the request carries this patch";
    match note {
        Some(n) => format!("{base}; {n}"),
        None => base.into(),
    }
}
