//! unclip — outside-of-LLM possibility engine (CLI entry point).

mod commands;
mod db;
mod matching;
mod sampling;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use unclip_io::{split_frame_selector, Format};
use unclip_store::FrameRepository;

use commands::{parse_kv, AddInput, QueryInput};
use sampling::{
    parse_format, parse_under_override, ComposeInput, FilterInput, SampleInput, UnderOverride,
};

#[derive(Parser)]
#[command(name = "unclip", version, about = "Outside-of-LLM possibility engine")]
struct Cli {
    /// Path to the SQLite database file.
    #[arg(long, global = true, default_value = "unclip.db")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create and migrate the database.
    Init,

    /// Add a new branch.
    Add {
        /// Slash-separated scope address, e.g. /ikebukuro/station/exit.
        path: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        weight: f64,
        /// One-to-one indexed value, name=value (repeatable).
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        /// One-to-many indexed value, name=value (repeatable).
        #[arg(long = "o2m", value_parser = parse_kv)]
        o2m: Vec<(String, String)>,
    },

    /// Show a branch as YAML.
    Show { path: String },

    /// List the direct children of a path.
    Ls { path: String },

    /// Print the branch tree under a path.
    Tree { path: String },

    /// Find branches by scope and hard o2o/o2m filters.
    Query {
        /// Restrict to branches under this path scope.
        #[arg(long)]
        under: Option<String>,
        /// Base the query on a frame slot, name.slot (e.g. story.place).
        #[arg(long)]
        frame: Option<String>,
        /// Required one-to-one value, name=value (repeatable).
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        /// Excluded one-to-one value, name=value (repeatable).
        #[arg(long = "avoid-o2o", value_parser = parse_kv)]
        avoid_o2o: Vec<(String, String)>,
        /// Required one-to-many value, name=value (repeatable).
        #[arg(long = "require-o2m", value_parser = parse_kv)]
        require_o2m: Vec<(String, String)>,
        /// Excluded one-to-many value, name=value (repeatable).
        #[arg(long = "avoid-o2m", value_parser = parse_kv)]
        avoid_o2m: Vec<(String, String)>,
    },

    /// List the o2o catalog, a single name's values, or branches for name=value.
    #[command(name = "o2o")]
    O2o {
        /// `name` (values for a name) or `name=value` (branches with it).
        selector: Option<String>,
    },

    /// List the o2m catalog, a single name's values, or branches for name=value.
    #[command(name = "o2m")]
    O2m {
        /// `name` (values for a name) or `name=value` (branches with it).
        selector: Option<String>,
    },

    /// Import frame definitions from a YAML file.
    ImportFrames { file: PathBuf },

    /// List stored frames.
    Frames,

    /// Show a frame (`name`) or one of its slots (`name.slot`) as YAML.
    Frame { selector: String },

    /// Create a skeleton branch from a frame slot (name.slot).
    Create {
        path: String,
        #[arg(long)]
        frame: String,
    },

    /// Validate a branch (name.slot) or a packet file (frame name) against a frame.
    Validate {
        /// Branch path or packet file, depending on the frame selector.
        target: String,
        #[arg(long)]
        frame: String,
    },

