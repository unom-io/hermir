//! `hermir` on the command line. Nouns first, verbs second, `--json` everywhere; human text
//! and progress go to stderr, data to stdout, exit codes from `Error::exit_code`.
use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use hermir::progress::{Event, Progress};
use hermir::{Catalog, Hermir, Options, Os, Store};

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
    /// libretro cores for the RetroArch on this machine.
    Core {
        #[command(subcommand)]
        cmd: CoreCmd,
    },
    /// What this machine can and cannot do.
    Doctor,
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
    emulator: Option<String>,
    #[arg(long)]
    all: bool,
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
            Event::Extracting => writeln!(err, "  extracting"),
            Event::Placed => writeln!(err, "  done"),
        };
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
            out(json, &row, || format!("{}: {}", row.emulator, row.exe));
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

#[allow(dead_code)]
fn _store_is_used(_: &Store) {}
