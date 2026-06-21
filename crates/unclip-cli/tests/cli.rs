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

    let out = unclip(&path, &["add", "/a", "--o2o", "axis=place", "--o2o", "axis=time"]);
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
        &["query", "--avoid-o2o", "axis=place", "--avoid-o2o", "axis=time"],
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
        &["sample", "--count", "3", "--seed", "42", "--format", "jsonl", "--dry-run"],
    );
    let two = unclip(
        &path,
        &["sample", "--count", "3", "--seed", "42", "--format", "jsonl", "--dry-run"],
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
    assert!(unclip(&path, &["sample", "--count", "1", "--seed", "1"]).status.success());
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
