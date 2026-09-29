//! Domain and measurement-frame catalog commands.
//!
//! Import and inspect the versioned domain snapshots and measurement frames
//! that the leveling stages calculate against.

pub(crate) async fn domain_import(
    repository: &impl unclip_store::DomainWriter,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let snapshot = unclip_io::load_domain(file)?;
    let selector = format!("{}@{}", snapshot.id.0, snapshot.version.0);
    let units = snapshot.units.len();
    let relations = snapshot.relations.len();
    repository.insert_domain_version(snapshot).await?;
    crate::output::outln!("imported domain {selector} ({units} unit(s), {relations} relation(s))");
    Ok(())
}

pub(crate) async fn domain_show(
    repository: &impl unclip_store::DomainReader,
    selector: &str,
    format: unclip_io::Format,
) -> anyhow::Result<()> {
    let (domain, version) = parse_domain_selector(selector)?;
    let snapshot = repository
        .get_domain_version(&domain, &version)
        .await?
        .ok_or_else(|| anyhow::anyhow!("domain version not found: {selector}"))?;
    crate::output::write_stdout(&unclip_io::render_domain(&snapshot, format)?)
}

pub(crate) fn parse_domain_selector(
    selector: &str,
) -> anyhow::Result<(unclip_domain::DomainId, unclip_epistemic::DomainVersion)> {
    let (domain, version) = selector
        .rsplit_once('@')
        .filter(|(domain, version)| !domain.is_empty() && !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("domain selector must be domain@version"))?;
    Ok((
        unclip_domain::DomainId::new(domain),
        unclip_epistemic::DomainVersion::new(version),
    ))
}

pub(crate) async fn frame_import(
    repository: &impl unclip_store::DomainWriter,
    file: &std::path::Path,
) -> anyhow::Result<()> {
    let document = unclip_io::load_measurement_frame(file)?;
    let selector = format!("{}@{}", document.frame.id.0, document.frame.version.0);
    let axes = document.frame.axes.len();
    repository
        .insert_measurement_frame(
            &document.domain_id,
            &document.domain_version,
            document.frame,
        )
        .await?;
    crate::output::outln!("imported measurement frame {selector} ({axes} axis/axes)");
    Ok(())
}

pub(crate) async fn frame_show(
    repository: &impl unclip_store::DomainReader,
    selector: &str,
    format: unclip_io::Format,
) -> anyhow::Result<()> {
    let (frame, version) = parse_frame_selector(selector)?;
    let snapshot = repository
        .get_measurement_frame(&frame, &version)
        .await?
        .ok_or_else(|| anyhow::anyhow!("measurement frame version not found: {selector}"))?;
    let rendered = match format {
        unclip_io::Format::Yaml => serde_norway::to_string(&snapshot)?,
        unclip_io::Format::Json => format!("{}\n", serde_json::to_string_pretty(&snapshot)?),
        unclip_io::Format::Jsonl => {
            anyhow::bail!("JSONL is not supported for measurement frames")
        }
    };
    crate::output::write_stdout(&rendered)
}

pub(crate) fn parse_frame_selector(
    selector: &str,
) -> anyhow::Result<(unclip_domain::FrameId, unclip_epistemic::FrameVersion)> {
    let (frame, version) = selector
        .rsplit_once('@')
        .filter(|(frame, version)| !frame.is_empty() && !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("frame selector must be frame@version"))?;
    Ok((
        unclip_domain::FrameId::new(frame),
        unclip_epistemic::FrameVersion::new(version),
    ))
}
