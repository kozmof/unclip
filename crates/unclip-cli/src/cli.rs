//! Command-line argument schema.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use unclip_io::Format;

use crate::commands::{parse_kv, AddInput, EditInput};
use crate::sampling::{parse_format, parse_under_override, UnderOverride};

#[derive(Parser)]
#[command(name = "unclip", version, about = "Outside-of-LLM possibility engine")]
pub(crate) struct Cli {
    /// Path to the SQLite database file.
    #[arg(long, global = true, default_value = "unclip.db")]
    pub(crate) db: PathBuf,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Scope and hard o2o/o2m filter flags shared by every filtering command.
///
/// `prefer_o2m` is deliberately not here: it is a scoring signal that only
/// `sample` consumes, so filter-only commands must not advertise the flag.
#[derive(Args)]
pub(crate) struct FilterArgs {
    /// Restrict to branches under this path scope.
    #[arg(long)]
    pub(crate) under: Option<String>,
    /// Required one-to-one value, name=value (repeatable).
    #[arg(long = "o2o", value_parser = parse_kv)]
    pub(crate) o2o: Vec<(String, String)>,
    /// Excluded one-to-one value, name=value (repeatable).
    #[arg(long = "avoid-o2o", value_parser = parse_kv)]
    pub(crate) avoid_o2o: Vec<(String, String)>,
    /// Required one-to-many value, name=value (repeatable).
    #[arg(long = "require-o2m", value_parser = parse_kv)]
    pub(crate) require_o2m: Vec<(String, String)>,
    /// Excluded one-to-many value, name=value (repeatable).
    #[arg(long = "avoid-o2m", value_parser = parse_kv)]
    pub(crate) avoid_o2m: Vec<(String, String)>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create and migrate the database.
    Init,

    /// Add a new branch.
    Add(AddInput),

    /// Edit fields, o2o, and o2m on an existing branch.
    Edit(EditInput),

    /// Show a branch as YAML.
    Show { path: String },

    /// List the direct children of a path.
    Ls { path: String },

    /// Print the branch tree under a path.
    Tree { path: String },

    /// Find branches by scope and hard o2o/o2m filters.
    Query {
        #[command(flatten)]
        filter: FilterArgs,
        /// Base the query on a frame slot, name.slot (e.g. story.place).
        #[arg(long)]
        frame: Option<String>,
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
        #[command(flatten)]
        filter: FilterArgs,
        /// Preferred one-to-many value (raises score), name=value (repeatable).
        #[arg(long = "prefer-o2m", value_parser = parse_kv)]
        prefer_o2m: Vec<(String, String)>,
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
        #[command(flatten)]
        filter: FilterArgs,
    },

    /// List branches matching a filter, least-used first.
    Stale {
        #[command(flatten)]
        filter: FilterArgs,
    },

    /// Import branches from a YAML/JSON/JSONL file (upsert by path).
    Import { file: PathBuf },

    /// Export branches matching a filter.
    Export {
        #[command(flatten)]
        filter: FilterArgs,
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
pub(crate) enum PatternAction {
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
