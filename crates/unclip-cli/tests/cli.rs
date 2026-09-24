//! End-to-end tests that drive the built `unclip` binary against a throwaway
//! SQLite file. These cover the CLI layer (flag parsing, query assembly, and
//! the validation edges) that the per-crate unit tests do not reach.
//!
//! No extra dev-dependencies: the binary is located via `CARGO_BIN_EXE_unclip`
//! (set by Cargo for integration tests) and each test uses its own temp DB.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A unique temp directory removed on drop.
struct TempDb {
    dir: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "unclip-cli-test-{}-{}-{}",
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("unclip.db")
    }

    /// Write `contents` to a file in the temp dir and return its path.
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::write(&p, contents).expect("write temp file");
        p
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run `unclip --db <db> <args...>` and capture the result.
fn unclip(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unclip"))
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("failed to spawn unclip")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn canonical_experiment_result(mut value: serde_json::Value, run_id: &str) -> serde_json::Value {
    fn replace_run_id(value: &mut serde_json::Value, run_id: &str) {
        match value {
            serde_json::Value::String(text) => {
                if let Some(suffix) = text.strip_prefix(run_id) {
                    *text = format!("RUN{suffix}");
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    replace_run_id(value, run_id);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values_mut() {
                    replace_run_id(value, run_id);
                }
            }
            _ => {}
        }
    }

    replace_run_id(&mut value, run_id);
    value
}

#[test]
fn query_rejects_an_invalid_scope() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());

    let out = unclip(&path, &["query", "--under", "relative"]);

    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("invalid --under scope `relative`"),
        "unexpected stderr: {}",
        stderr(&out)
    );
}

/// `init` then a basic add/show round-trip through the real binary.
#[test]
fn init_add_show_roundtrip() {
    let db = TempDb::new();
    let path = db.path();

    let init = unclip(&path, &["init"]);
    assert!(init.status.success(), "init failed: {}", stderr(&init));

    let add = unclip(
        &path,
        &[
            "add",
            "/ikebukuro/station/coin-locker",
            "--title",
            "Coin Locker Area",
            "--o2o",
            "axis=place",
            "--o2m",
            "mood=tense",
            "--o2m",
            "mood=hidden",
        ],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let show = unclip(&path, &["show", "/ikebukuro/station/coin-locker"]);
    assert!(show.status.success());
    let yaml = stdout(&show);
    assert!(yaml.contains("path: /ikebukuro/station/coin-locker"));
    assert!(yaml.contains("axis: place"));
    // o2m is a set returned in sorted order.
    assert!(yaml.contains("- hidden"));
    assert!(yaml.contains("- tense"));
}

/// `edit` patches an existing branch: set/overwrite o2o, add/remove o2m,
/// change scalar fields, and clear the title.
#[test]
fn edit_patches_existing_branch() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());

    let add = unclip(
        &path,
        &[
            "add",
            "/a",
            "--title",
            "Original",
            "--o2o",
            "axis=place",
            "--o2m",
            "mood=tense",
            "--o2m",
            "mood=hidden",
        ],
    );
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    // A reference must survive an edit (update() replaces child rows, so this
    // guards against edit silently dropping references it did not touch).
    assert!(unclip(&path, &["attach", "/a", "https://example.com"])
        .status
        .success());

    let edit = unclip(
        &path,
        &[
            "edit",
            "/a",
            "--clear-title",
            "--description",
            "now described",
            "--weight",
            "2.5",
            // Overwrite an existing o2o name (rejected by `add`, allowed here).
            "--o2o",
            "axis=time",
            "--add-o2m",
            "mood=calm",
            "--remove-o2m",
            "mood=hidden",
        ],
    );
    assert!(edit.status.success(), "edit failed: {}", stderr(&edit));

    let yaml = stdout(&unclip(&path, &["show", "/a"]));
    assert!(!yaml.contains("title:"), "title should be cleared: {yaml}");
    assert!(yaml.contains("description: now described"));
    assert!(yaml.contains("weight: 2.5"));
    // o2o overwritten in place.
    assert!(yaml.contains("axis: time"));
    assert!(!yaml.contains("place"));
    // o2m set: calm added, hidden removed, tense kept (sorted output).
    assert!(yaml.contains("- calm"));
    assert!(yaml.contains("- tense"));
    assert!(!yaml.contains("- hidden"));

    // The reference attached before the edit is still present afterwards.
    let refs = stdout(&unclip(&path, &["refs", "/a"]));
    assert!(refs.contains("https://example.com"), "ref dropped: {refs}");
}

/// `edit` on a missing branch fails, and an empty edit is a usage error.
#[test]
fn edit_rejects_missing_branch_and_empty_patch() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());

    let missing = unclip(&path, &["edit", "/nope", "--weight", "1.0"]);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("branch not found"));

    assert!(unclip(&path, &["add", "/a"]).status.success());
    let empty = unclip(&path, &["edit", "/a"]);
    assert!(!empty.status.success());
    assert!(stderr(&empty).contains("no changes requested"));

    // Removing an o2m value the branch does not carry is a set-semantics no-op,
    // so a patch made only of such removals is also "no changes".
    let noop = unclip(&path, &["edit", "/a", "--remove-o2m", "mood=absent"]);
    assert!(!noop.status.success());
    assert!(stderr(&noop).contains("no changes requested"));
}