    /// Sample branches into a selection packet.
    Sample {
        #[arg(long)]
        under: Option<String>,
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        #[arg(long = "avoid-o2o", value_parser = parse_kv)]
        avoid_o2o: Vec<(String, String)>,
        #[arg(long = "require-o2m", value_parser = parse_kv)]
        require_o2m: Vec<(String, String)>,
        #[arg(long = "prefer-o2m", value_parser = parse_kv)]
        prefer_o2m: Vec<(String, String)>,
        #[arg(long = "avoid-o2m", value_parser = parse_kv)]
        avoid_o2m: Vec<(String, String)>,
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long)]
        weighted: bool,
        #[arg(long = "avoid-recent")]
        avoid_recent: bool,
        #[arg(long)]
        seed: Option<u64>,
        #[arg(long, default_value = "yaml", value_parser = parse_format)]
        format: Format,
        /// Print the packet without recording usage or saving it.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Compose a packet (one selection group per frame slot).
    Compose {
        #[arg(long)]
        frame: String,
        /// Scope override: `slot:/path` or `/path` (global). Repeatable.
        #[arg(long = "under", value_parser = parse_under_override)]
        under: Vec<UnderOverride>,
        /// Number of packets to generate (batch).
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long)]
        seed: Option<u64>,
        #[arg(long, default_value = "yaml", value_parser = parse_format)]
        format: Format,
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Show usage history for a branch.
    Used { path: String },

    /// Aggregate usage stats over a filter.
    Stats {
        #[arg(long)]
        under: Option<String>,
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        #[arg(long = "avoid-o2o", value_parser = parse_kv)]
        avoid_o2o: Vec<(String, String)>,
        #[arg(long = "require-o2m", value_parser = parse_kv)]
        require_o2m: Vec<(String, String)>,
        #[arg(long = "avoid-o2m", value_parser = parse_kv)]
        avoid_o2m: Vec<(String, String)>,
    },

    /// List branches matching a filter, least-used first.
    Stale {
        #[arg(long)]
        under: Option<String>,
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        #[arg(long = "avoid-o2o", value_parser = parse_kv)]
        avoid_o2o: Vec<(String, String)>,
        #[arg(long = "require-o2m", value_parser = parse_kv)]
        require_o2m: Vec<(String, String)>,
        #[arg(long = "avoid-o2m", value_parser = parse_kv)]
        avoid_o2m: Vec<(String, String)>,
    },

    /// Import branches from a YAML/JSON/JSONL file (upsert by path).
    Import { file: PathBuf },

    /// Export branches matching a filter.
    Export {
        #[arg(long)]
        under: Option<String>,
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Vec<(String, String)>,
        #[arg(long = "avoid-o2o", value_parser = parse_kv)]
        avoid_o2o: Vec<(String, String)>,
        #[arg(long = "require-o2m", value_parser = parse_kv)]
        require_o2m: Vec<(String, String)>,
        #[arg(long = "avoid-o2m", value_parser = parse_kv)]
        avoid_o2m: Vec<(String, String)>,
        #[arg(long, default_value = "yaml", value_parser = parse_format)]
        format: Format,
    },

    /// Attach a reference to a branch.
    Attach {
        path: String,
        value: String,
        /// Reference type (default: inferred — url for http(s), else file).
        #[arg(long = "type")]
        kind: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },

    /// List a branch's references.
    Refs { path: String },

    /// Scan a text file for structured patterns from the archive.
    Scan { file: PathBuf },

    /// Suggest o2m values mentioned in a branch's text but not yet set.
    #[command(name = "suggest-o2m")]
    SuggestO2m { path: String },

    /// Manage the pattern dictionary.
    Pattern {
        #[command(subcommand)]
        action: PatternAction,
    },

    /// List stored pattern entries.
    Patterns,
}

#[derive(Subcommand)]
enum PatternAction {
    /// Add a pattern mapping (provide exactly one target).
    Add {
        /// The text pattern to match.
        pattern: String,
        #[arg(long = "o2m", value_parser = parse_kv)]
        o2m: Option<(String, String)>,
        #[arg(long = "o2o", value_parser = parse_kv)]
        o2o: Option<(String, String)>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        collapse: Option<String>,
    },

    /// Remove a pattern entry by id.
    Remove { id: i64 },

    /// Enable a previously disabled pattern entry.
    Enable { id: i64 },

    /// Disable a pattern entry without removing it.
    Disable { id: i64 },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Only `init` may create the database; other commands require it to exist.
    let create = matches!(cli.command, Command::Init);
    let repos = db::open_repos(&cli.db, create).await?;

