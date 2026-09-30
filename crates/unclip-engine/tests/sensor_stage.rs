//! Pipeline stage comes from the sensor's descriptor, not from its plugin id.
//!
//! The engine used to infer stage by matching the plugin id against the two
//! built-in names `sensor.coverage` and `sensor.residual`. That made the
//! explanation and residual stages unreachable for any sensor outside this
//! workspace, and it silently reclassified a built-in sensor if it were ever
//! renamed. These tests pin the replacement: a sensor declares its own stage,
//! and the engine honors that declaration.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use unclip_domain::{DomainId, DomainSnapshot, FrameId, MeasurementFrame};
use unclip_engine::{Engine, MeasurementInputs, MeasurementRun};
use unclip_epistemic::{
    Calculated, CalculationToken, DerivedId, DomainVersion, EmitMetadata, FrameVersion, PluginId,
    SourceRef, Timestamp,
};
use unclip_measure::{Measurement, MeasurementContext, MeasurementValue, Reading};
use unclip_plugin::{
    Applicability, EngineProfile, MeasureCtx, PluginSelection, Registry, Result, Sensor,
    SensorDescriptor, SensorStage,
};

/// Records the order in which the engine invoked each sensor.
type CallLog = Arc<Mutex<Vec<String>>>;

struct StageSensor {
    descriptor: SensorDescriptor,
    calls: CallLog,
}

impl StageSensor {
    fn registered(id: &str, stage: SensorStage, calls: CallLog) -> Arc<dyn Sensor> {
        Arc::new(Self {
            descriptor: SensorDescriptor {
                id: PluginId::new(id),
                version: semver::Version::new(0, 1, 0),
                stage,
                applicability: &[],
                evidence: &[],
                produces: &[unclip_measure::MeasurementKind::Scalar],
                params_schema: "{}",
            },
            calls,
        })
    }
}

impl Sensor for StageSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, _ctx: &MeasureCtx<'_>) -> Applicability {
        Applicability::Applicable
    }

    fn measure(
        &self,
        _ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        self.calls
            .lock()
            .unwrap()
            .push(self.descriptor.id.0.clone());
        Ok(vec![token.emit(Measurement {
            sensor: self.descriptor.id.clone(),
            sensor_version: self.descriptor.version.clone(),
            reading: Reading::Value {
                value: MeasurementValue::Scalar(1.0),
            },
            confidence: None,
            sample_count: None,
            context: MeasurementContext::default(),
        })])
    }
}

fn domain() -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new("stage"),
        version: DomainVersion::new("1"),
        units: BTreeMap::new(),
        relations: BTreeMap::new(),
    }
}

fn frame() -> MeasurementFrame {
    MeasurementFrame {
        id: FrameId::new("stage.frame"),
        version: FrameVersion::new("1"),
        axes: Vec::new(),
    }
}

fn run_metadata() -> EmitMetadata {
    let params = serde_json::json!({});
    EmitMetadata::new(
        DerivedId::new("stage-run"),
        PluginId::new("test"),
        semver::Version::new(0, 1, 0),
        &params,
        Timestamp::new("2026-09-19T00:00:00Z"),
    )
    .with_source(SourceRef::new("fixture"))
    .with_domain_version(DomainVersion::new("1"))
    .with_frame_version(FrameVersion::new("1"))
}

/// Run two sensors whose ids sort opposite to their declared stages.
///
/// `sensor.zzz` declares `Explanation` and `sensor.aaa` declares `Measurement`.
/// Sorting by id alone would run `aaa` first; sorting by declared stage must
/// run `zzz` first.
fn run_ordered_pair(explanation_stage: SensorStage) -> Vec<String> {
    let calls: CallLog = Arc::default();
    let mut registry = Registry::default();
    registry
        .register_sensor(StageSensor::registered(
            "sensor.zzz",
            explanation_stage,
            calls.clone(),
        ))
        .unwrap();
    registry
        .register_sensor(StageSensor::registered(
            "sensor.aaa",
            SensorStage::Measurement,
            calls.clone(),
        ))
        .unwrap();

    let engine = Engine::new(registry);
    let profile = EngineProfile {
        sensors: vec![
            PluginSelection::any("sensor.zzz"),
            PluginSelection::any("sensor.aaa"),
        ],
        inferrers: Vec::new(),
        comparators: Vec::new(),
        interpreters: Vec::new(),
        candidate_generators: Vec::new(),
        null_models: Vec::new(),
    };
    let plan = engine.plan(&profile).unwrap();

    let domain = domain();
    let frame = frame();
    let params = BTreeMap::new();
    engine
        .measure(
            &plan,
            MeasurementInputs {
                domain: &domain,
                frame: &frame,
                observations: &[],
                alignments: &[],
                rankings: &[],
            },
            MeasurementRun {
                id: &run_metadata().id.0,
                timestamp: Timestamp::new("2026-09-19T00:00:00Z"),
                params: &params,
            },
        )
        .unwrap();

    let order = calls.lock().unwrap().clone();
    order
}

#[test]
fn declared_stage_orders_sensors_ahead_of_plugin_id() {
    assert_eq!(
        run_ordered_pair(SensorStage::Explanation),
        vec!["sensor.zzz".to_string(), "sensor.aaa".to_string()],
        "an Explanation-stage sensor must run before a Measurement-stage one, \
         even though its id sorts later"
    );
}

#[test]
fn equal_stages_fall_back_to_stable_plugin_id_order() {
    assert_eq!(
        run_ordered_pair(SensorStage::Measurement),
        vec!["sensor.aaa".to_string(), "sensor.zzz".to_string()],
        "sensors sharing a stage keep deterministic id order"
    );
}

/// A sensor that declares nothing special is a measurement sensor.
#[test]
fn measurement_is_the_default_stage() {
    assert_eq!(SensorStage::default(), SensorStage::Measurement);
    assert!(SensorStage::Explanation < SensorStage::Residual);
    assert!(SensorStage::Residual < SensorStage::Measurement);
}

/// Every builtin descriptor's declared schema must be able to constrain something.
///
/// `unclip-plugin` now checks parameters against this string before invoking a
/// sensor, so a schema that is malformed, or that omits `additionalProperties`
/// and therefore silently permits any key, is a real gap rather than a cosmetic
/// one. This walks the whole registry so a newly registered plugin cannot skip it.
#[test]
fn every_builtin_plugin_declares_a_usable_params_schema() {
    let registry = unclip_engine::builtin_registry().expect("builtin registry");
    let mut checked = 0;
    let mut schemas: Vec<(String, &'static str)> = Vec::new();
    for plugin in registry.sensors() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.product_sensors() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.cross_product_sensors() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.inferrers() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.comparators() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.interpreters() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.candidate_generators() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for plugin in registry.null_models() {
        let d = plugin.descriptor();
        schemas.push((d.id.0.clone(), d.params_schema));
    }
    for (id, schema) in schemas {
        unclip_plugin::check_schema(schema)
            .unwrap_or_else(|error| panic!("{id} declares an unusable params schema: {error}"));
        checked += 1;
    }
    assert!(
        checked > 30,
        "expected the whole registry, checked {checked}"
    );
}