/// A command other than `init` must refuse to silently create a fresh database.
#[test]
fn non_init_requires_existing_db() {
    let db = TempDb::new();
    let out = unclip(&db.path(), &["show", "/x"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("database not found"));
}

#[test]
fn level_plugins_does_not_require_a_database() {
    let db = TempDb::new();
    let out = unclip(&db.path(), &["level", "plugins"]);
    assert!(out.status.success(), "plugins failed: {}", stderr(&out));
    let plugins = stdout(&out);
    assert!(plugins.contains("sensor.coverage"));
    assert!(plugins.contains("compare.scalar-difference"));
    assert!(plugins.contains("compare.kendall"));
    assert!(plugins.contains("compare.rbo"));
    assert!(plugins.contains("compare.jensen-shannon"));
    assert!(plugins.contains("compare.pairwise-matrix"));
    assert!(plugins.contains("compare.spectrum"));
    assert!(plugins.contains("compare.partition-rand"));
    assert!(plugins.contains("compare.change-point-alignment"));
    assert!(plugins.contains("compare.graph-identity"));
    assert!(plugins.contains("sensor.residual"));
    assert!(plugins.contains("sensor.permutation"));
    assert!(plugins.contains("sensor.lehmer"));
    assert!(plugins.contains("sensor.kendall"));
    assert!(plugins.contains("sensor.rbo"));
    assert!(plugins.contains("generate.persistent-residual"));
    assert!(plugins.contains("generate.missing-relation"));
    assert!(plugins.contains("generate.recurring-motif"));
    assert!(plugins.contains("generate.pairwise-coupling"));
    assert!(plugins.contains("generate.temporal-coupling"));
    assert!(plugins.contains("generate.community"));
    assert!(plugins.contains("generate.latent-axis"));
    assert!(plugins.contains("null.random-cooccurrence"));
    assert!(plugins.contains("null.ranking-constraints"));
    assert!(plugins.contains("null.existing-unit"));
    assert!(plugins.contains("null.existing-relation"));
    assert!(plugins.contains("null.weight-change"));
    assert!(plugins.contains("null.contextual-cooccurrence"));
    assert!(plugins.contains("null.coupling-zero"));
    assert!(plugins.contains("null.existing-motif"));
    assert!(plugins.contains("null.existing-role"));
    assert!(plugins.contains("null.existing-transformation"));
    assert!(plugins.contains("interpret.llm-label"));
    for line in plugins.lines() {
        let operation = line.split("\t").nth(1).expect("plugin operation column");
        assert!(
            matches!(
                operation,
                "INFERRED" | "CALCULATED" | "EXPERIMENTAL" | "INTERPRETED"
            ),
            "unlabeled plugin output: {line}"
        );
    }
    assert!(!db.path().exists());
}

#[tokio::test]
async fn level_domain_frame_and_observe_workflow() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    let fixture = db.write(
        "domain.yaml",
        r#"domain:
  id: coffee
  version: "7"
  units:
    sensory:
      id: sensory
      kind: atomic_meaning
      label: Sensory
      properties:
        weight: 0.75
    social:
      id: social
      kind: semantic_role
      label: Social
      properties: {}
  relations:
    connects:
      id: connects
      source: sensory
      target: social
      kind: association
      properties: {}
"#,
    );

    let imported = unclip(
        &path,
        &["level", "domain", "import", fixture.to_str().unwrap()],
    );
    assert!(
        imported.status.success(),
        "domain import failed: {}",
        stderr(&imported)
    );
    assert!(stdout(&imported).contains("imported domain coffee@7"));

    let yaml = unclip(&path, &["level", "domain", "show", "coffee@7"]);
    assert!(
        yaml.status.success(),
        "domain show failed: {}",
        stderr(&yaml)
    );
    assert!(stdout(&yaml).contains("id: coffee"));
    assert!(stdout(&yaml).contains("version: '7'"));
    assert!(stdout(&yaml).contains("sensory"));

    let json = unclip(
        &path,
        &["level", "domain", "show", "coffee@7", "--format", "json"],
    );
    assert!(json.status.success(), "JSON show failed: {}", stderr(&json));
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(value["domain"]["id"], "coffee");
    assert_eq!(value["domain"]["version"], "7");
    assert_eq!(
        value["domain"]["units"]["sensory"]["properties"]["weight"],
        0.75
    );

    let observation_fixture = db.write(
        "observation.json",
        r#"{
  "observation": {
    "id": "manual-observation",
    "source": "observation.json",
    "observed_at": null,
    "units": [
      {
        "id": "observed",
        "label": "Observed",
        "salience": 0.8,
        "uncertainty": 0.1,
        "context": {}
      }
    ],
    "relations": [],
    "context": {}
  }
}"#,
    );
    let profile_fixture = db.write(
        "engine.json",
        &serde_json::json!({
            "domain": "coffee@7",
            "inferrers": [{
                "id": "infer.manual",
                "params": {"file": observation_fixture.to_str().unwrap()}
            }]
        })
        .to_string(),
    );
    let observed = unclip(
        &path,
        &[
            "level",
            "observe",
            observation_fixture.to_str().unwrap(),
            "--profile",
            profile_fixture.to_str().unwrap(),
        ],
    );
    assert!(
        observed.status.success(),
        "level observe failed: {}",
        stderr(&observed)
    );
    assert!(stdout(&observed).contains("INFERRED"));
    assert!(stdout(&observed).contains("infer.manual@1.0.0"));
    assert!(stdout(&observed).contains("observations=1"));
    let run_id = stdout(&observed)
        .lines()
        .find_map(|line| line.strip_prefix("RUN\t"))
        .expect("observe output should identify its persisted run")
        .to_owned();

    std::fs::remove_file(&observation_fixture).unwrap();
    let verified = unclip(&path, &["level", "verify", &run_id]);
    assert!(
        verified.status.success(),
        "level verify failed: {}",
        stderr(&verified)
    );
    assert!(stdout(&verified).contains(&format!(
        "VERIFIED\tINFERENCE_REPLAY\trun={run_id} observations=1 alignments=0 rankings=0 calculated=0"
    )));

    let derived_id = format!("{run_id}/infer.manual");
    let provenance = unclip(&path, &["level", "provenance", &derived_id]);
    assert!(
        provenance.status.success(),
        "level provenance failed: {}",
        stderr(&provenance)
    );
    assert!(stdout(&provenance).contains(&format!(
        "{derived_id}\tINFERRED\tinfer.manual@1.0.0\tdepth=0 inputs=0"
    )));
    let missing_provenance = unclip(&path, &["level", "provenance", "missing"]);
    assert!(!missing_provenance.status.success());
    assert!(stderr(&missing_provenance).contains("provenance not found: missing"));

    let connection = unclip_store::connect(&format!("sqlite://{}?mode=rw", path.display()))
        .await
        .unwrap();
    let runs = unclip_store::SeaOrmEngineRunRepository::new(connection);
    let replay = unclip_store::EngineRunRepository::replay_run(&runs, &run_id)
        .await
        .unwrap()
        .expect("observe run should be persisted");
    assert_eq!(replay.run.status, unclip_store::EngineRunStatus::Completed);
    assert_eq!(replay.observations.len(), 1);
    assert_eq!(replay.observations[0].value.id.0, "manual-observation");
    assert_eq!(replay.provenance_ids.len(), 1);

    let explained = unclip(&path, &["level", "explain", "manual-observation"]);
    assert!(
        explained.status.success(),
        "level explain failed: {}",
        stderr(&explained)
    );
    assert!(stdout(&explained).contains("OBSERVATION\tINFERRED\tinfer.manual@1.0.0"));
    assert!(stdout(&explained).contains("id=manual-observation units=1 relations=0"));
    let discovery_profile = db.write("discovery.json", &serde_json::json!({"domain":"coffee@7","candidate_generators":[{"id":"generate.persistent-residual","params":{"minimum_observations":2}}]}).to_string());
    let discovered = unclip(
        &path,
        &[
            "level",
            "discover",
            "--profile",
            discovery_profile.to_str().unwrap(),
            "--observation",
            "manual-observation",
        ],
    );
    assert!(
        discovered.status.success(),
        "discovery failed: {}",
        stderr(&discovered)
    );
    assert!(stdout(&discovered).contains("CALCULATED\tDISCOVERY\trun="));
    assert!(!stdout(&discovered).contains("CANDIDATE\t"));
    let discovery_output = stdout(&discovered);
    let discovery_run = discovery_output
        .lines()
        .find_map(|line| line.strip_prefix("CALCULATED\tDISCOVERY\trun="))
        .expect("discovery output should identify its run");
    let verified = unclip(&path, &["level", "verify", discovery_run]);
    assert!(
        verified.status.success(),
        "verify failed: {}",
        stderr(&verified)
    );
    assert!(stdout(&verified).contains("VERIFIED\tDISCOVERY_REPLAY"));
    assert!(stdout(&verified).contains("candidates=0"));
    let listed = unclip(&path, &["level", "candidates", "--domain", "coffee@7"]);
    assert!(listed.status.success());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&listed)).unwrap(),
        serde_json::json!([])
    );
    let missing = unclip(
        &path,
        &[
            "level",
            "discover",
            "--profile",
            discovery_profile.to_str().unwrap(),
            "--observation",
            "missing",
        ],
    );
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("observation not found"));
    let duplicate = unclip(
        &path,
        &[
            "level",
            "discover",
            "--profile",
            discovery_profile.to_str().unwrap(),
            "--observation",
            "manual-observation",
            "--observation",
            "manual-observation",
        ],
    );
    assert!(!duplicate.status.success());
    assert!(stderr(&duplicate).contains("duplicate observation selection"));

    let frame_fixture = db.write(
        "frame.yaml",
        r#"measurement_frame:
  domain_id: coffee
  domain_version: "7"
  frame:
    id: coffee.general
    version: "2"
    axes:
      - unit: sensory
        label: Sensory axis
      - unit: social
        label: null
"#,
    );
    let frame_imported = unclip(
        &path,
        &["level", "frame", "import", frame_fixture.to_str().unwrap()],
    );
    assert!(
        frame_imported.status.success(),
        "frame import failed: {}",
        stderr(&frame_imported)
    );
    assert!(stdout(&frame_imported).contains("coffee.general@2"));

    let frame_yaml = unclip(&path, &["level", "frame", "show", "coffee.general@2"]);
    assert!(
        frame_yaml.status.success(),
        "frame show failed: {}",
        stderr(&frame_yaml)
    );
    assert!(stdout(&frame_yaml).contains("id: coffee.general"));
    assert!(stdout(&frame_yaml).contains("unit: sensory"));

    let frame_json = unclip(
        &path,
        &[
            "level",
            "frame",
            "show",
            "coffee.general@2",
            "--format",
            "json",
        ],
    );
    assert!(
        frame_json.status.success(),
        "frame JSON show failed: {}",
        stderr(&frame_json)
    );
    let frame_value: serde_json::Value = serde_json::from_str(&stdout(&frame_json)).unwrap();
    assert_eq!(frame_value["id"], "coffee.general");
    assert_eq!(frame_value["version"], "2");
    assert_eq!(frame_value["axes"][0]["unit"], "sensory");

    let measurement_profile = db.write(
        "measure.json",
        &serde_json::json!({
            "domain": "coffee@7",
            "frame": "coffee.general@2",
            "sensors": [{"id": "sensor.coverage"}]
        })
        .to_string(),
    );
    let measured = unclip(
        &path,
        &[
            "level",
            "measure",
            "manual-observation",
            "--profile",
            measurement_profile.to_str().unwrap(),
        ],
    );
    assert!(
        measured.status.success(),
        "level measure failed: {}",
        stderr(&measured)
    );
    assert!(stdout(&measured).contains("MEASUREMENT\tCALCULATED\tsensor.coverage@0.1.0"));
    let measured_stdout = stdout(&measured);
    let profile_id = measured_stdout
        .lines()
        .find_map(|line| line.strip_prefix("PROFILE\tCALCULATED\t"))
        .expect("measure output should identify its persisted profile");

    let profile_yaml = unclip(&path, &["level", "profile", profile_id]);
    assert!(
        profile_yaml.status.success(),
        "profile YAML failed: {}",
        stderr(&profile_yaml)
    );
    assert!(stdout(&profile_yaml).contains("measurement_profile:"));
    assert!(stdout(&profile_yaml).contains("sensor: sensor.coverage"));

    let profile_json = unclip(&path, &["level", "profile", profile_id, "--format", "json"]);
    assert!(
        profile_json.status.success(),
        "profile JSON failed: {}",
        stderr(&profile_json)
    );
    let profile_value: serde_json::Value = serde_json::from_str(&stdout(&profile_json)).unwrap();
    assert_eq!(
        profile_value["measurement_profile"]["measurements"][0]["sensor"],
        "sensor.coverage"
    );

    let profile_table = unclip(&path, &["level", "profile", profile_id, "--table"]);
    assert!(
        profile_table.status.success(),
        "profile table failed: {}",
        stderr(&profile_table)
    );
    let table = stdout(&profile_table);
    assert!(table.contains("CALCULATED SENSOR RESULTS"));
    assert!(table.contains("| Context | sensor.coverage@0.1.0 |"));
    assert!(table.contains("samples="));
    let conflicting_format = unclip(
        &path,
        &[
            "level", "profile", profile_id, "--table", "--format", "json",
        ],
    );
    assert!(!conflicting_format.status.success());
    assert!(stderr(&conflicting_format).contains("cannot be used with"));
    let missing_table = unclip(&path, &["level", "profile", "missing-profile", "--table"]);
    assert!(!missing_table.status.success());
    assert!(stderr(&missing_table).contains("measurement profile not found"));

    let measurement_run_id = profile_id.strip_suffix("/profile").unwrap();
    let verified_measurement = unclip(&path, &["level", "verify", measurement_run_id]);
    assert!(
        verified_measurement.status.success(),
        "single-observation verification failed: {}",
        stderr(&verified_measurement)
    );
    assert!(stdout(&verified_measurement).contains("observations=1"));

    let profile_jsonl = unclip(
        &path,
        &["level", "profile", profile_id, "--format", "jsonl"],
    );
    assert!(!profile_jsonl.status.success());
    assert!(stderr(&profile_jsonl).contains("JSONL is not supported"));

    let connection = unclip_store::connect(&format!("sqlite://{}?mode=rw", path.display()))
        .await
        .unwrap();
    let measurements = unclip_store::SeaOrmMeasurementRepository::new(connection.clone());
    let stored = unclip_store::MeasurementRepository::get_profile(&measurements, profile_id)
        .await
        .unwrap()
        .expect("measurement profile should be persisted");
    assert_eq!(stored.measurements.len(), 1);
    assert_eq!(stored.measurements[0].sensor.0, "sensor.coverage");

    let training_evidence = unclip_epistemic::DerivedId::new("training-evidence");
    let root_params = serde_json::json!({"fixture":"training-only"});
    let provenance = unclip_store::SeaOrmProvenanceRepository::new(connection.clone());
    unclip_store::ProvenanceRepository::insert_provenance(
        &provenance,
        unclip_store::StoredProvenance {
            id: training_evidence.clone(),
            run_id: None,
            provenance: unclip_epistemic::Provenance {
                operation: unclip_epistemic::Operation::Calculated,
                producer: unclip_epistemic::PluginId::new("fixture.training"),
                algorithm: "fixture.training".into(),
                version: "0.1.0".parse().unwrap(),
                params_hash: unclip_epistemic::hash_params(&root_params),
                params: root_params,
                inputs: vec![],
                source: None,
                timestamp: unclip_epistemic::Timestamp::new("2026-09-22T00:00:00Z"),
                domain_version: Some(unclip_epistemic::DomainVersion::new("7")),
                frame_version: None,
                model: None,
            },
        },
    )
    .await
    .unwrap();
    let leaked_intermediate = unclip_epistemic::DerivedId::new("leaked-intermediate");
    let leaked_params = serde_json::json!({"fixture":"transitive-leak"});
    unclip_store::ProvenanceRepository::insert_provenance(
        &provenance,
        unclip_store::StoredProvenance {
            id: leaked_intermediate.clone(),
            run_id: None,
            provenance: unclip_epistemic::Provenance {
                operation: unclip_epistemic::Operation::Calculated,
                producer: unclip_epistemic::PluginId::new("fixture.intermediate"),
                algorithm: "fixture.intermediate".into(),
                version: "0.1.0".parse().unwrap(),
                params_hash: unclip_epistemic::hash_params(&leaked_params),
                params: leaked_params,
                inputs: vec![unclip_epistemic::DerivedId::new(&derived_id)],
                source: None,
                timestamp: unclip_epistemic::Timestamp::new("2026-09-22T00:00:00Z"),
                domain_version: Some(unclip_epistemic::DomainVersion::new("7")),
                frame_version: None,
                model: None,
            },
        },
    )
    .await
    .unwrap();
    let proposal = unclip_store::CandidateProposal {
        domain_version_id: serde_json::to_string(&("coffee", "7")).unwrap(),
        kind: unclip_store::CandidateKind::AtomicMeaning,
        value: serde_json::json!({
            "pattern":{"matching":"exact_observed_label","observed_label":"new evidence"},
            "fixture":"training-only"
        })
        .as_object()
        .unwrap()
        .clone(),
    };
    let make_candidate = |id: &str, input: unclip_epistemic::DerivedId| {
        let dependencies = unclip_epistemic::DependencyCollector::default();
        dependencies.read(&unclip_epistemic::Tracked::from_recorded(input, ()));
        let params = serde_json::json!({"fixture":true});
        unclip_epistemic::CalculationToken::from_harness(
            unclip_epistemic::EmitMetadata {
                id: unclip_epistemic::DerivedId::new(id),
                producer: unclip_epistemic::PluginId::new("generate.fixture"),
                algorithm: "generate.fixture".into(),
                version: "0.1.0".parse().unwrap(),
                params_hash: unclip_epistemic::hash_params(&params),
                params,
                source: None,
                timestamp: unclip_epistemic::Timestamp::new("2026-09-22T00:00:00Z"),
                domain_version: Some(unclip_epistemic::DomainVersion::new("7")),
                frame_version: None,
                model: None,
            },
            dependencies,
        )
        .emit(proposal.clone())
    };
    let candidate_id = unclip_epistemic::DerivedId::new("experiment-candidate");
    let leaked_candidate_id = unclip_epistemic::DerivedId::new("leaked-candidate");
    let experiments = unclip_store::SeaOrmExperimentRepository::new(connection.clone());
    unclip_store::CandidateRepository::insert_candidate(
        &experiments,
        None,
        make_candidate(&candidate_id.0, training_evidence),
    )
    .await
    .unwrap();
    unclip_store::CandidateRepository::insert_candidate(
        &experiments,
        None,
        make_candidate(&leaked_candidate_id.0, leaked_intermediate),
    )
    .await
    .unwrap();
    let domains = unclip_store::SeaOrmDomainRepository::new(connection.clone());
    let active_domain_id = unclip_domain::DomainId::new("coffee");
    let active_domain_version = unclip_epistemic::DomainVersion::new("7");
    let active_domain_before = unclip_store::DomainReader::get_domain_version(
        &domains,
        &active_domain_id,
        &active_domain_version,
    )
    .await
    .unwrap()
    .expect("active domain should exist before experiments");
    let experiment_profile = db.write(
        "experiment-profile.json",
        &serde_json::json!({
            "domain":"coffee@7",
            "frame":"coffee.general@2",
            "sensors":[{"id":"sensor.coverage"}],
            "comparators":[{"id":"compare.scalar-difference"}],
            "null_models":[{"id":"null.existing-unit"}]
        })
        .to_string(),
    );
    let experiment_request = db.write(
        "experiment-request.json",
        &serde_json::json!({
            "run_id":"experiment-cli",
            "candidate":candidate_id,
            "training":[],
            "held_out":["manual-observation"],
            "comparison_sensor":"sensor.coverage",
            "constraints":[{
                "kind":"complexity_budget",
                "maximum_added_units":1,
                "maximum_added_relations":0,
                "maximum_property_changes":0
            }],
            "pareto_dimensions":[{
                "name":"coverage",
                "left":"experiment-cli/before/sensor.coverage",
                "right":"experiment-cli/after/sensor.coverage",
                "direction":"maximize"
            }]
        })
        .to_string(),
    );
    let executed = unclip(
        &path,
        &[
            "level",
            "experiment",
            "--profile",
            experiment_profile.to_str().unwrap(),
            "--request",
            experiment_request.to_str().unwrap(),
        ],
    );
    assert!(
        executed.status.success(),
        "level experiment failed: {}",
        stderr(&executed)
    );
    assert!(
        stdout(&executed).contains("EXPERIMENT\tEXPERIMENTAL\texperiment-cli/experiment/completed")
    );
    let result = stdout(&executed)
        .lines()
        .find_map(|line| line.strip_prefix("RESULT\t"))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .expect("experiment output should contain typed result JSON");
    assert_eq!(result["null_results"].as_array().unwrap().len(), 1);
    assert_eq!(result["constraints"][0]["status"], "satisfied");
    assert_eq!(result["pareto"]["relation"], "incomparable");
    assert_eq!(
        result["delta_profile"]["deltas"].as_array().unwrap().len(),
        1
    );
    let repeated_request = db.write(
        "repeated-experiment-request.json",
        &serde_json::json!({
            "run_id":"experiment-repeated",
            "candidate":candidate_id,
            "training":[],
            "held_out":["manual-observation"],
            "comparison_sensor":"sensor.coverage",
            "constraints":[{
                "kind":"complexity_budget",
                "maximum_added_units":1,
                "maximum_added_relations":0,
                "maximum_property_changes":0
            }],
            "pareto_dimensions":[{
                "name":"coverage",
                "left":"experiment-repeated/before/sensor.coverage",
                "right":"experiment-repeated/after/sensor.coverage",
                "direction":"maximize"
            }]
        })
        .to_string(),
    );
    let repeated = unclip(
        &path,
        &[
            "level",
            "experiment",
            "--profile",
            experiment_profile.to_str().unwrap(),
            "--request",
            repeated_request.to_str().unwrap(),
        ],
    );
    assert!(
        repeated.status.success(),
        "repeated level experiment failed: {}",
        stderr(&repeated)
    );
    let repeated_result = stdout(&repeated)
        .lines()
        .find_map(|line| line.strip_prefix("RESULT\t"))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .expect("repeated experiment output should contain typed result JSON");
    assert_eq!(
        canonical_experiment_result(result.clone(), "experiment-cli"),
        canonical_experiment_result(repeated_result, "experiment-repeated"),
        "the same evidence and configuration must reproduce calculated result values"
    );
    let leaked_request = db.write(
        "leaked-experiment-request.json",
        &serde_json::json!({
            "run_id":"experiment-leak",
            "candidate":leaked_candidate_id,
            "training":[],
            "held_out":["manual-observation"],
            "comparison_sensor":"sensor.coverage"
        })
        .to_string(),
    );
    let rejected = unclip(
        &path,
        &[
            "level",
            "experiment",
            "--profile",
            experiment_profile.to_str().unwrap(),
            "--request",
            leaked_request.to_str().unwrap(),
        ],
    );
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains("candidate provenance depends on held-out evidence"),
        "unexpected leakage error: {}",
        stderr(&rejected)
    );
    let runs = unclip_store::SeaOrmEngineRunRepository::new(connection.clone());
    assert!(
        unclip_store::EngineRunRepository::get_run(&runs, "experiment-leak")
            .await
            .unwrap()
            .is_none(),
        "rejected leakage must not create an engine run"
    );
    let completed = unclip_store::ExperimentRepository::get_completed_experiment(
        &experiments,
        &unclip_epistemic::DerivedId::new("experiment-cli/experiment/completed"),
    )
    .await
    .unwrap()
    .expect("completed experiment should be persisted");
    assert_eq!(
        completed.outcome.held_out,
        vec![unclip_observe::ObservationId::new("manual-observation")]
    );
    assert_eq!(completed.deltas.len(), 1);
    let repeated_completed = unclip_store::ExperimentRepository::get_completed_experiment(
        &experiments,
        &unclip_epistemic::DerivedId::new("experiment-repeated/experiment/completed"),
    )
    .await
    .unwrap()
    .expect("repeated completed experiment should be persisted");
    assert_eq!(completed.outcome.plan, repeated_completed.outcome.plan);
    assert_eq!(
        completed.outcome.training,
        repeated_completed.outcome.training
    );
    assert_eq!(
        completed.outcome.held_out,
        repeated_completed.outcome.held_out
    );
    assert_eq!(completed.deltas.len(), repeated_completed.deltas.len());
    for (first, repeated) in completed.deltas.iter().zip(&repeated_completed.deltas) {
        assert_eq!(first.comparator_version, repeated.comparator_version);
        assert_eq!(first.delta, repeated.delta);
    }
    for side in ["before", "after"] {
        let first = unclip_store::MeasurementRepository::get_profile(
            &measurements,
            &format!("experiment-cli/{side}-profile"),
        )
        .await
        .unwrap()
        .expect("first calculated measurement profile should be persisted");
        let repeated = unclip_store::MeasurementRepository::get_profile(
            &measurements,
            &format!("experiment-repeated/{side}-profile"),
        )
        .await
        .unwrap()
        .expect("repeated calculated measurement profile should be persisted");
        assert_eq!(first, repeated, "{side} measurements must reproduce");
    }
    let provenance = unclip_store::SeaOrmProvenanceRepository::new(connection.clone());
    for id in [
        "experiment-cli/nulls/null.existing-unit",
        "experiment-cli/constraints",
        "experiment-cli/pareto",
        "experiment-cli/comparison/profile",
        "experiment-cli/experiment",
        "experiment-cli/experiment/completed",
    ] {
        let stored = unclip_store::ProvenanceRepository::get_provenance(
            &provenance,
            &unclip_epistemic::DerivedId::new(id),
        )
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{id} provenance should be persisted"));
        if matches!(
            id,
            "experiment-cli/experiment" | "experiment-cli/experiment/completed"
        ) {
            assert_eq!(
                stored.provenance.operation,
                unclip_epistemic::Operation::Experimental
            );
        }
    }
    assert!(unclip_store::MeasurementRepository::get_profile(
        &measurements,
        "experiment-cli/before-profile"
    )
    .await
    .unwrap()
    .is_some());

    let observation_records = unclip_store::SeaOrmObservationRepository::new(connection.clone());
    unclip_store::ObservationRepository::insert_alignment(
        &observation_records,
        "manual-alignment",
        unclip_observe::Alignment {
            observation: unclip_observe::ObservationId::new("manual-observation"),
            candidates: vec![unclip_observe::AlignmentCandidate {
                observed: unclip_observe::ObservedUnitId::new("observed"),
                domain: unclip_domain::UnitId::new("sensory"),
                confidence: 1.0,
                evidence: vec![],
            }],
        },
        &active_domain_id,
        &active_domain_version,
        &unclip_epistemic::DerivedId::new(&derived_id),
    )
    .await
    .unwrap();
    unclip_store::ObservationRepository::insert_ranking(
        &observation_records,
        "manual-ranking",
        unclip_observe::PartialRanking {
            observation: unclip_observe::ObservationId::new("manual-observation"),
            tiers: vec![unclip_observe::RankTier {
                units: vec![unclip_observe::ObservedUnitId::new("observed")],
            }],
            unknown: vec![],
        },
        &unclip_epistemic::DerivedId::new(&derived_id),
    )
    .await
    .unwrap();
    let ranking_profile = db.write(
        "ranking-experiment-profile.json",
        &serde_json::json!({
            "domain":"coffee@7",
            "frame":"coffee.general@2",
            "sensors":[{"id":"sensor.permutation"}],
            "comparators":[{"id":"compare.rbo","params":{"p":0.9}}]
        })
        .to_string(),
    );
    let ranking_request = db.write(
        "ranking-experiment-request.json",
        &serde_json::json!({
            "run_id":"experiment-ranking",
            "candidate":candidate_id,
            "training":[],
            "held_out":["manual-observation"],
            "comparison_sensor":"sensor.permutation"
        })
        .to_string(),
    );
    let ranked = unclip(
        &path,
        &[
            "level",
            "experiment",
            "--profile",
            ranking_profile.to_str().unwrap(),
            "--request",
            ranking_request.to_str().unwrap(),
        ],
    );
    assert!(
        ranked.status.success(),
        "ranking experiment failed: {}",
        stderr(&ranked)
    );
    let ranking_result = stdout(&ranked)
        .lines()
        .find_map(|line| line.strip_prefix("RESULT\t"))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .expect("ranking experiment should emit typed result JSON");
    let ranking_delta = &ranking_result["delta_profile"]["deltas"][0]["delta"];
    assert_eq!(ranking_delta["comparator"], "compare.rbo");
    assert_eq!(ranking_delta["value"]["kind"], "structured");
    assert_eq!(ranking_delta["value"]["value"]["status"], "rbo");
    assert_eq!(ranking_delta["value"]["value"]["similarity"], 1.0);
    assert_eq!(
        ranking_delta["value"]["value"]["before"]["tiers"][0][0],
        "sensory"
    );
    assert_eq!(
        ranking_delta["value"]["value"]["before"]["unknown"],
        serde_json::json!(["social"])
    );
    let ranking_completed = unclip_store::ExperimentRepository::get_completed_experiment(
        &experiments,
        &unclip_epistemic::DerivedId::new("experiment-ranking/experiment/completed"),
    )
    .await
    .unwrap()
    .expect("ranking experiment should be persisted");
    assert_eq!(ranking_completed.deltas.len(), 1);
    assert_eq!(
        ranking_completed.deltas[0].delta.comparator,
        unclip_epistemic::PluginId::new("compare.rbo")
    );
    assert_eq!(
        ranking_completed.outcome.plan["comparators"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ranking_completed.outcome.plan["comparators"][0]["id"],
        "compare.rbo"
    );
    assert!(matches!(
        &ranking_completed.deltas[0].delta.value,
        unclip_measure::MeasurementValue::Structured(value) if value["status"] == "rbo"
    ));
    assert!(
        !serde_json::to_string(&ranking_completed.outcome)
            .unwrap()
            .contains("compare.scalar-difference"),
        "the ranking experiment must not require a scalar comparator"
    );

    let applied = unclip(
        &path,
        &[
            "level",
            "apply",
            &candidate_id.0,
            "--experiment",
            "experiment-cli/experiment/completed",
            "--target-domain",
            "coffee@8",
            "--reason",
            "held-out coverage and the explicit complexity budget support promotion",
        ],
    );
    assert!(
        applied.status.success(),
        "level apply failed: {}",
        stderr(&applied)
    );
    let applied_output = stdout(&applied);
    let revision_id = applied_output
        .lines()
        .find_map(|line| {
            line.strip_prefix("APPLIED\tEXPERIMENTAL\t")
                .and_then(|line| line.strip_suffix("\tdomain=coffee@8"))
        })
        .expect("apply output should identify its revision");
    let successor = unclip_store::DomainReader::get_domain_version(
        &domains,
        &active_domain_id,
        &unclip_epistemic::DomainVersion::new("8"),
    )
    .await
    .unwrap()
    .expect("apply should create the explicit successor version");
    assert!(successor.units.contains_key(&unclip_domain::UnitId::new(
        "candidate:experiment-candidate"
    )));
    let ledger = unclip_store::DomainRevisionRepository::get_domain_revision_ledger(
        &experiments,
        &unclip_epistemic::DerivedId::new(revision_id),
    )
    .await
    .unwrap()
    .expect("apply should persist a reconstructable revision ledger");
    assert_eq!(ledger.candidate.id, candidate_id);
    assert_eq!(
        ledger.experiment.id,
        unclip_epistemic::DerivedId::new("experiment-cli/experiment/completed")
    );
    assert_eq!(ledger.revision.revision.to_version_id, r#"["coffee","8"]"#);
    assert_eq!(
        ledger.revision.revision.reason,
        "held-out coverage and the explicit complexity budget support promotion"
    );
    assert_eq!(
        ledger
            .provenance
            .iter()
            .find(|value| value.id == ledger.revision.id)
            .expect("revision provenance should be in the ledger")
            .provenance
            .operation,
        unclip_epistemic::Operation::Experimental
    );

    let stale = unclip(
        &path,
        &[
            "level",
            "apply",
            &candidate_id.0,
            "--experiment",
            "experiment-cli/experiment/completed",
            "--target-domain",
            "coffee@9",
            "--reason",
            "stale retry must fail",
        ],
    );
    assert!(!stale.status.success());
    assert!(stderr(&stale).contains("reload and retry"));
    assert!(
        unclip_store::DomainReader::get_domain_version(
            &domains,
            &active_domain_id,
            &unclip_epistemic::DomainVersion::new("9"),
        )
        .await
        .unwrap()
        .is_none(),
        "stale application must not create another successor"
    );

    let active_domain_after = unclip_store::DomainReader::get_domain_version(
        &domains,
        &active_domain_id,
        &active_domain_version,
    )
    .await
    .unwrap()
    .expect("active domain should remain after experiments");
    assert_eq!(
        active_domain_after, active_domain_before,
        "experiment candidate application must not mutate the stored active domain"
    );
    assert!(!active_domain_after
        .units
        .contains_key(&unclip_domain::UnitId::new(
            "candidate:experiment-candidate"
        )));
    for version in [
        "counterfactual:experiment-cli/application",
        "counterfactual:experiment-repeated/application",
        "counterfactual:experiment-leak/application",
        "counterfactual:experiment-ranking/application",
    ] {
        assert!(
            unclip_store::DomainReader::get_domain_version(
                &domains,
                &active_domain_id,
                &unclip_epistemic::DomainVersion::new(version),
            )
            .await
            .unwrap()
            .is_none(),
            "temporary counterfactual version must not be persisted: {version}"
        );
    }

    let unversioned = unclip(&path, &["level", "domain", "show", "coffee"]);
    assert!(!unversioned.status.success());
    assert!(stderr(&unversioned).contains("domain@version"));
}

#[test]
fn level_help_lists_plugins_command() {
    let db = TempDb::new();
    let out = unclip(&db.path(), &["level", "--help"]);
    assert!(out.status.success(), "help failed: {}", stderr(&out));
    assert!(stdout(&out).contains("plugins"));
    assert!(stdout(&out).contains("apply"));
    assert!(stdout(&out).contains("interpret"));
    assert!(stdout(&out).contains("candidates"));
    assert!(stdout(&out).contains("experiment"));
    assert!(stdout(&out).contains("verify"));
    let verify_help = unclip(&db.path(), &["level", "verify", "--help"]);
    assert!(verify_help.status.success());
    assert!(stdout(&verify_help).contains("Replay persisted inference"));
    let profile_help = unclip(&db.path(), &["level", "profile", "--help"]);
    assert!(profile_help.status.success());
    assert!(stdout(&profile_help).contains("--table"));
    assert!(stdout(&profile_help).contains("side by side"));
    assert!(!db.path().exists());
}

/// Adding the same path twice is a usage error.
#[test]
fn duplicate_add_is_rejected() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    assert!(unclip(&path, &["add", "/a"]).status.success());

    let again = unclip(&path, &["add", "/a"]);
    assert!(!again.status.success());
    assert!(stderr(&again).contains("already exists"));
}

