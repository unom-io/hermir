//! `hermir` on the command line. Nouns first, verbs second, `--json` everywhere; human text
//! and progress go to stderr, data to stdout, exit codes from `Error::exit_code`.
use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use hermir::progress::{Event, Progress};
use hermir::{Aspect, Catalog, Hermir, Options, Os, Region, StepOutcome, Support};

#[derive(Parser)]
#[command(
    name = "hermir",
    version,
    about = "One interface for managing emulators"
)]
struct Cli {
    /// Print data as JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// The managed prefix (default: the OS data directory + hermir).
    #[arg(long, global = true, env = "HERMIR_PREFIX")]
    prefix: Option<PathBuf>,
    /// Resolve for another OS. Only `resolve` and `catalog` are meaningful across OSes.
    #[arg(long, global = true, value_parser = clap::value_parser!(Os))]
    os: Option<Os>,
    /// Refuse a download there is nothing to check against (exit 4), instead of installing it
    /// as not verified.
    #[arg(long, global = true, env = "HERMIR_REQUIRE_VERIFIED")]
    require_verified: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// The catalog: what hermir knows.
    Catalog {
        #[command(subcommand)]
        cmd: CatalogCmd,
    },
    /// Managed and detected emulators, and whether an update exists.
    Status {
        emulator: Option<String>,
        /// Ask every archive channel whether it moved (one request each).
        #[arg(long)]
        check: bool,
    },
    /// Emulators the user installed, fresh from the machine.
    Detect,
    /// Install (or reinstall) from the channel for this OS.
    Install { emulator: String },
    /// Reinstall when the channel moved.
    Update {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        emulator: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Remove a managed install; `--purge` removes its data too.
    Remove {
        emulator: String,
        #[arg(long)]
        purge: bool,
    },
    /// The exe and config root of the best copy on this machine.
    Where { emulator: String },
    /// Make every copy ready to play: answer its first-run questions, place or install
    /// `--firmware` for `--platform`.
    Prepare {
        emulator: String,
        #[arg(long)]
        platform: Option<String>,
        #[arg(long)]
        firmware: Vec<PathBuf>,
    },
    /// Write pads into every copy's own bindings, seat by seat, or with `--revert` put the
    /// player's settings back. A pad is `vendor:product[:name]` (hex ids); none means one
    /// Xbox 360 pad in seat 1.
    Players {
        emulator: String,
        #[arg(long)]
        revert: bool,
        #[arg(long)]
        pad: Vec<String>,
    },
    /// A session's settings: write them into every copy, put them back, or see what each
    /// emulator takes.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// libretro cores for the RetroArch on this machine.
    Core {
        #[command(subcommand)]
        cmd: CoreCmd,
    },
    /// What this machine can and cannot do.
    Doctor,
}

/// `vendor:product[:name]` → the pad at `index`.
fn pad_arg(spec: &str, index: u32) -> hermir::Result<hermir::PadRef> {
    let mut parts = spec.splitn(3, ':');
    let hex =
        |s: Option<&str>| s.and_then(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    let (Some(vendor), Some(product)) = (hex(parts.next()), hex(parts.next())) else {
        return Err(hermir::Error::Place {
            what: spec.into(),
            why: "a pad is vendor:product[:name], hex ids".into(),
        });
    };
    let mut pad = hermir::PadRef::xbox360(index);
    pad.vendor = vendor;
    pad.product = product;
    if let Some(name) = parts.next() {
        pad.name = name.into();
    }
    Ok(pad)
}

/// `section/key=value` → a native key for `file`; the last slash before `=` splits them.
fn native_arg(file: &str, spec: &str) -> hermir::Result<hermir::Native> {
    let Some((left, value)) = spec.split_once('=') else {
        return Err(hermir::Error::Place {
            what: spec.into(),
            why: "a native key is section/key=value".into(),
        });
    };
    let (section, key) = match left.rsplit_once('/') {
        Some((s, k)) => (s, k),
        None => ("", left),
    };
    if key.trim().is_empty() {
        return Err(hermir::Error::Place {
            what: spec.into(),
            why: "a native key is section/key=value".into(),
        });
    }
    Ok(hermir::Native {
        file: file.into(),
        section: section.trim().into(),
        key: key.trim().into(),
        value: value.into(),
    })
}

fn pads_arg(pad: &[String]) -> hermir::Result<Vec<hermir::Player>> {
    let pads: Vec<hermir::PadRef> = pad
        .iter()
        .zip(0u32..)
        .map(|(spec, i)| pad_arg(spec, i))
        .collect::<hermir::Result<_>>()?;
    Ok(pads
        .into_iter()
        .zip(1u8..)
        .map(|(pad, seat)| hermir::Player { seat, pad })
        .collect())
}

fn support_mark(s: Support) -> &'static str {
    match s {
        Support::Applied => "+",
        Support::Partial => "~",
        Support::Unsupported => "-",
        Support::Failed => "!",
    }
}

fn step_line(s: &hermir::PrepareStep) -> String {
    format!(
        "  {:?} {} {}{}",
        s.outcome,
        s.kind,
        s.target.display(),
        s.note
            .as_deref()
            .map(|n| format!(" — {n}"))
            .unwrap_or_default()
    )
}

#[derive(Subcommand)]
enum CatalogCmd {
    /// Every entry, with what it offers on this OS.
    List,
    /// One entry, in full.
    Show { emulator: String },
    /// Parse a catalog directory strictly and apply the rules a schema cannot.
    Validate { dir: PathBuf },
    /// The JSON Schema of an entry.
    Schema,
    /// Where each channel points right now, without downloading. `--all` walks the catalog.
    Resolve(ResolveArgs),
}

#[derive(Args)]
struct ResolveArgs {
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    emulator: Option<String>,
    #[arg(long)]
    all: bool,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Write video, region, native keys and pads into every copy of an emulator. Every file
    /// is snapshotted first; `config revert` puts it back.
    Apply(ApplyArgs),
    /// Put the player's own files back: one emulator, or every one with changes outstanding.
    Revert {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        emulator: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// What `apply` can do, knob by knob, for one emulator or all of them.
    Support { emulator: Option<String> },
}

#[derive(Args)]
struct ApplyArgs {
    emulator: String,
    /// Start in fullscreen (`--fullscreen` alone means yes; `--fullscreen no` says no).
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_parser = clap::builder::BoolishValueParser::new())]
    fullscreen: Option<bool>,
    /// Internal resolution, as a multiple of the console's: 1 is native, 3 is 3×.
    #[arg(long)]
    scale: Option<u8>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_parser = clap::builder::BoolishValueParser::new())]
    vsync: Option<bool>,
    /// auto, 4:3, 16:9 or stretch.
    #[arg(long, value_parser = clap::value_parser!(Aspect))]
    aspect: Option<Aspect>,
    /// auto, jp, us or eu (also ntsc-j, ntsc-u, pal).
    #[arg(long, value_parser = clap::value_parser!(Region))]
    region: Option<Region>,
    /// A key the model does not cover, as `section/key=value` (`key=value` at the top
    /// level), written as given into `--file`. The section may itself contain slashes.
    #[arg(long = "set", value_name = "SECTION/KEY=VALUE")]
    set: Vec<String>,
    /// The catalog's name for the file `--set` goes into.
    #[arg(long, default_value = "main")]
    file: String,
    /// A pad into the bindings, seat by seat: `vendor:product[:name]`, hex ids.
    #[arg(long)]
    pad: Vec<String>,
}

