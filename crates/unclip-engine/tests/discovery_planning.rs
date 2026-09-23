use std::{collections::BTreeMap, sync::Arc};
use unclip_engine::Engine;
use unclip_epistemic::{hash_params, PluginId, Timestamp};
use unclip_measure::Reading;
use unclip_plugin::{
    CandidateGenerator, EngineProfile, Interpreter, NullModel, PluginDescriptor, PluginError,
    PluginSelection, Registry,
};

struct Stub(PluginDescriptor);
impl CandidateGenerator for Stub {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.0
    }
    fn generate(
        &self,
        _: &unclip_plugin::CandidateCtx<'_>,
        _: unclip_epistemic::CalculationToken,
    ) -> unclip_plugin::Result<Vec<unclip_epistemic::Calculated<unclip_domain::CandidateProposal>>>
    {
        panic!("planning must not execute a generator")
    }
}
impl NullModel for Stub {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.0
    }
    fn evaluate(
        &self,
        _: &unclip_plugin::NullCtx<'_>,
        _: unclip_epistemic::CalculationToken,
    ) -> unclip_plugin::Result<unclip_epistemic::Calculated<Reading>> {
        panic!("planning must not execute a null model")
    }
}
#[async_trait::async_trait]
impl Interpreter for Stub {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.0
    }
    async fn interpret(
        &self,
        _: &unclip_measure::EmpiricalStructure,
        _: &unclip_plugin::Params,
        _: &dyn unclip_plugin::InterpretationIo,
        _: unclip_epistemic::InterpretationToken,
    ) -> unclip_plugin::Result<unclip_epistemic::Interpreted<serde_json::Value>> {
        panic!("planning must not execute an interpreter")
    }
}
fn stub(id: &str) -> Arc<Stub> {
    Arc::new(Stub(PluginDescriptor {
        id: PluginId::new(id),
        version: "1.2.3".parse().unwrap(),
        params_schema: "{}",
    }))
}
fn registry() -> Registry {
    let mut registry = Registry::default();
    for id in ["generate.z", "generate.a"] {
        registry.register_generator(stub(id)).unwrap();
    }
    registry.register_null_model(stub("null.fixture")).unwrap();
    registry
        .register_interpreter(stub("interpret.fixture"))
        .unwrap();
    registry
}
fn profile() -> EngineProfile {
    EngineProfile {
        candidate_generators: vec![
            PluginSelection::any("generate.z"),
            PluginSelection::any("generate.a"),
        ],
        null_models: vec![PluginSelection::any("null.fixture")],
        interpreters: vec![PluginSelection::any("interpret.fixture")],
        ..EngineProfile::default()
    }
}
#[test]
fn registry_selects_only_explicit_plugins_and_enforces_identity_and_version() {
    let mut registry = registry();
    let empty = registry.resolve(&EngineProfile::default()).unwrap();
    assert!(
        empty.candidate_generators.is_empty()
            && empty.null_models.is_empty()
            && empty.interpreters.is_empty()
    );
    assert_eq!(
        registry
            .candidate_generators()
            .map(|p| p.descriptor().id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["generate.a", "generate.z"]
    );
    assert_eq!(registry.null_models().count(), 1);
    assert_eq!(
        registry
            .interpreters()
            .map(|plugin| plugin.descriptor().id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["interpret.fixture"]
    );
    assert!(matches!(
        registry.register_generator(stub("generate.a")),
        Err(PluginError::DuplicatePlugin(_))
    ));
    assert!(matches!(
        registry.register_null_model(stub("null.fixture")),
        Err(PluginError::DuplicatePlugin(_))
    ));
    assert!(matches!(
        registry.register_interpreter(stub("interpret.fixture")),
        Err(PluginError::DuplicatePlugin(_))
    ));
    for stage in ["generator", "null", "interpreter"] {
        let mut selected = profile();
        let selections = match stage {
            "generator" => &mut selected.candidate_generators,
            "null" => &mut selected.null_models,
            _ => &mut selected.interpreters,
        };
        selections[0].version = "^2".parse().unwrap();
        assert!(matches!(
            registry.resolve(&selected),
            Err(PluginError::IncompatibleVersion { .. })
        ));
        let selections = match stage {
            "generator" => &mut selected.candidate_generators,
            "null" => &mut selected.null_models,
            _ => &mut selected.interpreters,
        };
        selections[0] = PluginSelection::any("absent");
        assert!(matches!(
            registry.resolve(&selected),
            Err(PluginError::MissingPlugin(_))
        ));
    }
    let mut selected = profile();
    selected
        .null_models
        .push(PluginSelection::any("generate.a"));
    assert!(matches!(
        registry.resolve(&selected),
        Err(PluginError::DuplicatePlugin(_))
    ));
    let mut selected = profile();
    selected
        .candidate_generators
        .push(PluginSelection::any("generate.a"));
    assert!(matches!(
        registry.resolve(&selected),
        Err(PluginError::DuplicatePlugin(_))
    ));
    let mut selected = profile();
    selected
        .interpreters
        .push(PluginSelection::any("generate.a"));
    assert!(matches!(
        registry.resolve(&selected),
        Err(PluginError::DuplicatePlugin(_))
    ));
}
#[test]
fn stored_plans_pin_discovery_versions_parameters_and_hashes_in_canonical_order() {
    let engine = Engine::new(registry());
    let selected = profile();
    let plan = engine.plan(&selected).unwrap();
    assert_eq!(plan.candidate_generators.len(), 2);
    assert_eq!(plan.null_models.len(), 1);
    assert_eq!(plan.interpreters.len(), 1);
    let params = BTreeMap::from([
        (
            PluginId::new("generate.a"),
            serde_json::json!({"minimum_samples":4}),
        ),
        (PluginId::new("null.fixture"), serde_json::json!({"seed":7})),
        (
            PluginId::new("interpret.fixture"),
            serde_json::json!({"temperature":0}),
        ),
    ]);
    let record = engine.run_record(
        &plan,
        &params,
        "discovery",
        Timestamp::new("now"),
        serde_json::json!({}),
    );
    assert_eq!(
        record.resolved_plan["candidate_generators"][0]["id"],
        "generate.a"
    );
    assert_eq!(
        record.resolved_plan["candidate_generators"][1]["params"],
        serde_json::json!({})
    );
    for section in ["candidate_generators", "null_models", "interpreters"] {
        for entry in record.resolved_plan[section].as_array().unwrap() {
            assert_eq!(entry["version"], "1.2.3");
            assert_eq!(
                entry["params_hash"],
                serde_json::to_value(hash_params(&entry["params"])).unwrap()
            );
        }
    }
    assert_eq!(record.resolved_plan["null_models"][0]["params"]["seed"], 7);
    assert_eq!(
        record.resolved_plan["interpreters"][0]["params"]["temperature"],
        0
    );
    let mut reverse = selected;
    reverse.candidate_generators.reverse();
    let replay = engine.run_record(
        &engine.plan(&reverse).unwrap(),
        &params,
        "discovery",
        Timestamp::new("now"),
        serde_json::json!({}),
    );
    assert_eq!(record, replay);
}