/// A repeated one-to-one o2o name on `add` is rejected (o2o is one value/name).
#[test]
fn duplicate_o2o_name_on_add_is_rejected() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);

    let out = unclip(
        &path,
        &["add", "/a", "--o2o", "axis=place", "--o2o", "axis=time"],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("duplicate o2o name"));
}

/// Repeated `--avoid-o2o` names accumulate: every listed value is excluded
/// (matching `--avoid-o2m`), unlike `--o2o`, which is one required value/name.
#[test]
fn repeated_avoid_o2o_names_accumulate_on_query() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    assert!(unclip(&path, &["add", "/a", "--o2o", "axis=place"])
        .status
        .success());
    assert!(unclip(&path, &["add", "/b", "--o2o", "axis=time"])
        .status
        .success());
    assert!(unclip(&path, &["add", "/c"]).status.success());

    let out = unclip(
        &path,
        &[
            "query",
            "--avoid-o2o",
            "axis=place",
            "--avoid-o2o",
            "axis=time",
        ],
    );
    assert!(out.status.success(), "query failed: {}", stderr(&out));
    let got = stdout(&out);
    assert!(got.contains("/c"), "got: {got}");
    assert!(!got.contains("/a"), "got: {got}");
    assert!(!got.contains("/b"), "got: {got}");
}

