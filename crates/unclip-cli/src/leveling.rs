//! Commands for the semantic leveling engine.

pub(crate) fn plugins() -> anyhow::Result<()> {
    let registry = unclip_engine::builtin_registry()?;
    let mut found = false;

    for plugin in registry.inferrers() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tinferred\t{}\t-\tinference_output",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tcalculated\t{}\t{:?}/{:?}\t{:?}",
            descriptor.id,
            descriptor.version,
            descriptor.applicability,
            descriptor.evidence,
            descriptor.produces
        );
    }
    for plugin in registry.comparators() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tcalculated\t{}\t{:?}\tdelta",
            descriptor.id,
            descriptor.version,
            descriptor.supports
        );
    }

    if !found {
        crate::output::outln!("no leveling plugins registered");
    }
    Ok(())
}

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

fn parse_domain_selector(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_selector_requires_both_immutable_parts() {
        let (domain, version) = parse_domain_selector("coffee@7").unwrap();
        assert_eq!(domain.0, "coffee");
        assert_eq!(version.0, "7");
        assert!(parse_domain_selector("coffee").is_err());
        assert!(parse_domain_selector("@7").is_err());
        assert!(parse_domain_selector("coffee@").is_err());
    }
}
