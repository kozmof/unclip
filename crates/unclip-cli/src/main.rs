//! unclip — outside-of-LLM possibility engine (CLI entry point).

mod commands;
mod db;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let repo = db::open_repo(&cli.db).await?;

    match cli.command {
        Command::Init => {
            // open_repo already ran migrations; just confirm.
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
                &repo,
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
        Command::Show { path } => commands::show(&repo, &path).await?,
        Command::Ls { path } => commands::ls(&repo, &path).await?,
        Command::Tree { path } => commands::tree(&repo, &path).await?,
        Command::Query {
            under,
            o2o,
            avoid_o2o,
            avoid_o2m,
        } => {
            commands::query(
                &repo,
                QueryInput {
                    under,
                    require_o2o: o2o,
                    avoid_o2o,
                    avoid_o2m,
                },
            )
            .await?;
        }
        Command::O2o { selector } => commands::o2o(&repo, selector).await?,
        Command::O2m { selector } => commands::o2m(&repo, selector).await?,
    }

    Ok(())
}