/// `sample --seed` is reproducible, and `--dry-run` records no usage.
#[test]
fn sample_seed_is_reproducible_and_dry_run_records_nothing() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    for p in ["/a", "/b", "/c", "/d", "/e"] {
        assert!(unclip(&path, &["add", p]).status.success());
    }

    let one = unclip(
        &path,
        &[
            "sample",
            "--count",
            "3",
            "--seed",
            "42",
            "--format",
            "jsonl",
            "--dry-run",
        ],
    );
    let two = unclip(
        &path,
        &[
            "sample",
            "--count",
            "3",
            "--seed",
            "42",
            "--format",
            "jsonl",
            "--dry-run",
        ],
    );
    assert!(one.status.success() && two.status.success());
    // The packets carry a wall-clock `created_at`, so compare only the
    // deterministic tail (the selections), which the seed fixes.
    let selections = |s: String| s[s.find("\"selections\"").expect("has selections")..].to_string();
    assert_eq!(
        selections(stdout(&one)),
        selections(stdout(&two)),
        "same seed must reproduce selections"
    );

    // Dry-run must not have recorded usage.
    let stats = unclip(&path, &["stats"]);
    assert!(stdout(&stats).contains("total uses: 0"));

    // A real sample records usage.
    assert!(unclip(&path, &["sample", "--count", "1", "--seed", "1"])
        .status
        .success());
    let stats = unclip(&path, &["stats"]);
    assert!(stdout(&stats).contains("total uses: 1"));

    // The complete u64 seed range must survive packet persistence.
    let high_seed = unclip(&path, &["sample", "--seed", "18446744073709551615"]);
    assert!(
        high_seed.status.success(),
        "high seed failed: {}",
        stderr(&high_seed)
    );
    assert!(stdout(&high_seed).contains("seed: 18446744073709551615"));
}

