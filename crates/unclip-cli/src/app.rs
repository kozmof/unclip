//! Command dispatch for the unclip CLI.

use anyhow::Context;
use clap::Parser;
use unclip_io::split_frame_selector;
use unclip_store::FrameRepository;

use crate::cli::{Cli, Command, PatternAction};
use crate::{commands, db, matching, sampling};

use commands::QueryInput;
use sampling::{ComposeInput, FilterInput, SampleInput};

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Only `init` may create the database; other commands require it to exist.
    let create = matches!(cli.command, Command::Init);
    let repos = db::open_repos(&cli.db, create).await?;

    match cli.command {
        Command::Init => {
            // open_repos already ran migrations; just confirm.
            crate::output::outln!("initialized {}", cli.db.display());
        }
        Command::Add(input) => commands::add(&repos.branches, input).await?,
        Command::Edit(input) => commands::edit(&repos.branches, input).await?,
        Command::Show { path } => commands::show(&repos.branches, &path).await?,
        Command::Ls { path } => commands::ls(&repos.branches, &path).await?,
        Command::Tree { path } => commands::tree(&repos.branches, &path).await?,
        Command::Query { filter, frame } => {
            let frame_slot = resolve_query_slot(&repos.frames, frame.as_deref()).await?;
            commands::query(
                &repos.branches,
                QueryInput {
                    under: filter.under,
                    frame_slot,
                    require_o2o: filter.o2o,
                    avoid_o2o: filter.avoid_o2o,
                    require_o2m: filter.require_o2m,
                    avoid_o2m: filter.avoid_o2m,
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
            filter,
            prefer_o2m,
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
                    filter: FilterInput::from_args(filter, prefer_o2m),
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
        Command::Stats { filter } => {
            sampling::stats_cmd(
                &repos.branches,
                &repos.history,
                FilterInput::from_args(filter, Vec::new()),
            )
            .await?;
        }
        Command::Stale { filter } => {
            sampling::stale_cmd(
                &repos.branches,
                &repos.history,
                FilterInput::from_args(filter, Vec::new()),
            )
            .await?;
        }
        Command::Import { file } => {
            let branches = unclip_io::load_branches_file(&file)?;
            commands::import(&repos.branches, branches).await?;
        }
        Command::Export { filter, format } => {
            sampling::export_cmd(
                &repos.branches,
                FilterInput::from_args(filter, Vec::new()),
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