    match cli.command {
        Command::Init => {
            // open_repos already ran migrations; just confirm.
            println!("initialized {}", cli.db.display());
        }
        Command::Add {
            path,
            title,
            description,
            weight,
            o2o,
            o2m,
        } => {
            commands::add(
                &repos.branches,
                AddInput {
                    path,
                    title,
                    description,
                    weight,
                    o2o,
                    o2m,
                },
            )
            .await?;
        }
        Command::Show { path } => commands::show(&repos.branches, &path).await?,
        Command::Ls { path } => commands::ls(&repos.branches, &path).await?,
        Command::Tree { path } => commands::tree(&repos.branches, &path).await?,
        Command::Query {
            under,
            frame,
            o2o,
            avoid_o2o,
            require_o2m,
            avoid_o2m,
        } => {
            let frame_slot = resolve_query_slot(&repos.frames, frame.as_deref()).await?;
            commands::query(
                &repos.branches,
                QueryInput {
                    under,
                    frame_slot,
                    require_o2o: o2o,
                    avoid_o2o,
                    require_o2m,
                    avoid_o2m,
                },
            )
            .await?;
        }
        Command::O2o { selector } => commands::o2o(&repos.branches, selector).await?,
        Command::O2m { selector } => commands::o2m(&repos.branches, selector).await?,
        Command::ImportFrames { file } => {
            let frames = unclip_io::load_frames(&file)?;
            commands::import_frames(&repos.frames, frames).await?;
        }
        Command::Frames => commands::frames_list(&repos.frames).await?,
        Command::Frame { selector } => commands::frame_show(&repos.frames, &selector).await?,
        Command::Create { path, frame } => {
            commands::create(&repos.branches, &repos.frames, path, &frame).await?;
        }
        Command::Validate { target, frame } => {
            commands::validate(&repos.branches, &repos.frames, &target, &frame).await?;
        }
        Command::Sample {
            under,
            o2o,
            avoid_o2o,
            require_o2m,
            prefer_o2m,
            avoid_o2m,
            count,
            weighted,
            avoid_recent,
            seed,
            format,
            dry_run,
        } => {
            sampling::sample_cmd(
                &repos.branches,
                &repos.history,
                SampleInput {
                    filter: FilterInput {
                        under,
                        require_o2o: o2o,
                        avoid_o2o,
                        require_o2m,
                        prefer_o2m,
                        avoid_o2m,
                    },
                    count,
                    weighted,
                    avoid_recent,
                    seed,
                    format,
                    dry_run,
                },
            )
            .await?;
        }
        Command::Compose {
            frame,
            under,
            count,
            seed,
            format,
            dry_run,
        } => {
            sampling::compose_cmd(
                &repos.branches,
                &repos.frames,
                &repos.history,
                ComposeInput {
                    frame,
                    under,
                    count,
                    seed,
                    format,
                    dry_run,
                },
            )
            .await?;
        }
        Command::Used { path } => {
            sampling::used_cmd(&repos.branches, &repos.history, &path).await?;
        }
        Command::Stats {
            under,
            o2o,
            avoid_o2o,
            require_o2m,
            avoid_o2m,
        } => {
            sampling::stats_cmd(
                &repos.branches,
                &repos.history,
                FilterInput {
                    under,
                    require_o2o: o2o,
                    avoid_o2o,
                    require_o2m,
                    prefer_o2m: Vec::new(),
                    avoid_o2m,
                },
            )
            .await?;
        }
        Command::Stale {
            under,
            o2o,
            avoid_o2o,
            require_o2m,
            avoid_o2m,
        } => {
            sampling::stale_cmd(
                &repos.branches,
                &repos.history,
                FilterInput {
                    under,
                    require_o2o: o2o,
                    avoid_o2o,
                    require_o2m,
                    prefer_o2m: Vec::new(),
                    avoid_o2m,
                },
            )
            .await?;
        }
        Command::Import { file } => {
            let branches = unclip_io::load_branches_file(&file)?;
            commands::import(&repos.branches, branches).await?;
        }
        Command::Export {
            under,
            o2o,
            avoid_o2o,
            require_o2m,
            avoid_o2m,
            format,
        } => {
            sampling::export_cmd(
                &repos.branches,
                FilterInput {
                    under,
                    require_o2o: o2o,
                    avoid_o2o,
                    require_o2m,
                    prefer_o2m: Vec::new(),
                    avoid_o2m,
                },
                format,
            )
            .await?;
        }
        Command::Attach {
            path,
            value,
            kind,
            note,
        } => {
            commands::attach(&repos.branches, &path, value, kind, note).await?;
        }
        Command::Refs { path } => commands::refs(&repos.branches, &path).await?,
        Command::Scan { file } => {
            matching::scan_cmd(&repos.branches, &repos.patterns, &file).await?;
        }
        Command::SuggestO2m { path } => {
            matching::suggest_o2m_cmd(&repos.branches, &repos.patterns, &path).await?;
        }
        Command::Pattern { action } => match action {
            PatternAction::Add {
                pattern,
                o2m,
                o2o,
                branch,
                collapse,
            } => {
                matching::pattern_add_cmd(
                    &repos.patterns,
                    matching::PatternAddInput {
                        pattern,
                        o2m,
                        o2o,
                        branch,
                        collapse,
                    },
                )
                .await?;
            }
            PatternAction::Remove { id } => {
                matching::pattern_remove_cmd(&repos.patterns, id).await?;
            }
            PatternAction::Enable { id } => {
                matching::pattern_set_enabled_cmd(&repos.patterns, id, true).await?;
            }
            PatternAction::Disable { id } => {
                matching::pattern_set_enabled_cmd(&repos.patterns, id, false).await?;
            }
        },
        Command::Patterns => matching::patterns_cmd(&repos.patterns).await?,
    }

    Ok(())
}

/// Resolve a `--frame name.slot` selector for `query` into a slot, if given.
async fn resolve_query_slot(
    frames: &impl FrameRepository,
    selector: Option<&str>,
) -> anyhow::Result<Option<unclip_core::Slot>> {
    let Some(selector) = selector else {
        return Ok(None);
    };
    let (frame_name, slot_name) = split_frame_selector(selector);
    let slot_name = slot_name.context("query --frame requires name.slot, e.g. story.place")?;
    let frame = frames
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;
    let slot = frame
        .slot(slot_name)
        .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
    Ok(Some(slot.clone()))
}