#[test]
fn sample_rejects_zero_count() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());

    let output = unclip(&path, &["sample", "--count", "0"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("sample count must be greater than zero"));
}

/// `export` returns every matching branch with no count to invent.
#[test]
fn export_returns_all_matches() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    for p in ["/a", "/b", "/c"] {
        unclip(&path, &["add", p]);
    }

    let out = unclip(&path, &["export", "--format", "jsonl"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).lines().count(), 3);
}

/// Frame lifecycle: import a frame file, list/show it, `create` a skeleton
/// branch from a slot, and `validate` both a conforming and a non-conforming
/// branch against that slot.
#[test]
fn frame_import_create_validate_flow() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);

    let frames = db.write(
        "frames.yaml",
        "\
frames:
  story:
    description: narrative scaffolding
    slots:
      - name: place
        require_o2o:
          domain: story
          axis: place
        default_o2o:
          use: scene-anchor
",
    );

    let imported = unclip(&path, &["import-frames", frames.to_str().unwrap()]);
    assert!(
        imported.status.success(),
        "import-frames failed: {}",
        stderr(&imported)
    );

    // The frame is listed and shows its slot.
    assert!(stdout(&unclip(&path, &["frames"])).contains("story"));
    let frame_yaml = stdout(&unclip(&path, &["frame", "story"]));
    assert!(frame_yaml.contains("name: place"));

    // `create` builds a skeleton branch carrying the slot's required o2o.
    let created = unclip(&path, &["create", "/scene/alley", "--frame", "story.place"]);
    assert!(
        created.status.success(),
        "create failed: {}",
        stderr(&created)
    );
    let shown = stdout(&unclip(&path, &["show", "/scene/alley"]));
    assert!(shown.contains("axis: place"));
    assert!(shown.contains("domain: story"));

    // The skeleton satisfies the slot it was created from.
    let ok = unclip(
        &path,
        &["validate", "/scene/alley", "--frame", "story.place"],
    );
    assert!(ok.status.success(), "validate should pass: {}", stderr(&ok));
    assert!(stdout(&ok).contains("OK"));

    // A plain branch missing the required o2o fails validation.
    unclip(&path, &["add", "/plain"]);
    let bad = unclip(&path, &["validate", "/plain", "--frame", "story.place"]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("violation"));
}

/// `compose` must never emit or persist a packet that violates slot counts.
#[test]
fn compose_rejects_insufficient_candidates() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);

    let frames = db.write(
        "frames.yaml",
        "frames:\n  story:\n    slots:\n      - name: place\n        count: 2\n        require_o2o:\n          axis: place\n",
    );
    assert!(unclip(&path, &["import-frames", frames.to_str().unwrap()])
        .status
        .success());
    assert!(unclip(&path, &["add", "/only", "--o2o", "axis=place"])
        .status
        .success());

    let out = unclip(&path, &["compose", "--frame", "story"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("requires 2 selection(s), but only 1 candidate(s) match"));
    assert!(stdout(&unclip(&path, &["stats"])).contains("total uses: 0"));
}

/// A user-controlled batch count must not trigger unbounded allocation/work.
#[test]
fn compose_rejects_oversized_batch() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);

    let frames = db.write(
        "frames.yaml",
        "frames:\n  story:\n    slots:\n      - name: place\n",
    );
    assert!(unclip(&path, &["import-frames", frames.to_str().unwrap()])
        .status
        .success());

    let out = unclip(&path, &["compose", "--frame", "story", "--count", "1001"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("compose count must not exceed 1000"));
}

/// `pattern add` requires exactly one target, and a stored pattern is then
/// surfaced by `scan` over a text file.
#[test]
fn pattern_add_and_scan() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    unclip(&path, &["add", "/ikebukuro/coin-locker"]);

    // Zero targets is rejected.
    let none = unclip(&path, &["pattern", "add", "coin locker"]);
    assert!(!none.status.success());
    assert!(stderr(&none).contains("exactly one"));

    // Two targets is rejected.
    let two = unclip(
        &path,
        &[
            "pattern",
            "add",
            "coin locker",
            "--o2m",
            "object=locker",
            "--branch",
            "/ikebukuro/coin-locker",
        ],
    );
    assert!(!two.status.success());

    // Exactly one target is accepted and listed.
    let add = unclip(
        &path,
        &[
            "pattern",
            "add",
            "coin locker",
            "--branch",
            "/ikebukuro/coin-locker",
        ],
    );
    assert!(add.status.success(), "pattern add failed: {}", stderr(&add));
    assert!(stdout(&unclip(&path, &["patterns"])).contains("coin locker"));

    // `scan` finds the pattern in a text file.
    let text = db.write("scene.txt", "I found a coin locker by the exit.");
    let scan = unclip(&path, &["scan", text.to_str().unwrap()]);
    assert!(scan.status.success());
    assert!(stdout(&scan).contains("coin locker"));
}

/// `scan` matches CJK patterns embedded in running Japanese prose, where the
/// neighbouring characters are kana/kanji rather than spaces or punctuation.
#[test]
fn scan_matches_inside_japanese_prose() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    unclip(&path, &["add", "/池袋/駅前"]);

    let add = unclip(&path, &["pattern", "add", "駅前", "--branch", "/池袋/駅前"]);
    assert!(add.status.success(), "pattern add failed: {}", stderr(&add));

    let text = db.write("scene_ja.txt", "夕方の駅前は人であふれていた。");
    let scan = unclip(&path, &["scan", text.to_str().unwrap()]);
    assert!(scan.status.success());
    assert!(stdout(&scan).contains("駅前"));
}

