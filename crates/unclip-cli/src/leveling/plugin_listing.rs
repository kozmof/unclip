//! `unclip level plugins` — list every registered leveling plugin.

//! Each line reports the plugin's id, the epistemic operation it produces,
//! its version, and — for sensors — the inputs and evidence it requires.

pub(crate) fn plugins() -> anyhow::Result<()> {
    let registry = unclip_engine::builtin_registry()?;
    let mut found = false;

    for plugin in registry.inferrers() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tINFERRED\t{}\t-\tinference_output",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t{:?}/{:?}\t{:?}",
            descriptor.id,
            descriptor.version,
            descriptor.applicability,
            descriptor.evidence,
            descriptor.produces
        );
    }
    for plugin in registry.product_sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t{:?}/{:?}\t{:?}",
            descriptor.id,
            descriptor.version,
            descriptor.applicability,
            descriptor.evidence,
            descriptor.produces
        );
    }
    for plugin in registry.cross_product_sensors() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t{:?}/{:?}\t{:?}",
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
            "{}\tCALCULATED\t{}\t{:?}\tdelta",
            descriptor.id,
            descriptor.version,
            descriptor.supports
        );
    }
    for plugin in registry.interpreters() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tINTERPRETED\t{}\t-\tinterpretation",
            descriptor.id,
            descriptor.version
        );
    }

    for plugin in registry.candidate_generators() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t-\tcandidate",
            descriptor.id,
            descriptor.version
        );
    }
    for plugin in registry.null_models() {
        found = true;
        let descriptor = plugin.descriptor();
        crate::output::outln!(
            "{}\tCALCULATED\t{}\t-\tnull_reading",
            descriptor.id,
            descriptor.version
        );
    }

    if !found {
        crate::output::outln!("no leveling plugins registered");
    }
    Ok(())
}
