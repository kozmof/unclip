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
}

/// A command other than `init` must refuse to silently create a fresh database.
#[test]
fn non_init_requires_existing_db() {
    let db = TempDb::new();
    let out = unclip(&db.path(), &["show", "/x"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("database not found"));
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

/// `query --avoid-o2o` rejects a repeated name too (consistent with require),
/// rather than silently keeping only the last value.
#[test]
fn duplicate_avoid_o2o_name_on_query_is_rejected() {
    let db = TempDb::new();
    let path = db.path();
    unclip(&path, &["init"]);
    unclip(&path, &["add", "/a"]);

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
    assert!(!out.status.success());
    assert!(stderr(&out).contains("duplicate o2o name"));
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