/// `suggest-o2m` proposes a catalog o2m value present in a branch's text but
/// not yet set on it.
#[test]
fn suggest_o2m_proposes_missing_value() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);

    // Seed the o2m catalog with `density=crowded` via one branch.
    unclip(&path, &["add", "/seed", "--o2m", "density=crowded"]);
    // A second branch mentions "crowded" in its title but carries no o2m.
    unclip(&path, &["add", "/room", "--title", "a crowded back room"]);

    let out = unclip(&path, &["suggest-o2m", "/room"]);
    assert!(out.status.success(), "suggest-o2m failed: {}", stderr(&out));
    assert!(stdout(&out).contains("density=crowded"));
}

/// `attach` adds references (type inferred from the value) and `refs` lists them.
#[test]
fn attach_and_list_refs() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    unclip(&path, &["add", "/branch"]);

    assert!(unclip(&path, &["attach", "/branch", "https://example.com"])
        .status
        .success());
    assert!(unclip(&path, &["attach", "/branch", "notes/local.md"])
        .status
        .success());

    let refs = stdout(&unclip(&path, &["refs", "/branch"]));
    // Inferred kinds: url for http(s), file otherwise.
    assert!(refs.contains("url\thttps://example.com"));
    assert!(refs.contains("file\tnotes/local.md"));
}

/// `import` upserts branches from a JSONL file (path-keyed), reported as
/// added vs. updated.
#[test]
fn import_upserts_from_file() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    unclip(&path, &["add", "/a"]); // pre-existing → will be updated

    let file = db.write(
        "branches.jsonl",
        "{\"path\":\"/a\",\"o2o\":{},\"o2m\":{}}\n{\"path\":\"/b\",\"o2o\":{},\"o2m\":{}}\n",
    );
    let out = unclip(&path, &["import", file.to_str().unwrap()]);
    assert!(out.status.success(), "import failed: {}", stderr(&out));
    let report = stdout(&out);
    assert!(report.contains("1 added"));
    assert!(report.contains("1 updated"));
    // Both paths are now present.
    assert!(unclip(&path, &["show", "/b"]).status.success());
}

/// `ls` and `tree` surface path segments that only exist as scopes of deeper
/// branches, marked with a trailing slash, so the hierarchy can be walked
/// top-down without knowing full paths.
#[test]
fn ls_and_tree_surface_scope_only_segments() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    // `/a` and `/a/b` are never added as branches — they exist only as scopes.
    assert!(unclip(&path, &["add", "/a/b/c", "--title", "C"])
        .status
        .success());
    assert!(unclip(&path, &["add", "/a/d", "--title", "D"])
        .status
        .success());

    // `ls /` lists the scope-only top-level segment.
    let ls_root = unclip(&path, &["ls", "/"]);
    assert!(ls_root.status.success());
    assert_eq!(stdout(&ls_root), "/a/\n");

    // Under `/a`: one scope-only child, one branch child.
    let ls = unclip(&path, &["ls", "/a"]);
    assert!(ls.status.success());
    assert_eq!(stdout(&ls), "/a/b/\n/a/d\tD\n");

    // `tree` gives the scope-only segment its own row, so `c` nests under
    // `b/` instead of appearing to be a sibling of `d`.
    let tree = unclip(&path, &["tree", "/a"]);
    assert!(tree.status.success());
    assert_eq!(stdout(&tree), "  b/\n    c\tC\n  d\tD\n");

    // The empty-scope message names the root instead of an empty string.
    let empty = unclip(&path, &["ls", "/nothing"]);
    assert!(stderr(&empty).contains("(no children under /nothing)"));
    let fresh = TempDb::new();
    let fresh_path = fresh.path();
    assert!(unclip(&fresh_path, &["init"]).status.success());
    let empty_root = unclip(&fresh_path, &["ls", "/"]);
    assert!(stderr(&empty_root).contains("(no children under /)"));
}

/// `tree` renders the scope's own branch alongside its subtree; `ls` lists
/// only what is under the scope.
#[test]
fn tree_includes_the_scope_branch_itself() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    assert!(unclip(&path, &["add", "/a", "--title", "A"])
        .status
        .success());
    assert!(unclip(&path, &["add", "/a/b", "--title", "B"])
        .status
        .success());

    let tree = unclip(&path, &["tree", "/a"]);
    assert!(tree.status.success());
    assert_eq!(stdout(&tree), "a\tA\n  b\tB\n");

    let ls = unclip(&path, &["ls", "/a"]);
    assert!(ls.status.success());
    assert_eq!(stdout(&ls), "/a/b\tB\n");
}

/// `ls` tolerates a trailing slash, and `stale` reports last-used timestamps.
#[test]
fn ls_accepts_trailing_slash_and_stale_reports_last_used() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    assert!(unclip(&path, &["add", "/a/b", "--title", "B"])
        .status
        .success());

    // `/a/` must list the same children as `/a`.
    let ls = unclip(&path, &["ls", "/a/"]);
    assert!(ls.status.success());
    assert!(stdout(&ls).contains("/a/b"), "got: {}", stdout(&ls));

    // Never-used branches report `last=-`.
    let stale = unclip(&path, &["stale", "--under", "/a"]);
    assert!(stale.status.success());
    assert!(
        stdout(&stale).contains("/a/b\tuses=0\tlast=-"),
        "got: {}",
        stdout(&stale)
    );

    // After a sample records usage, the timestamp appears.
    let sample = unclip(&path, &["sample", "--under", "/a", "--seed", "1"]);
    assert!(
        sample.status.success(),
        "sample failed: {}",
        stderr(&sample)
    );
    let stale = unclip(&path, &["stale", "--under", "/a"]);
    assert!(
        stdout(&stale).contains("uses=1\tlast=20"),
        "got: {}",
        stdout(&stale)
    );
}