#[derive(Subcommand)]
enum CoreCmd {
    /// Fetch `<core>_libretro` from the buildbot into RetroArch's cores directory.
    Install { core: String },
}

struct Stderr;

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
        };
    }
}

/// The one line about shipped files the user had edited, when there are any.
fn kept_line(row: &hermir::Installed) -> Option<String> {
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

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(()) => {}
        Err(e) => {
            if json {
                let _ = writeln!(
                    std::io::stdout(),
                    "{}",
                    serde_json::json!({ "error": e.to_string(), "exit": e.exit_code() })
                );
            }
            eprintln!("hermir: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

fn out<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    } else {
        let s = human();
        if !s.is_empty() {
            println!("{s}");
        }
    }
}

fn run(cli: Cli) -> hermir::Result<()> {
    let json = cli.json;
    if let Cmd::Catalog { cmd } = &cli.cmd {
        match cmd {
            CatalogCmd::Schema => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Catalog::entry_schema()).unwrap_or_default()
                );
                return Ok(());
            }
            CatalogCmd::Validate { dir } => {
                let c = Catalog::from_dir(dir)?;
                out(
                    json,
                    &serde_json::json!({ "entries": c.entries().len(), "platforms": c.platforms().len() }),
                    || {
                        format!(
                            "ok: {} entries, {} platforms",
                            c.entries().len(),
                            c.platforms().len()
                        )
                    },
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let h = Hermir::open(Options {
        prefix: cli.prefix,
        os: cli.os,
        require_verified: cli.require_verified,
        ..Options::default()
    })?;
    let os = h.os();

    match cli.cmd {
        Cmd::Catalog { cmd } => match cmd {
            CatalogCmd::List => {
                let rows: Vec<serde_json::Value> = h
                    .catalog()
                    .entries()
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "id": e.id, "name": e.name, "platforms": e.platforms,
                            "channel": e.channels.get(os).map(|c| c.kind()),
                            "no_install": e.no_install,
                        })
                    })
                    .collect();
                out(json, &rows, || {
                    h.catalog()
                        .entries()
                        .iter()
                        .map(|e| {
                            format!(
                                "{:<15} {:<22} {:<10} {}",
                                e.id,
                                e.name,
                                e.channels.get(os).map(|c| c.kind()).unwrap_or(
                                    if e.no_install.is_some() {
                                        "no-install"
                                    } else {
                                        "-"
                                    }
                                ),
                                e.platforms.join(",")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            CatalogCmd::Show { emulator } => {
                let e = h.emulator(&emulator)?.entry().clone();
                out(json, &e, || {
                    serde_json::to_string_pretty(&e).unwrap_or_default()
                });
            }
            CatalogCmd::Resolve(args) => {
                let ids: Vec<String> = if args.all {
                    h.catalog().offered_on(os).map(|e| e.id.clone()).collect()
                } else {
                    vec![
                        args.emulator
                            .ok_or_else(|| hermir::Error::NotInCatalog("<none>".into()))?,
                    ]
                };
                let mut rows = Vec::new();
                let mut failed = 0;
                for id in ids {
                    match h.emulator(&id)?.resolve() {
                        Ok(r) => {
                            if !json {
                                println!(
                                    "{:<15} {:<8} {:<28} {}",
                                    r.emulator,
                                    r.channel,
                                    r.release,
                                    r.file_name.clone().unwrap_or_default()
                                );
                            }
                            rows.push(serde_json::to_value(r).unwrap_or_default());
                        }
                        Err(e) => {
                            failed += 1;
                            if !json {
                                println!("{id:<15} FAILED   {e}");
                            }
                            rows.push(
                                serde_json::json!({ "emulator": id, "error": e.to_string() }),
                            );
                        }
                    }
                }
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&rows).unwrap_or_default()
                    );
                }
                if failed > 0 {
                    return Err(hermir::Error::Catalog {
                        entry: "resolve".into(),
                        why: format!("{failed} entries did not resolve"),
                    });
                }
            }
            CatalogCmd::Schema | CatalogCmd::Validate { .. } => unreachable!("handled above"),
        },
        Cmd::Status { emulator, check } => {
            let rows = h.status(emulator.as_deref(), check)?;
            out(json, &rows, || {
                rows.iter()
                    .filter(|s| s.managed.is_some() || !s.detected.is_empty() || emulator.is_some())
                    .map(|s| {
                        let mut line = format!("{:<15} {}", s.emulator, s.name);
                        if let Some(m) = &s.managed {
                            line.push_str(&format!(
                                "\n  managed  {}{}",
                                m.exe,
                                m.version
                                    .as_ref()
                                    .map(|v| format!("  v{v}"))
                                    .unwrap_or_default()
                            ));
                            if let Some(u) = &s.update {
                                line.push_str(&format!("  → update: {u}"));
                            }
                        }
                        for d in &s.detected {
                            line.push_str(&format!(
                                "\n  {:<8} {}",
                                format!("{:?}", d.kind).to_lowercase(),
                                d.exe
                            ));
                        }
                        if s.managed.is_none() && s.detected.is_empty() {
                            line.push_str(if s.offered {
                                "\n  not installed"
                            } else {
                                "\n  not offered here"
                            });
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Cmd::Detect => {
            let found = h.detect();
            out(json, &found, || {
                if found.is_empty() {
                    "nothing detected".into()
                } else {
                    found
                        .iter()
                        .map(|i| {
                            format!(
                                "{:<15} {:<9} {}",
                                i.emulator,
                                format!("{:?}", i.kind).to_lowercase(),
                                i.exe
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
        }
        Cmd::Install { emulator } => {
            let e = h.emulator(&emulator)?;
            if !json {
                eprintln!("installing {}", e.entry().name);
            }
            let row = e.install(&Stderr)?;
            out(json, &row, || {
                let mut s = format!("{}: {}", row.emulator, row.exe);
                if let Some(kept) = kept_line(&row) {
                    s.push('\n');
                    s.push_str(&kept);
                }
                s
            });
        }
        Cmd::Update { emulator, all } => {
            let ids: Vec<String> = if all {
                h.store()
                    .installed()?
                    .into_iter()
                    .map(|r| r.emulator)
                    .collect()
            } else {
                vec![emulator.ok_or_else(|| hermir::Error::NotInCatalog("<none>".into()))?]
            };
            let mut rows = Vec::new();
            for id in ids {
                let e = h.emulator(&id)?;
                match e.update(&Stderr)? {
                    Some(row) => {
                        if !json {
                            println!(
                                "{id}: updated to {}",
                                row.release.clone().unwrap_or_default()
                            );
                            if let Some(kept) = kept_line(&row) {
                                println!("{kept}");
                            }
                        }
                        rows.push(serde_json::json!({ "emulator": id, "updated": row }));
                    }
                    None => {
                        if !json {
                            println!("{id}: current");
                        }
                        rows.push(serde_json::json!({ "emulator": id, "updated": null }));
                    }
                }
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).unwrap_or_default()
                );
            }
        }
        Cmd::Remove { emulator, purge } => {
            h.emulator(&emulator)?.remove(purge)?;
            out(
                json,
                &serde_json::json!({ "removed": emulator, "purged": purge }),
                || format!("{emulator}: removed"),
            );
        }
        Cmd::Where { emulator } => {
            let best = h.emulator(&emulator)?.best()?;
            match best {
                Some(i) => out(json, &i, || {
                    format!(
                        "{}\nconfig: {}",
                        i.exe,
                        i.config_root
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "unknown".into())
                    )
                }),
                None => {
                    return Err(hermir::Error::Place {
                        what: emulator,
                        why: "not on this machine".into(),
                    });
                }
            }
        }
        Cmd::Prepare {
            emulator,
            platform,
            firmware,
        } => {
            let e = h.emulator(&emulator)?;
            let copies = e.copies()?;
            if copies.is_empty() {
                return Err(hermir::Error::Place {
                    what: emulator,
                    why: "not on this machine".into(),
                });
            }
            let done: Vec<_> = copies
                .iter()
                .map(|i| {
                    e.prepare(i, platform.as_deref(), &firmware)
                        .map(|p| (i.exe.to_string(), p))
                })
                .collect::<hermir::Result<_>>()?;
            let rows: Vec<serde_json::Value> = done
                .iter()
                .map(|(exe, p)| serde_json::json!({ "exe": exe, "steps": p.steps }))
                .collect();
            out(json, &rows, || {
                done.iter()
                    .flat_map(|(exe, p)| {
                        std::iter::once(exe.clone()).chain(p.steps.iter().map(|s| {
                            format!(
                                "  {:?} {} {}{}",
                                s.outcome,
                                s.kind,
                                s.target.display(),
                                s.note
                                    .as_deref()
                                    .map(|n| format!(" — {n}"))
                                    .unwrap_or_default()
                            )
                        }))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            let failed = done
                .iter()
                .flat_map(|(_, p)| &p.steps)
                .any(|s| s.outcome == hermir::StepOutcome::Failed);
            if failed {
                std::process::exit(5);
            }
        }
        Cmd::Players {
            emulator,
            revert,
            pad,
        } => {
            let e = h.emulator(&emulator)?;
            let done: Vec<(String, Vec<hermir::PrepareStep>)> = if revert {
                vec![("revert".to_string(), e.revert()?)]
            } else {
                let players = if pad.is_empty() {
                    vec![hermir::Player {
                        seat: 1,
                        pad: hermir::PadRef::xbox360(0),
                    }]
                } else {
                    pads_arg(&pad)?
                };
                e.copies()?
                    .iter()
                    .map(|i| {
                        e.apply_players(i, &players)
                            .map(|p| (i.exe.to_string(), p.steps))
                    })
                    .collect::<hermir::Result<_>>()?
            };
            let rows: Vec<serde_json::Value> = done
                .iter()
                .map(|(exe, steps)| serde_json::json!({ "exe": exe, "steps": steps }))
                .collect();
            out(json, &rows, || {
                done.iter()
                    .flat_map(|(exe, steps)| {
                        std::iter::once(exe.clone()).chain(steps.iter().map(|s| {
                            format!(
                                "  {:?} {} {}{}",
                                s.outcome,
                                s.kind,
                                s.target.display(),
                                s.note
                                    .as_deref()
                                    .map(|n| format!(" — {n}"))
                                    .unwrap_or_default()
                            )
                        }))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if done
                .iter()
                .flat_map(|(_, s)| s)
                .any(|s| s.outcome == hermir::StepOutcome::Failed)
            {
                std::process::exit(5);
            }
        }
        Cmd::Config {
            cmd: ConfigCmd::Apply(a),
        } => {
            let e = h.emulator(&a.emulator)?;
            let video = hermir::Video {
                fullscreen: a.fullscreen,
                scale: a.scale,
                vsync: a.vsync,
                aspect: a.aspect,
            };
            let patch = hermir::Patch {
                players: (!a.pad.is_empty()).then(|| pads_arg(&a.pad)).transpose()?,
                video: (!video.is_empty()).then_some(video),
                region: a.region,
                native: a
                    .set
                    .iter()
                    .map(|s| native_arg(&a.file, s))
                    .collect::<hermir::Result<_>>()?,
            };
            if patch.is_empty() {
                return Err(hermir::Error::Place {
                    what: a.emulator,
                    why: "nothing to apply; see `hermir config apply --help`".into(),
                });
            }
            let copies = e.copies()?;
            if copies.is_empty() {
                return Err(hermir::Error::Place {
                    what: a.emulator,
                    why: "not on this machine".into(),
                });
            }
            let done: Vec<(String, hermir::Applied)> = copies
                .iter()
                .map(|i| e.apply(i, &patch).map(|a| (i.exe.to_string(), a)))
                .collect::<hermir::Result<_>>()?;
            let rows: Vec<serde_json::Value> = done
                .iter()
                .map(|(exe, a)| {
                    serde_json::json!({ "exe": exe, "knobs": a.knobs, "steps": a.steps })
                })
                .collect();
            out(json, &rows, || {
                done.iter()
                    .flat_map(|(exe, a)| {
                        std::iter::once(exe.clone())
                            .chain(a.knobs.iter().map(|k| {
                                format!(
                                    "  {} {:<17}{}{}",
                                    support_mark(k.support),
                                    k.knob,
                                    k.file
                                        .as_ref()
                                        .map(|f| format!(" {}", f.display()))
                                        .unwrap_or_default(),
                                    k.note
                                        .as_deref()
                                        .map(|n| format!(" — {n}"))
                                        .unwrap_or_default()
                                )
                            }))
                            .chain(a.steps.iter().map(step_line))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if done.iter().any(|(_, a)| a.failed()) {
                std::process::exit(5);
            }
        }
        Cmd::Config {
            cmd: ConfigCmd::Revert { emulator, all },
        } => {
            let done: Vec<(String, Vec<hermir::PrepareStep>)> = if all {
                h.revert_all()?
            } else {
                let id = emulator.ok_or_else(|| hermir::Error::NotInCatalog("<none>".into()))?;
                vec![(id.clone(), h.emulator(&id)?.revert()?)]
            };
            let rows: Vec<serde_json::Value> = done
                .iter()
                .map(|(id, steps)| serde_json::json!({ "emulator": id, "steps": steps }))
                .collect();
            out(json, &rows, || {
                if done.iter().all(|(_, s)| s.is_empty()) {
                    return "nothing outstanding".into();
                }
                done.iter()
                    .flat_map(|(id, steps)| {
                        std::iter::once(id.clone()).chain(steps.iter().map(step_line))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if done
                .iter()
                .flat_map(|(_, s)| s)
                .any(|s| s.outcome == StepOutcome::Failed)
            {
                std::process::exit(5);
            }
        }
        Cmd::Config {
            cmd: ConfigCmd::Support { emulator },
        } => {
            let matrix: Vec<(String, Vec<hermir::KnobChange>)> = match &emulator {
                Some(id) => vec![(id.clone(), h.emulator(id)?.support())],
                None => h.support(),
            };
            let rows: Vec<serde_json::Value> = matrix
                .iter()
                .map(|(id, knobs)| serde_json::json!({ "emulator": id, "knobs": knobs }))
                .collect();
            out(json, &rows, || {
                if let [(id, knobs)] = matrix.as_slice()
                    && emulator.is_some()
                {
                    return std::iter::once(id.clone())
                        .chain(knobs.iter().map(|k| {
                            format!(
                                "  {} {:<17}{}",
                                support_mark(k.support),
                                k.knob,
                                k.note
                                    .as_deref()
                                    .map(|n| format!(" {n}"))
                                    .unwrap_or_default()
                            )
                        }))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                let names: Vec<&str> = matrix
                    .first()
                    .map(|(_, k)| k.iter().map(|k| k.knob.as_str()).collect())
                    .unwrap_or_default();
                let mut lines = vec![format!(
                    "{:<15} {}",
                    "",
                    names
                        .iter()
                        .map(|n| format!("{:<11}", n.trim_start_matches("video.")))
                        .collect::<String>()
                )];
                for (id, knobs) in &matrix {
                    lines.push(format!(
                        "{:<15} {}",
                        id,
                        knobs
                            .iter()
                            .map(|k| format!("{:<11}", support_mark(k.support)))
                            .collect::<String>()
                    ));
                }
                lines.push(
                    "+ applied   ~ partial   - unsupported; `config support <emulator>` says why"
                        .into(),
                );
                lines.join("\n")
            });
        }
        Cmd::Core {
            cmd: CoreCmd::Install { core },
        } => {
            let path = h.install_core(&core, &Stderr)?;
            out(
                json,
                &serde_json::json!({ "core": core, "path": path }),
                || path.display().to_string(),
            );
        }
        Cmd::Doctor => {
            let prefix = h.store().root().to_path_buf();
            let writable = std::fs::create_dir_all(&prefix).is_ok()
                && std::fs::write(prefix.join(".doctor"), b"")
                    .is_ok_and(|_| std::fs::remove_file(prefix.join(".doctor")).is_ok());
            let flatpak = os != Os::Linux || which("flatpak");
            let report = serde_json::json!({
                "os": os, "prefix": prefix, "prefix_writable": writable,
                "flatpak": if os == Os::Linux { Some(flatpak) } else { None },
                "catalog_entries": h.catalog().entries().len(),
                "offered_here": h.catalog().offered_on(os).count(),
                "managed": h.store().installed()?.len(),
                "detected": h.detect().len(),
                "github_token": std::env::var("GITHUB_TOKEN").is_ok_and(|t| !t.is_empty()),
            });
            out(json, &report, || {
                let mut s = format!(
                    "os: {os}\nprefix: {} ({})",
                    prefix.display(),
                    if writable { "writable" } else { "NOT writable" }
                );
                if os == Os::Linux {
                    s.push_str(&format!(
                        "\nflatpak: {}",
                        if flatpak {
                            "found"
                        } else {
                            "missing — the Linux channel needs it"
                        }
                    ));
                }
                s.push_str(&format!(
                    "\ncatalog: {} entries, {} offered here\nmanaged: {}, detected: {}",
                    report["catalog_entries"],
                    report["offered_here"],
                    report["managed"],
                    report["detected"]
                ));
                s
            });
        }
    }
    Ok(())
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}
