//! Frame definitions and the commands built on them: `import-frames`,
//! `frames`, `frame`, `rm-frame`, `create`, `validate`.

use anyhow::{bail, Context};
use unclip_core::{validate_branch, validate_packet, validate_path, Frame, SelectionPacket};
use unclip_io::split_frame_selector;
use unclip_store::{BranchReader, BranchWriter, FrameRepository};

/// Import frames parsed from a frames file.
pub async fn import_frames(repo: &impl FrameRepository, frames: Vec<Frame>) -> anyhow::Result<()> {
    if frames.is_empty() {
        crate::output::errln!("(no frames in file)");
        return Ok(());
    }
    // Capture summaries before the batch consumes the frames, so the per-frame
    // output can be printed only after the whole import commits atomically.
    let summaries: Vec<(String, usize)> = frames
        .iter()
        .map(|frame| (frame.name.clone(), frame.slots.len()))
        .collect();
    repo.save_frames(frames).await?;
    for (name, slots) in summaries {
        crate::output::outln!("imported frame {name} ({slots} slot(s))");
    }
    Ok(())
}

/// `unclip frames` — list stored frames.
pub async fn frames_list(repo: &impl FrameRepository) -> anyhow::Result<()> {
    let frames = repo.list_frames().await?;
    if frames.is_empty() {
        crate::output::errln!("(no frames)");
        return Ok(());
    }
    for info in frames {
        match &info.description {
            Some(desc) => {
                crate::output::outln!("{}\t{} slot(s)\t{}", info.name, info.slot_count, desc)
            }
            None => crate::output::outln!("{}\t{} slot(s)", info.name, info.slot_count),
        }
    }
    Ok(())
}

/// `unclip frame <name>` or `unclip frame <name>.<slot>` — show as YAML.
pub async fn frame_show(repo: &impl FrameRepository, selector: &str) -> anyhow::Result<()> {
    let (frame_name, slot_name) = split_frame_selector(selector);
    let frame = repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;
    match slot_name {
        None => crate::output::out!("{}", serde_norway::to_string(&frame)?),
        Some(slot_name) => {
            let slot = frame
                .slot(slot_name)
                .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
            crate::output::out!("{}", serde_norway::to_string(slot)?);
        }
    }
    Ok(())
}

/// `unclip rm-frame <name>` — delete a frame and its slots.
pub async fn rm_frame(repo: &impl FrameRepository, name: &str) -> anyhow::Result<()> {
    if repo.get_frame(name).await?.is_none() {
        bail!("frame not found: {name}");
    }
    repo.delete_frame(name).await?;
    crate::output::outln!("deleted frame {name}");
    Ok(())
}

/// `unclip create <path> --frame <name.slot>` — create a skeleton branch.
pub async fn create(
    branch_repo: &impl BranchWriter,
    frame_repo: &impl FrameRepository,
    path: String,
    selector: &str,
) -> anyhow::Result<()> {
    validate_path(&path)?;
    let (frame_name, slot_name) = split_frame_selector(selector);
    let Some(slot_name) = slot_name else {
        bail!("create requires a frame.slot selector, e.g. story.place");
    };
    let frame = frame_repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;
    let slot = frame
        .slot(slot_name)
        .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;

    // A duplicate path is reported atomically by the repository insert.
    let branch = slot.skeleton(&path);
    branch_repo.add(&branch).await?;
    crate::output::outln!("created {path} from {selector}");
    crate::output::out!("{}", serde_norway::to_string(&branch)?);
    Ok(())
}

/// `unclip validate <target> --frame <selector>`.
///
/// `name.slot` validates a stored branch (by path); a frame-only selector
/// validates a packet file (by path on disk).
pub async fn validate(
    branch_repo: &impl BranchReader,
    frame_repo: &impl FrameRepository,
    target: &str,
    selector: &str,
) -> anyhow::Result<()> {
    let (frame_name, slot_name) = split_frame_selector(selector);
    let frame = frame_repo
        .get_frame(frame_name)
        .await?
        .with_context(|| format!("frame not found: {frame_name}"))?;

    let violations = match slot_name {
        Some(slot_name) => {
            let slot = frame
                .slot(slot_name)
                .with_context(|| format!("frame `{frame_name}` has no slot `{slot_name}`"))?;
            let branch = branch_repo
                .get(target)
                .await?
                .with_context(|| format!("branch not found: {target}"))?;
            validate_branch(slot, &branch)
        }
        None => {
            let text = unclip_io::read_text_file(std::path::Path::new(target), "packet file")?;
            let packet: SelectionPacket = serde_norway::from_str(&text)?;
            validate_packet(&frame, &packet)
        }
    };

    if violations.is_empty() {
        crate::output::outln!("OK: {target} satisfies {selector}");
        Ok(())
    } else {
        for reason in &violations {
            crate::output::errln!("- {reason}");
        }
        bail!("{} violation(s)", violations.len());
    }
}