/// `rm` refuses a subtree without --recursive and deletes it with it;
/// `rm-frame` deletes a stored frame. Both error on missing targets.
#[test]
fn rm_and_rm_frame_delete_explicitly() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    assert!(unclip(&path, &["add", "/a"]).status.success());
    // A gap descendant: /a/b does not exist, only /a/b/c.
    assert!(unclip(&path, &["add", "/a/b/c"]).status.success());

    // Missing target is an error.
    let out = unclip(&path, &["rm", "/nope"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("branch not found"));

    // A branch with descendants needs --recursive.
    let out = unclip(&path, &["rm", "/a"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--recursive"),
        "got: {}",
        stderr(&out)
    );

    let out = unclip(&path, &["rm", "/a", "--recursive"]);
    assert!(out.status.success(), "rm failed: {}", stderr(&out));
    assert!(stdout(&out).contains("deleted /a (2 branch(es))"));
    assert!(!unclip(&path, &["show", "/a/b/c"]).status.success());

    // A leaf deletes without --recursive.
    assert!(unclip(&path, &["add", "/leaf"]).status.success());
    assert!(unclip(&path, &["rm", "/leaf"]).status.success());
    assert!(!unclip(&path, &["show", "/leaf"]).status.success());

    // --recursive accepts a pure scope: a path that only exists through its
    // descendants. An empty scope is still an error.
    assert!(unclip(&path, &["add", "/scope/only/child"])
        .status
        .success());
    let out = unclip(&path, &["rm", "/scope", "--recursive"]);
    assert!(out.status.success(), "scope rm failed: {}", stderr(&out));
    assert!(stdout(&out).contains("deleted /scope (1 branch(es))"));
    let out = unclip(&path, &["rm", "/scope", "--recursive"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no branches under /scope"));

    // rm-frame deletes a stored frame and errors when it is missing.
    let frames = db.write(
        "frames.yaml",
        "frames:\n  story:\n    slots:\n      - name: place\n",
    );
    assert!(unclip(&path, &["import-frames", frames.to_str().unwrap()])
        .status
        .success());
    let out = unclip(&path, &["rm-frame", "story"]);
    assert!(out.status.success(), "rm-frame failed: {}", stderr(&out));
    assert!(stdout(&out).contains("deleted frame story"));
    let out = unclip(&path, &["rm-frame", "story"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("frame not found"));
}

/// `replay` re-runs a packet's recorded query with its seed and reproduces
/// the same selections, for both `sample` and `compose` packets.
#[test]
fn replay_reproduces_sample_and_compose_packets() {
    let db = TempDb::new();
    let path = db.path();
    assert!(unclip(&path, &["init"]).status.success());
    for branch in ["/r/one", "/r/two", "/r/three", "/r/four"] {
        assert!(unclip(&path, &["add", branch, "--o2o", "domain=story"])
            .status
            .success());
    }

    // Selected branch paths, in order, from a rendered packet.
    fn selected_paths(yaml: &str) -> Vec<String> {
        yaml.lines()
            .filter(|l| l.trim_start().starts_with("path: "))
            .map(|l| l.trim().to_string())
            .collect()
    }

    // Sample-packet replay.
    let sampled = unclip(
        &path,
        &[
            "sample",
            "--under",
            "/r",
            "--o2o",
            "domain=story",
            "--count",
            "2",
            "--seed",
            "42",
            "--dry-run",
        ],
    );
    assert!(sampled.status.success(), "sample: {}", stderr(&sampled));
    let packet = db.write("sample-packet.yaml", &stdout(&sampled));
    let replayed = unclip(&path, &["replay", packet.to_str().unwrap(), "--dry-run"]);
    assert!(replayed.status.success(), "replay: {}", stderr(&replayed));
    let original = selected_paths(&stdout(&sampled));
    assert_eq!(original.len(), 2);
    assert_eq!(selected_paths(&stdout(&replayed)), original);

    // Compose-packet replay, including an --under override recorded in
    // provenance.
    let frames = db.write(
        "frames.yaml",
        "frames:\n  story:\n    slots:\n      - name: place\n        require_o2o:\n          domain: story\n",
    );
    assert!(unclip(&path, &["import-frames", frames.to_str().unwrap()])
        .status
        .success());
    let composed = unclip(
        &path,
        &[
            "compose",
            "--frame",
            "story",
            "--under",
            "place:/r",
            "--seed",
            "7",
            "--dry-run",
        ],
    );
    assert!(composed.status.success(), "compose: {}", stderr(&composed));
    let packet = db.write("compose-packet.yaml", &stdout(&composed));
    let replayed = unclip(&path, &["replay", packet.to_str().unwrap(), "--dry-run"]);
    assert!(replayed.status.success(), "replay: {}", stderr(&replayed));
    assert_eq!(
        selected_paths(&stdout(&replayed)),
        selected_paths(&stdout(&composed))
    );

    // A --seed override takes precedence over the packet's seed.
    let reseeded = unclip(
        &path,
        &[
            "replay",
            packet.to_str().unwrap(),
            "--seed",
            "8",
            "--dry-run",
        ],
    );
    assert!(reseeded.status.success());
    assert!(stdout(&reseeded).contains("seed: 8"));
}

#[tokio::test]
async fn batch_measurement_cli_snapshots_inputs_and_detects_replay_mismatches() {
    use sea_orm::ConnectionTrait;
    use std::collections::BTreeMap;
    use unclip_domain::{
        DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame, Unit, UnitId, UnitKind,
    };
    use unclip_epistemic::{
        hash_params, CalculationToken, DependencyCollector, DerivedId, DomainVersion, EmitMetadata,
        FrameVersion, Operation, PluginId, Provenance, SourceRef, Timestamp, Tracked,
    };
    use unclip_observe::{
        Alignment, AlignmentCandidate, Observation, ObservationId, ObservedUnit, ObservedUnitId,
        PartialRanking, RankTier,
    };
    use unclip_store::{
        CandidateInterpretationRepository, CandidateRepository, DomainWriter, EngineRunRepository,
        MeasurementRepository, ObservationRepository, ProvenanceRepository,
    };

    let temp = TempDb::new();
    let path = temp.path();
    let db = unclip_store::connect_and_migrate(&format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    let domains = unclip_store::SeaOrmDomainRepository::new(db.clone());
    let observations = unclip_store::SeaOrmObservationRepository::new(db.clone());
    let provenance = unclip_store::SeaOrmProvenanceRepository::new(db.clone());
    let runs = unclip_store::SeaOrmEngineRunRepository::new(db.clone());
    let measurements = unclip_store::SeaOrmMeasurementRepository::new(db.clone());
    let domain = DomainSnapshot {
        id: DomainId::new("batch"),
        version: DomainVersion::new("1"),
        relations: BTreeMap::new(),
        units: ["a", "b"]
            .map(|id| {
                (
                    UnitId::new(id),
                    Unit {
                        id: UnitId::new(id),
                        kind: UnitKind::AtomicMeaning,
                        label: None,
                        properties: BTreeMap::new(),
                    },
                )
            })
            .into_iter()
            .collect(),
    };
    let frame = MeasurementFrame {
        id: FrameId::new("batch.general"),
        version: FrameVersion::new("1"),
        axes: ["a", "b"]
            .map(|id| FrameAxis {
                unit: UnitId::new(id),
                label: None,
            })
            .into(),
    };
    domains.insert_domain_version(domain.clone()).await.unwrap();
    domains
        .insert_measurement_frame(&domain.id, &domain.version, frame)
        .await
        .unwrap();
    for (id, order) in [
        ("obs-1", ["a", "b"]),
        ("obs-2", ["b", "a"]),
        ("unselected", ["a", "b"]),
    ] {
        let derived_id = DerivedId::new(format!("inferred/{id}"));
        provenance
            .insert_provenance(unclip_store::StoredProvenance {
                id: derived_id.clone(),
                run_id: None,
                provenance: Provenance {
                    operation: Operation::Inferred,
                    producer: PluginId::new("infer.fixture"),
                    algorithm: "fixture".into(),
                    version: "0.1.0".parse().unwrap(),
                    params: serde_json::json!({}),
                    params_hash: hash_params(&serde_json::json!({})),
                    inputs: vec![],
                    source: Some(SourceRef::new("fixture")),
                    timestamp: Timestamp::new("then"),
                    domain_version: Some(domain.version.clone()),
                    frame_version: None,
                    model: None,
                },
            })
            .await
            .unwrap();
        let observation = Observation {
            id: ObservationId::new(id),
            source: SourceRef::new("fixture"),
            observed_at: None,
            relations: vec![],
            context: BTreeMap::new(),
            units: ["a", "b"]
                .map(|id| ObservedUnit {
                    id: ObservedUnitId::new(id),
                    label: id.into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                })
                .into(),
        };
        observations
            .insert_observation(observation.clone(), &derived_id)
            .await
            .unwrap();
        observations
            .insert_alignment(
                &format!("align/{id}"),
                Alignment {
                    observation: observation.id.clone(),
                    candidates: ["a", "b"]
                        .map(|id| AlignmentCandidate {
                            observed: ObservedUnitId::new(id),
                            domain: UnitId::new(id),
                            confidence: 1.0,
                            evidence: vec![],
                        })
                        .into(),
                },
                &domain.id,
                &domain.version,
                &derived_id,
            )
            .await
            .unwrap();
        observations
            .insert_ranking(
                &format!("rank/{id}"),
                PartialRanking {
                    observation: observation.id,
                    tiers: order
                        .map(|id| RankTier {
                            units: vec![ObservedUnitId::new(id)],
                        })
                        .into(),
                    unknown: vec![],
                },
                &derived_id,
            )
            .await
            .unwrap();
    }
    let profile = temp.write("batch-engine.json", &serde_json::json!({
        "domain":"batch@1", "frame":"batch.general@1", "sensors":[
            {"id":"sensor.trajectories"}, {"id":"sensor.spearman"}, {"id":"sensor.mutual-information"}
        ]
    }).to_string());
    let discovery_profile = temp.write(
        "discovery-engine.json",
        &serde_json::json!({
            "domain":"batch@1", "frame":"batch.general@1", "candidate_generators":[
                {"id":"generate.persistent-residual", "params":{"minimum_observations":2}}
            ]
        })
        .to_string(),
    );
    let unsupported = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-1",
            "obs-2",
            "--profile",
            discovery_profile.to_str().unwrap(),
        ],
    );
    assert!(!unsupported.status.success());
    assert!(stderr(&unsupported).contains("do not execute candidate generators"));
    let comparator_profile=temp.write("comparator-engine.json",&serde_json::json!({"domain":"batch@1","frame":"batch.general@1","comparators":[{"id":"compare.scalar-difference"}]}).to_string());
    let unsupported = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-1",
            "obs-2",
            "--profile",
            comparator_profile.to_str().unwrap(),
        ],
    );
    assert!(!unsupported.status.success());
    assert!(stderr(&unsupported).contains("or comparators"));

    let no_selection = unclip(
        &path,
        &["level", "measure", "--profile", profile.to_str().unwrap()],
    );
    assert!(!no_selection.status.success());
    let duplicate = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-1",
            "obs-1",
            "--profile",
            profile.to_str().unwrap(),
        ],
    );
    assert!(!duplicate.status.success());
    assert!(stderr(&duplicate).contains("must be unique"));
    let missing = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-1",
            "missing",
            "--profile",
            profile.to_str().unwrap(),
        ],
    );
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("observation not found"));
    let measured = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-2",
            "obs-1",
            "--profile",
            profile.to_str().unwrap(),
        ],
    );
    assert!(measured.status.success(), "{}", stderr(&measured));
    let measured_text = stdout(&measured);
    let profile_id = measured_text
        .lines()
        .find_map(|line| line.strip_prefix("PROFILE\tCALCULATED\t"))
        .unwrap();
    let run_id = profile_id.strip_suffix("/profile").unwrap();
    let replay = runs.replay_run(run_id).await.unwrap().unwrap();
    assert_eq!(
        replay
            .observations
            .iter()
            .map(|record| record.value.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["obs-2", "obs-1"]
    );
    assert_eq!(replay.alignments.len(), 2);
    assert_eq!(replay.rankings.len(), 2);
    assert_eq!(replay.run.metadata["domain"], "batch@1");
    assert_eq!(replay.run.metadata["frame"], "batch.general@1");
    let stored = measurements.get_profile(profile_id).await.unwrap().unwrap();
    assert_eq!(stored.measurements.len(), 3);
    let verified = unclip(&path, &["level", "verify", run_id]);
    assert!(verified.status.success(), "{}", stderr(&verified));
    assert!(stdout(&verified).contains("observations=2 alignments=2 rankings=2 calculated=3"));
    let table = unclip(&path, &["level", "profile", profile_id, "--table"]);
    assert!(table.status.success());
    assert!(stdout(&table).contains("sensor.spearman"));
    assert!(stdout(&table).contains("sensor.mutual-information"));
    // Select matrices from two separately stored profiles and preserve each source.
    let second = unclip(
        &path,
        &[
            "level",
            "measure",
            "obs-1",
            "obs-2",
            "--profile",
            profile.to_str().unwrap(),
        ],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    let second_text = stdout(&second);
    let second_profile = second_text
        .lines()
        .find_map(|line| line.strip_prefix("PROFILE\tCALCULATED\t"))
        .unwrap();
    let config = temp.write(
        "communities.yaml",
        "method: communities\nthreshold: 0.5\nminimum_samples: 2\n",
    );
    let duplicate = unclip(
        &path,
        &[
            "level",
            "derive",
            profile_id,
            profile_id,
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert!(!duplicate.status.success());
    assert!(stderr(&duplicate).contains("duplicate profile"));
    let missing = unclip(
        &path,
        &[
            "level",
            "derive",
            "absent",
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert!(!missing.status.success());
    for invalid in [
        "method: communities\nthreshold: 0.5\nminimum_samples: 0\n",
        "method: communities\nthreshold: 0.5\nminimum_samples: 2\nlabel: invented\n",
        "method: spectral\nminimum_samples: 2\ntolerance: 0\nmax_sweeps: 100\n",
    ] {
        let invalid_config = temp.write("invalid-empirical.yaml", invalid);
        let rejected = unclip(
            &path,
            &[
                "level",
                "derive",
                profile_id,
                "--config",
                invalid_config.to_str().unwrap(),
            ],
        );
        assert!(!rejected.status.success());
        assert!(!stdout(&rejected).contains("CALCULATED"));
    }
    let mut community_run = String::new();
    let mut community_structure = String::new();
    let mut unrelated_structure = String::new();
    let mut spectral_run = String::new();
    for method in ["communities", "spectral", "sparse"] {
        let config = match method {
            "communities" => config.clone(),
            "spectral" => temp.write("spectral.yaml", "method: spectral\nminimum_samples: 2\ntolerance: 0.000000000001\nmax_sweeps: 100\n"),
            _ => temp.write("sparse.yaml", "method: communities\nthreshold: 0.5\nminimum_samples: 3\n"),
        };
        let derived = unclip(
            &path,
            &[
                "level",
                "derive",
                profile_id,
                second_profile,
                "--config",
                config.to_str().unwrap(),
            ],
        );
        assert!(derived.status.success(), "{}", stderr(&derived));
        let text = stdout(&derived);
        let empirical_run = text
            .lines()
            .find_map(|line| line.strip_prefix("CALCULATED\tEMPIRICAL\trun="))
            .unwrap();
        let checked = unclip(&path, &["level", "verify", empirical_run]);
        assert!(checked.status.success(), "{}", stderr(&checked));
        assert!(stdout(&checked).contains(if method == "sparse" {
            "calculated=0"
        } else {
            "calculated=4"
        }));
        if method == "sparse" {
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("INSUFFICIENT_EVIDENCE"))
                    .count(),
                4
            );
        } else {
            let ids = text
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("STRUCTURE\t")
                        .and_then(|line| line.split_once("\tsource="))
                })
                .collect::<Vec<_>>();
            assert_eq!(ids.len(), 4);
            if method == "communities" {
                community_structure = ids[0].0.to_owned();
                unrelated_structure = ids[1].0.to_owned();
            }
            for (id, source) in ids {
                let shown = unclip(&path, &["level", "structure", id, "--format", "json"]);
                assert!(shown.status.success(), "{}", stderr(&shown));
                let value: serde_json::Value = serde_json::from_str(&stdout(&shown)).unwrap();
                assert_eq!(value["kind"], method);
                assert!(value["value"].get("label").is_none());
                assert_eq!(
                    provenance.direct_inputs(&DerivedId::new(id)).await.unwrap(),
                    vec![DerivedId::new(source)]
                );
            }
        }
        if method == "communities" {
            community_run = empirical_run.into();
        }
        if method == "spectral" {
            spectral_run = empirical_run.into();
        }
    }
    let candidate_id = DerivedId::new("interpret-candidate");
    let dependencies = DependencyCollector::default();
    dependencies.read(&Tracked::from_recorded(
        DerivedId::new(&community_structure),
        (),
    ));
    let candidate_params = serde_json::json!({"fixture":"interpretation"});
    let candidate = CalculationToken::from_harness(
        EmitMetadata {
            id: candidate_id.clone(),
            producer: PluginId::new("generate.fixture"),
            algorithm: "generate.fixture".into(),
            version: "0.1.0".parse().unwrap(),
            params_hash: hash_params(&candidate_params),
            params: candidate_params,
            source: None,
            timestamp: Timestamp::new("2026-09-23T00:00:00Z"),
            domain_version: Some(domain.version.clone()),
            frame_version: None,
            model: None,
        },
        dependencies,
    )
    .emit(unclip_store::CandidateProposal {
        domain_version_id: serde_json::to_string(&("batch", "1")).unwrap(),
        kind: unclip_store::CandidateKind::CompositeMeaning,
        value: serde_json::json!({"source_structure":community_structure})
            .as_object()
            .unwrap()
            .clone(),
    });
    let experiments = unclip_store::SeaOrmExperimentRepository::new(db.clone());
    experiments.insert_candidate(None, candidate).await.unwrap();
    let interpretation_profile = temp.write(
        "interpretation-profile.json",
        &serde_json::json!({
            "domain":"batch@1",
            "interpreters":[{
                "id":"interpret.llm-label",
                "params":{
                    "model":"fixture/semantic-labeler",
                    "model_version":"2026-09-23",
                    "generation":{"temperature":0}
                }
            }]
        })
        .to_string(),
    );
    let interpretation_response = temp.write(
        "interpretation-response.json",
        &serde_json::json!({
            "candidate":candidate_id,
            "structure":community_structure,
            "model":"fixture/semantic-labeler",
            "model_version":"2026-09-23",
            "response":{
                "label":"alternating pair",
                "explanation":"the anonymous community retains the two measured units"
            }
        })
        .to_string(),
    );
    let unrelated_response = temp.write(
        "unrelated-interpretation-response.json",
        &serde_json::json!({
            "candidate":candidate_id,
            "structure":unrelated_structure,
            "model":"fixture/semantic-labeler",
            "model_version":"2026-09-23",
            "response":{"label":"wrong source","explanation":"must be rejected"}
        })
        .to_string(),
    );
    let unrelated = unclip(
        &path,
        &[
            "level",
            "interpret",
            &candidate_id.0,
            "--structure",
            &unrelated_structure,
            "--profile",
            interpretation_profile.to_str().unwrap(),
            "--response",
            unrelated_response.to_str().unwrap(),
        ],
    );
    assert!(!unrelated.status.success());
    assert!(stderr(&unrelated).contains("is not provenance evidence for candidate"));
    let interpreted = unclip(
        &path,
        &[
            "level",
            "interpret",
            &candidate_id.0,
            "--structure",
            &community_structure,
            "--profile",
            interpretation_profile.to_str().unwrap(),
            "--response",
            interpretation_response.to_str().unwrap(),
        ],
    );
    assert!(
        interpreted.status.success(),
        "interpretation failed: {}",
        stderr(&interpreted)
    );
    let interpretation_output = stdout(&interpreted);
    let interpretation_run = interpretation_output
        .lines()
        .find_map(|line| line.strip_prefix("INTERPRETED\tLABEL\trun="))
        .expect("interpretation output should identify its run");
    let (interpretation_id, value) = interpretation_output
        .lines()
        .find_map(|line| {
            line.strip_prefix("INTERPRETATION\t")
                .and_then(|line| line.split_once('\t'))
        })
        .expect("interpretation output should include its stored value");
    let value: serde_json::Value = serde_json::from_str(value).unwrap();
    assert_eq!(value["interpretation"]["label"], "alternating pair");
    assert_eq!(value["structure"]["kind"], "communities");
    let stored = experiments
        .get_candidate_interpretation(&DerivedId::new(interpretation_id))
        .await
        .unwrap()
        .expect("candidate interpretation should be persisted");
    assert_eq!(stored.candidate_id, candidate_id);
    assert_eq!(stored.value, value);
    let interpreted_provenance = provenance
        .get_provenance(&stored.id)
        .await
        .unwrap()
        .expect("interpretation provenance should be persisted");
    assert_eq!(
        interpreted_provenance.provenance.operation,
        Operation::Interpreted
    );
    assert_eq!(
        interpreted_provenance.provenance.model,
        Some(unclip_epistemic::ModelRef::versioned(
            "fixture/semantic-labeler",
            "2026-09-23"
        ))
    );
    assert_eq!(
        runs.get_run(interpretation_run)
            .await
            .unwrap()
            .expect("interpretation run should be persisted")
            .status,
        unclip_store::EngineRunStatus::Completed
    );
    db.execute_unprepared(
        "UPDATE empirical_structures SET value_json = '{}' WHERE kind = 'communities'",
    )
    .await
    .unwrap();
    let corrupt = unclip(&path, &["level", "verify", &community_run]);
    assert!(!corrupt.status.success());
    assert!(stderr(&corrupt).contains("differs from stored result"));
    assert!(!stdout(&corrupt).contains("VERIFIED"));
    db.execute_unprepared(
        "DELETE FROM provenance_inputs WHERE derived_id LIKE '%empirical.spectral%'",
    )
    .await
    .unwrap();
    let corrupt = unclip(&path, &["level", "verify", &spectral_run]);
    assert!(!corrupt.status.success());
    assert!(stderr(&corrupt).contains("provenance differs"));
    // A newly persisted alternative ranking must not change the recorded selection.
    observations
        .insert_ranking(
            "rank/later",
            PartialRanking {
                observation: ObservationId::new("obs-1"),
                tiers: ["b", "a"]
                    .map(|id| RankTier {
                        units: vec![ObservedUnitId::new(id)],
                    })
                    .into(),
                unknown: vec![],
            },
            &DerivedId::new("inferred/obs-1"),
        )
        .await
        .unwrap();
    let verified_again = unclip(&path, &["level", "verify", run_id]);
    assert!(
        verified_again.status.success(),
        "{}",
        stderr(&verified_again)
    );
    assert_eq!(stdout(&verified_again), stdout(&verified));
    assert_eq!(
        runs.replay_run(run_id)
            .await
            .unwrap()
            .unwrap()
            .rankings
            .len(),
        2
    );
    // Alter a stored measurement to prove verify compares results, not just reruns.
    db.execute_unprepared(
        "UPDATE measurements SET sample_count = 999 WHERE id LIKE '%sensor.spearman%'",
    )
    .await
    .unwrap();
    let mismatched = unclip(&path, &["level", "verify", run_id]);
    assert!(!mismatched.status.success());
    assert!(stderr(&mismatched).contains("differ from stored profiles"));
    assert!(!stdout(&mismatched).contains("VERIFIED"));
    db.execute_unprepared(
        "UPDATE measurements SET sample_count = 2 WHERE id LIKE '%sensor.spearman%'",
    )
    .await
    .unwrap();
    db.execute_unprepared(
        "DELETE FROM provenance_inputs WHERE derived_id LIKE '%sensor.spearman%'",
    )
    .await
    .unwrap();
    let mismatched_provenance = unclip(&path, &["level", "verify", run_id]);
    assert!(!mismatched_provenance.status.success());
    assert!(stderr(&mismatched_provenance).contains("provenance differs"));
}
