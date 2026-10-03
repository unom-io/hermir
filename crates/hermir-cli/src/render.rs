//! Human text: progress on stderr, results as lines.
use std::io::Write;

use hermir::progress::{Event, Progress};
use hermir::{InstallKind, Installed, KnobChange, PrepareStep, Support};

/// Progress for a terminal.
pub struct Stderr;

impl Progress for Stderr {
    fn on(&self, event: Event) {
        let mut err = std::io::stderr();
        let _ = match event {
            Event::Resolved {
                release,
                file_name,
                size,
            } => writeln!(
                err,
                "  {release}{}{}",
                file_name.map(|f| format!(" — {f}")).unwrap_or_default(),
                size.map(|s| format!(" ({} MB)", s / 1_048_576))
                    .unwrap_or_default()
            ),
            Event::Download { done, total } => match total {
                Some(t) if t > 0 => write!(err, "\r  downloading {:>3}%", done * 100 / t),
                _ => write!(err, "\r  downloading {} MB", done / 1_048_576),
            },
            Event::Verifying => writeln!(err, "\n  verifying"),
            Event::NotVerified { why } => writeln!(err, "\n  not verified: {why}"),
            Event::Extracting => writeln!(err, "  extracting"),
            Event::Placed => writeln!(err, "  done"),
            _ => Ok(()),
        };
    }
}

pub fn support_mark(s: Support) -> &'static str {
    match s {
        Support::Applied => "+",
        Support::Partial => "~",
        Support::Unsupported => "-",
        Support::Failed => "!",
        _ => "?",
    }
}

pub fn kind(k: InstallKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{k:?}").to_lowercase())
}

pub fn step(s: &PrepareStep) -> String {
    format!(
        "  {:?} {} {}{}",
        s.outcome,
        s.kind,
        s.target.display(),
        note(s.note.as_deref())
    )
}

pub fn knob(k: &KnobChange) -> String {
    format!(
        "  {} {:<17}{}{}",
        support_mark(k.support),
        k.knob,
        k.file
            .as_ref()
            .map(|f| format!(" {}", f.display()))
            .unwrap_or_default(),
        note(k.note.as_deref())
    )
}

fn note(n: Option<&str>) -> String {
    n.map(|n| format!(" — {n}")).unwrap_or_default()
}

/// The one line about shipped files the user had edited, when there are any.
pub fn kept(row: &Installed) -> Option<String> {
    match row.kept.as_slice() {
        [] => None,
        [one] => Some(format!(
            "kept {one}, which you edited; the new one is beside it as {one}.new"
        )),
        many => Some(format!(
            "{} files you edited were kept; the new ones are beside them as .new: {}",
            many.len(),
            many.join(", ")
        )),
    }
}
