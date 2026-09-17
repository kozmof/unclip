//! Commands for the semantic leveling engine.

use unclip_plugin::Registry;

pub(crate) fn plugins() -> anyhow::Result<()> {
    let registry = Registry::with_builtins();
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
