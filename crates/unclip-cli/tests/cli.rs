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
    assert!(plugins.contains("sensor.residual"));
    assert!(plugins.contains("sensor.permutation"));
    assert!(plugins.contains("sensor.lehmer"));
    assert!(plugins.contains("sensor.kendall"));
    assert!(plugins.contains("sensor.rbo"));
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

    let profile_jsonl = unclip(
        &path,
        &["level", "profile", profile_id, "--format", "jsonl"],
    );
    assert!(!profile_jsonl.status.success());
    assert!(stderr(&profile_jsonl).contains("JSONL is not supported"));

    let connection = unclip_store::connect(&format!("sqlite://{}?mode=rw", path.display()))
        .await
        .unwrap();
    let measurements = unclip_store::SeaOrmMeasurementRepository::new(connection);
    let stored = unclip_store::MeasurementRepository::get_profile(&measurements, profile_id)
        .await
        .unwrap()
        .expect("measurement profile should be persisted");
    assert_eq!(stored.measurements.len(), 1);
    assert_eq!(stored.measurements[0].sensor.0, "sensor.coverage");

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
    assert!(stdout(&out).contains("verify"));
    let verify_help = unclip(&db.path(), &["level", "verify", "--help"]);
    assert!(verify_help.status.success());
    assert!(stdout(&verify_help).contains("Replay persisted inference"));
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
