//! unclip — outside-of-LLM possibility engine (CLI entry point).

mod commands;
mod db;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use unclip_io::split_frame_selector;
use unclip_store::FrameRepository;

use commands::{parse_kv, AddInput, QueryInput};

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
    Show {
        path: String,
    },

    /// List the direct children of a path.
    Ls {
        path: String,
    },

    /// Print the branch tree under a path.
    Tree {
        path: String,
    },

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
    ImportFrames {
        file: PathBuf,
    },

    /// List stored frames.
    Frames,

    /// Show a frame (`name`) or one of its slots (`name.slot`) as YAML.
    Frame {
        selector: String,
    },

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let repos = db::open_repos(&cli.db).await?;

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
