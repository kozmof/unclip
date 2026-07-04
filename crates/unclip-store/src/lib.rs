//! unclip-store — repository traits and SeaORM-backed persistence.

#![forbid(unsafe_code)]

mod error;
mod frame_mapper;
mod frame_repository;
mod history;
mod mapper;
mod pattern_repository;
mod repository;
mod seaorm;
mod sqlite_limits;

pub use error::{StoreError, StoreResult};
pub use frame_repository::{FrameInfo, FrameRepository, SeaOrmFrameRepository};
pub use history::{
    now, HistoryRepository, PacketRecord, PacketUsageRecord, SeaOrmHistoryRepository, UsageSummary,
};
pub use pattern_repository::{SeaOrmPatternRepository, StoredPattern};
pub use repository::{
    BranchRepository, BranchRepositoryError, BranchRepositoryResult, IndexedValue,
    SeaOrmBranchRepository,
};
pub use seaorm::{
    connect, connect_and_migrate, connect_and_migrate_with_options, connect_with_options,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use unclip_core::{Branch, Frame, Reference, SampleQuery, Slot};

    async fn repo() -> SeaOrmBranchRepository {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        SeaOrmBranchRepository::new(db)
    }

    fn sample_branch() -> Branch {
        let mut b = Branch::new("/ikebukuro/station/coin-locker");
        b.title = Some("Coin Locker Area".into());
        b.description = Some("A small coin locker area.".into());
        b.o2o.insert("domain".into(), "story".into());
        b.o2o.insert("axis".into(), "place".into());
        b.o2o.insert("use".into(), "scene-anchor".into());
        // o2m is a set; values are stored/returned in sorted order.
        b.o2m
            .insert("mood".into(), vec!["hidden".into(), "tense".into()]);
        b.o2m
            .insert("topic".into(), vec!["locker".into(), "transit".into()]);
        b.weight = 1.5;
        b.metadata = json!({ "affordances": ["a key can be exchanged by mistake"] });
        b.references = vec![Reference {
            kind: "file".into(),
            value: "refs/locker.jpg".into(),
            note: None,
        }];
        b
    }

    #[tokio::test]
    async fn rejects_invalid_branch_state_at_repository_boundary() {
        let repo = repo().await;
        let mut branch = Branch::new("/bad");
        branch.weight = f64::NAN;

        let err = repo.add(branch).await.unwrap_err().to_string();
        assert!(err.contains("weight must be finite"), "got: {err}");
    }

    #[tokio::test]
    async fn add_get_update_delete_roundtrip() {
        let repo = repo().await;
        let branch = sample_branch();
        repo.add(branch.clone()).await.unwrap();

        // get round-trips o2o/o2m/metadata/references (ignoring assigned id).
        let got = repo.get(&branch.path).await.unwrap().unwrap();
        assert!(got.id.is_some());
        assert!(got.revision.is_some());
        let mut comparable = got.clone();
        comparable.id = None;
        comparable.revision = None;
        assert_eq!(comparable, branch);

        // update mutates indexed values.
        let mut edited = got.clone();
        edited.o2m.insert("mood".into(), vec!["urgent".into()]);
        edited.title = Some("Renamed".into());
        repo.update(edited.clone()).await.unwrap();

        let mut after = repo.get(&branch.path).await.unwrap().unwrap();
        after.id = None;
        assert_eq!(after.title.as_deref(), Some("Renamed"));
        assert_eq!(after.o2m.get("mood").unwrap(), &vec!["urgent".to_string()]);

        // delete removes the branch and its child rows.
        repo.delete(&branch.path).await.unwrap();
        assert!(repo.get(&branch.path).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn stale_branch_update_is_rejected() {
        let repo = repo().await;
        repo.add(Branch::new("/concurrent")).await.unwrap();

        let mut first = repo.get("/concurrent").await.unwrap().unwrap();
        let mut stale = first.clone();
        first.title = Some("first writer".into());
        repo.update(first).await.unwrap();

        stale.description = Some("stale writer".into());
        let error = repo.update(stale).await.unwrap_err();
        assert!(
            matches!(
                &error,
                BranchRepositoryError::Conflict { path } if path == "/concurrent"
            ),
            "got: {error}"
        );

        let stored = repo.get("/concurrent").await.unwrap().unwrap();
        assert_eq!(stored.title.as_deref(), Some("first writer"));
        assert!(stored.description.is_none());
    }

    #[tokio::test]
    async fn reference_attachment_invalidates_stale_edits_across_connections() {
        let path = std::env::temp_dir().join(format!(
            "unclip-reference-concurrency-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = || {
            let mut options = sea_orm::ConnectOptions::new("sqlite://unclip-placeholder?mode=rwc");
            let filename = path.clone();
            options.map_sqlx_sqlite_opts(move |sqlite| sqlite.filename(&filename));
            options
        };

        let db_a = connect_and_migrate_with_options(options()).await.unwrap();
        let db_b = connect_and_migrate_with_options(options()).await.unwrap();
        let repo_a = SeaOrmBranchRepository::new(db_a);
        let repo_b = SeaOrmBranchRepository::new(db_b);

        repo_a.add(Branch::new("/shared")).await.unwrap();
        let mut stale = repo_a.get("/shared").await.unwrap().unwrap();
        let old_revision = stale.revision.clone();

        repo_b
            .attach_reference(
                "/shared",
                &Reference {
                    kind: "url".into(),
                    value: "https://example.test/reference".into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        let attached = repo_b.get("/shared").await.unwrap().unwrap();
        assert_ne!(attached.revision, old_revision);

        stale.description = Some("stale replacement".into());
        let error = repo_a.update(stale).await.unwrap_err();
        assert!(matches!(
            &error,
            BranchRepositoryError::Conflict { path } if path == "/shared"
        ));

        let stored = repo_a.get("/shared").await.unwrap().unwrap();
        assert!(stored.description.is_none());
        assert_eq!(stored.references, attached.references);

        drop(repo_a);
        drop(repo_b);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn duplicate_o2m_values_do_not_violate_pk() {
        // A branch built with repeated o2m values (e.g. from an import file)
        // must not collide on the (branch_id, name, value) primary key.
        let repo = repo().await;
        let mut b = Branch::new("/dup");
        b.o2m.insert(
            "topic".into(),
            vec!["locker".into(), "locker".into(), "transit".into()],
        );
        repo.add(b).await.unwrap();

        let got = repo.get("/dup").await.unwrap().unwrap();
        assert_eq!(
            got.o2m.get("topic").unwrap(),
            &vec!["locker".to_string(), "transit".to_string()]
        );
    }

    #[tokio::test]
    async fn hydrates_archives_across_sqlite_parameter_chunks() {
        let repo = repo().await;
        let branches = (0..501)
            .map(|n| {
                let mut branch = Branch::new(format!("/bulk/{n:03}"));
                branch.o2o.insert("axis".into(), "place".into());
                branch.o2m.insert("tag".into(), vec![format!("tag-{n}")]);
                branch.references.push(Reference {
                    kind: "url".into(),
                    value: format!("https://example.test/{n}"),
                    note: None,
                });
                branch
            })
            .collect();

        repo.upsert_many(branches).await.unwrap();
        let loaded = repo.find(SampleQuery::default()).await.unwrap();

        assert_eq!(loaded.len(), 501);
        assert!(loaded.iter().all(|branch| {
            branch.o2o.get("axis").map(String::as_str) == Some("place")
                && branch.o2m.contains_key("tag")
                && branch.references.len() == 1
        }));
    }

    #[tokio::test]
    async fn persists_large_child_sets_in_sqlite_safe_batches() {
        let repo = repo().await;
        let mut branch = Branch::new("/many-children");
        for n in 0..400 {
            branch.o2o.insert(format!("o2o-{n}"), format!("value-{n}"));
            branch
                .o2m
                .insert(format!("o2m-{n}"), vec![format!("value-{n}")]);
            branch.references.push(Reference {
                kind: "url".into(),
                value: format!("https://example.test/{n}"),
                note: Some(format!("reference {n}")),
            });
        }

        repo.add(branch).await.unwrap();
        let loaded = repo.get("/many-children").await.unwrap().unwrap();
        assert_eq!(loaded.o2o.len(), 400);
        assert_eq!(loaded.o2m.len(), 400);
        assert_eq!(loaded.references.len(), 400);
    }

    #[tokio::test]
    async fn navigation_and_find() {
        let repo = repo().await;
        for path in [
            "/ikebukuro",
            "/ikebukuro/station",
            "/ikebukuro/station/exit",
            "/ueno",
        ] {
            let mut b = Branch::new(path);
            b.o2o.insert("domain".into(), "story".into());
            if path.ends_with("exit") {
                b.o2o.insert("axis".into(), "place".into());
            }
            repo.add(b).await.unwrap();
        }

        let children = repo.children("/ikebukuro").await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "/ikebukuro/station");

        let descendants = repo.descendants("/ikebukuro").await.unwrap();
        assert_eq!(descendants.len(), 2);

        let ancestors = repo.ancestors("/ikebukuro/station/exit").await.unwrap();
        let mut ancestor_paths: Vec<_> = ancestors.iter().map(|b| b.path.clone()).collect();
        ancestor_paths.sort();
        assert_eq!(ancestor_paths, vec!["/ikebukuro", "/ikebukuro/station"]);

        // find: scope + required o2o.
        let mut q = SampleQuery {
            under: Some("/ikebukuro".into()),
            ..Default::default()
        };
        q.require_o2o.insert("axis".into(), "place".into());
        let found = repo.find(q).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/ikebukuro/station/exit");
    }

    #[tokio::test]
    async fn find_returns_paths_in_stable_order() {
        let repo = repo().await;
        for path in ["/z", "/a", "/m"] {
            repo.add(Branch::new(path)).await.unwrap();
        }

        let paths: Vec<_> = repo
            .find(SampleQuery::default())
            .await
            .unwrap()
            .into_iter()
            .map(|branch| branch.path)
            .collect();
        assert_eq!(paths, vec!["/a", "/m", "/z"]);
    }

    #[tokio::test]
    async fn find_applies_avoid_o2o_and_o2m_in_sql() {
        let repo = repo().await;

        // /keep: plain. /skip_o2o: excluded by avoid_o2o. /skip_o2m: excluded
        // by avoid_o2m.
        repo.add(Branch::new("/keep")).await.unwrap();

        let mut skip_o2o = Branch::new("/skip-o2o");
        skip_o2o.o2o.insert("mood".into(), "tense".into());
        repo.add(skip_o2o).await.unwrap();

        let mut skip_o2m = Branch::new("/skip-o2m");
        skip_o2m
            .o2m
            .insert("topic".into(), vec!["cafe".into(), "transit".into()]);
        repo.add(skip_o2m).await.unwrap();

        let mut q = SampleQuery::default();
        q.avoid_o2o.insert("mood".into(), "tense".into());
        q.avoid_o2m.insert("topic".into(), vec!["cafe".into()]);

        let found: Vec<_> = repo
            .find(q)
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.path)
            .collect();
        assert_eq!(found, vec!["/keep".to_string()]);
    }

    #[tokio::test]
    async fn find_applies_require_o2m_in_sql() {
        let repo = repo().await;

        // /both carries every required value; /partial is missing one; /none has
        // neither. require_o2m must keep only /both.
        let mut both = Branch::new("/both");
        both.o2m
            .insert("mood".into(), vec!["tense".into(), "hidden".into()]);
        repo.add(both).await.unwrap();

        let mut partial = Branch::new("/partial");
        partial.o2m.insert("mood".into(), vec!["tense".into()]);
        repo.add(partial).await.unwrap();

        repo.add(Branch::new("/none")).await.unwrap();

        let mut q = SampleQuery::default();
        q.require_o2m
            .insert("mood".into(), vec!["tense".into(), "hidden".into()]);

        let found: Vec<_> = repo
            .find(q)
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.path)
            .collect();
        assert_eq!(found, vec!["/both".to_string()]);
    }

    #[tokio::test]
    async fn titles_projects_path_and_title_only() {
        let repo = repo().await;
        let mut titled = Branch::new("/a");
        titled.title = Some("Alpha".into());
        repo.add(titled).await.unwrap();
        repo.add(Branch::new("/b")).await.unwrap(); // no title

        let titles = repo.titles().await.unwrap();
        assert_eq!(titles, vec![("/a".to_string(), "Alpha".to_string())]);
    }

    #[tokio::test]
    async fn scope_matching_treats_underscore_literally() {
        // `_` is a SQL LIKE wildcard; a scope like `/a_b` must not match `/axb`.
        let repo = repo().await;
        for path in ["/a_b", "/a_b/child", "/axb", "/axb/child"] {
            repo.add(Branch::new(path)).await.unwrap();
        }

        let mut descendants: Vec<_> = repo
            .descendants("/a_b")
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.path)
            .collect();
        descendants.sort();
        assert_eq!(descendants, vec!["/a_b/child"]);

        // `find` applies the same scope filter (self + descendants).
        let q = SampleQuery {
            under: Some("/a_b".into()),
            ..Default::default()
        };
        let mut found: Vec<_> = repo
            .find(q)
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.path)
            .collect();
        found.sort();
        assert_eq!(found, vec!["/a_b", "/a_b/child"]);
    }

    #[tokio::test]
    async fn catalog_and_value_lookup() {
        let repo = repo().await;
        for (path, axis) in [("/a", "place"), ("/b", "place"), ("/c", "time")] {
            let mut br = Branch::new(path);
            br.o2o.insert("domain".into(), "story".into());
            br.o2o.insert("axis".into(), axis.into());
            br.o2m.insert("topic".into(), vec!["transit".into()]);
            repo.add(br).await.unwrap();
        }

        // Full o2o catalog: domain=story(3), axis=place(2), axis=time(1).
        let catalog = repo.o2o_catalog(None).await.unwrap();
        let axis_place = catalog
            .iter()
            .find(|v| v.name == "axis" && v.value == "place")
            .unwrap();
        assert_eq!(axis_place.count, 2);
        let domain = catalog
            .iter()
            .find(|v| v.name == "domain" && v.value == "story")
            .unwrap();
        assert_eq!(domain.count, 3);

        // Single-name catalog.
        let axis_values = repo.o2o_catalog(Some("axis")).await.unwrap();
        assert_eq!(axis_values.len(), 2);

        // o2m catalog.
        let o2m = repo.o2m_catalog(None).await.unwrap();
        assert_eq!(o2m.len(), 1);
        assert_eq!(o2m[0].count, 3);

        // Branch lookup by value.
        let with_place = repo.branches_with_o2o("axis", "place").await.unwrap();
        assert_eq!(with_place.len(), 2);
        let with_transit = repo.branches_with_o2m("topic", "transit").await.unwrap();
        assert_eq!(with_transit.len(), 3);
    }

    #[tokio::test]
    async fn frame_save_get_list_roundtrip() {
        use frame_repository::{FrameRepository, SeaOrmFrameRepository};
        use unclip_core::{Frame, Slot};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);

        let place = Slot {
            name: "place".into(),
            under: Some("/ikebukuro".into()),
            require_o2o: [
                ("domain".to_string(), "story".to_string()),
                ("axis".to_string(), "place".to_string()),
            ]
            .into_iter()
            .collect(),
            default_o2o: [("use".to_string(), "scene-anchor".to_string())]
                .into_iter()
                .collect(),
            avoid_o2o: Default::default(),
            require_o2m: [("mood".to_string(), vec!["tense".to_string()])]
                .into_iter()
                .collect(),
            prefer_o2m: [("density".to_string(), vec!["crowded".to_string()])]
                .into_iter()
                .collect(),
            avoid_o2m: [("topic".to_string(), vec!["cafe".to_string()])]
                .into_iter()
                .collect(),
            count: 1,
            avoid_recent: true,
            weighted: false,
            metadata_suggest: vec!["sensory".into(), "affordances".into()],
        };
        let mut mood = place.clone();
        mood.name = "mood".into();
        let frame = Frame {
            name: "story".into(),
            description: Some("Story frame".into()),
            slots: vec![place, mood],
        };

        frames.save_frame(frame.clone()).await.unwrap();

        let got = frames.get_frame("story").await.unwrap().unwrap();
        assert_eq!(got, frame);

        let list = frames.list_frames().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slot_count, 2);

        // save_frame replaces (upsert), not duplicates.
        frames.save_frame(frame.clone()).await.unwrap();
        assert_eq!(frames.list_frames().await.unwrap().len(), 1);

        frames.delete_frame("story").await.unwrap();
        assert!(frames.get_frame("story").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn large_frames_load_replace_and_delete_across_parameter_chunks() {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);
        let slots = (0..1001)
            .map(|n| Slot {
                name: format!("slot-{n:04}"),
                under: None,
                require_o2o: [("axis".to_string(), format!("axis-{n}"))]
                    .into_iter()
                    .collect(),
                default_o2o: Default::default(),
                avoid_o2o: Default::default(),
                require_o2m: [("tag".to_string(), vec![format!("tag-{n}")])]
                    .into_iter()
                    .collect(),
                prefer_o2m: Default::default(),
                avoid_o2m: Default::default(),
                count: 1,
                avoid_recent: false,
                weighted: false,
                metadata_suggest: Vec::new(),
            })
            .collect();
        let frame = Frame {
            name: "large".into(),
            description: None,
            slots,
        };

        frames.save_frame(frame.clone()).await.unwrap();
        assert_eq!(frames.get_frame("large").await.unwrap().unwrap(), frame);

        frames.save_frame(frame).await.unwrap();
        frames.delete_frame("large").await.unwrap();
        assert!(frames.get_frame("large").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_save_rejects_invalid_slot_count() {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);
        let frame = Frame {
            name: "bad".into(),
            description: None,
            slots: vec![Slot {
                name: "empty".into(),
                under: None,
                require_o2o: Default::default(),
                default_o2o: Default::default(),
                avoid_o2o: Default::default(),
                require_o2m: Default::default(),
                prefer_o2m: Default::default(),
                avoid_o2m: Default::default(),
                count: 0,
                avoid_recent: false,
                weighted: false,
                metadata_suggest: Vec::new(),
            }],
        };

        let err = frames.save_frame(frame).await.unwrap_err().to_string();
        assert!(err.contains("invalid count"), "got: {err}");
    }

    #[tokio::test]
    async fn save_frames_is_atomic() {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);

        let bare = |name: &str, count: usize| Frame {
            name: name.into(),
            description: None,
            slots: vec![Slot {
                name: "s".into(),
                under: None,
                require_o2o: Default::default(),
                default_o2o: Default::default(),
                avoid_o2o: Default::default(),
                require_o2m: Default::default(),
                prefer_o2m: Default::default(),
                avoid_o2m: Default::default(),
                count,
                avoid_recent: false,
                weighted: false,
                metadata_suggest: Vec::new(),
            }],
        };

        // The second frame is invalid; the whole batch must roll back so the
        // earlier, valid frame is never committed.
        let err = frames
            .save_frames(vec![bare("good", 1), bare("bad", 0)])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid count"), "got: {err}");
        assert!(frames.get_frame("good").await.unwrap().is_none());
        assert!(frames.list_frames().await.unwrap().is_empty());

        // A valid batch commits every frame.
        frames
            .save_frames(vec![bare("a", 1), bare("b", 2)])
            .await
            .unwrap();
        assert_eq!(frames.list_frames().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn upsert_many_inserts_and_replaces() {
        let repo = repo().await;

        // First import: both are new.
        let mut a = Branch::new("/a");
        a.o2m.insert("topic".into(), vec!["one".into()]);
        let (added, updated) = repo
            .upsert_many(vec![a.clone(), Branch::new("/b")])
            .await
            .unwrap();
        assert_eq!((added, updated), (2, 0));

        // Second import: `/a` is updated (child rows replaced), `/c` is new.
        let mut a2 = Branch::new("/a");
        a2.o2m.insert("topic".into(), vec!["two".into()]);
        let (added, updated) = repo.upsert_many(vec![a2, Branch::new("/c")]).await.unwrap();
        assert_eq!((added, updated), (1, 1));

        let got = repo.get("/a").await.unwrap().unwrap();
        assert_eq!(got.o2m.get("topic").unwrap(), &vec!["two".to_string()]);
    }

    #[tokio::test]
    async fn upsert_many_always_advances_the_revision() {
        use sea_orm::ConnectionTrait;

        let repo = repo().await;
        repo.add(Branch::new("/revision")).await.unwrap();

        // Put the stored revision ahead of the wall clock. The import path
        // must advance from the persisted token instead of replacing it with
        // a potentially equal or older timestamp.
        repo.connection()
            .execute_unprepared(
                "UPDATE branches \
                 SET updated_at = '2999-01-01T00:00:00.000Z' \
                 WHERE path = '/revision'",
            )
            .await
            .unwrap();
        let stale = repo.get("/revision").await.unwrap().unwrap();

        let mut imported = Branch::new("/revision");
        imported.title = Some("imported".into());
        repo.upsert_many(vec![imported]).await.unwrap();

        let stored = repo.get("/revision").await.unwrap().unwrap();
        assert!(
            stored.revision.as_deref() > stale.revision.as_deref(),
            "import did not advance the revision: before={:?}, after={:?}",
            stale.revision,
            stored.revision
        );

        let mut stale_edit = stale;
        stale_edit.description = Some("must not win".into());
        assert!(repo.update(stale_edit).await.is_err());
    }

    #[tokio::test]
    async fn upsert_many_rejects_duplicate_paths_without_writing() {
        let repo = repo().await;
        let mut first = Branch::new("/duplicate");
        first.title = Some("first".into());
        let mut second = Branch::new("/duplicate");
        second.title = Some("second".into());

        let error = repo
            .upsert_many(vec![first, second])
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("duplicate branch path `/duplicate`"));
        assert!(repo.get("/duplicate").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn attach_reference_appends() {
        let repo = repo().await;
        repo.add(Branch::new("/ueno/cafe")).await.unwrap();

        repo.attach_reference(
            "/ueno/cafe",
            &Reference {
                kind: "file".into(),
                value: "refs/cafe.jpg".into(),
                note: None,
            },
        )
        .await
        .unwrap();
        repo.attach_reference(
            "/ueno/cafe",
            &Reference {
                kind: "url".into(),
                value: "https://example.com".into(),
                note: Some("ext".into()),
            },
        )
        .await
        .unwrap();

        let branch = repo.get("/ueno/cafe").await.unwrap().unwrap();
        assert_eq!(branch.references.len(), 2);
        assert_eq!(branch.references[0].kind, "file");
        assert_eq!(branch.references[1].note.as_deref(), Some("ext"));

        // Attaching to a missing branch errors.
        assert!(repo
            .attach_reference(
                "/nope",
                &Reference {
                    kind: "file".into(),
                    value: "x".into(),
                    note: None
                }
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn save_packet_with_usages_persists_both() {
        use history::{PacketRecord, SeaOrmHistoryRepository};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let branches = SeaOrmBranchRepository::new(db.clone());
        let history = SeaOrmHistoryRepository::new(db);

        // Two branches to record usage against.
        branches.add(Branch::new("/a")).await.unwrap();
        branches.add(Branch::new("/b")).await.unwrap();
        let a = branches.get("/a").await.unwrap().unwrap().id.unwrap();
        let b = branches.get("/b").await.unwrap().unwrap().id.unwrap();

        history
            .save_packet_with_usages(
                PacketRecord {
                    id: "pkt-1",
                    frame_name: None,
                    seed: Some(7),
                    query_json: None,
                    packet_json: "{}",
                },
                "sample",
                &[a, b],
            )
            .await
            .unwrap();

        // Each branch now has exactly one usage tied to the packet.
        assert_eq!(history.usage_for(a).await.unwrap().count, 1);
        assert_eq!(history.usage_for(b).await.unwrap().count, 1);
    }

    #[tokio::test]
    async fn packet_batch_rolls_back_every_packet_and_usage_on_failure() {
        use history::{PacketUsageRecord, SeaOrmHistoryRepository};
        use sea_orm::EntityTrait;
        use unclip_entity::selection_packets;

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let branches = SeaOrmBranchRepository::new(db.clone());
        let history = SeaOrmHistoryRepository::new(db.clone());

        branches.add(Branch::new("/a")).await.unwrap();
        let a = branches.get("/a").await.unwrap().unwrap().id.unwrap();
        let records = vec![
            PacketUsageRecord {
                id: "pkt-good".into(),
                frame_name: Some("story".into()),
                seed: Some(1),
                query_json: None,
                packet_json: "{}".into(),
                branch_ids: vec![a],
            },
            PacketUsageRecord {
                id: "pkt-bad".into(),
                frame_name: Some("story".into()),
                seed: Some(2),
                query_json: None,
                packet_json: "{}".into(),
                branch_ids: vec![i64::MAX],
            },
        ];

        let err = history
            .save_packets_with_usages(&records, "compose")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("branch id exceeds"), "got: {err}");
        assert_eq!(history.usage_for(a).await.unwrap().count, 0);
        assert!(selection_packets::Entity::find_by_id("pkt-good")
            .one(&db)
            .await
            .unwrap()
            .is_none());
        assert!(selection_packets::Entity::find_by_id("pkt-bad")
            .one(&db)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn branch_delete_removes_history_without_foreign_key_cascades() {
        use history::SeaOrmHistoryRepository;
        use sea_orm::ConnectionTrait;

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let branches = SeaOrmBranchRepository::new(db.clone());
        let history = SeaOrmHistoryRepository::new(db.clone());

        branches.add(Branch::new("/used")).await.unwrap();
        let id = branches.get("/used").await.unwrap().unwrap().id.unwrap();
        history
            .record_usage(id, "sample", None, None)
            .await
            .unwrap();
        assert_eq!(history.usage_for(id).await.unwrap().count, 1);

        db.execute_unprepared("PRAGMA foreign_keys = OFF;")
            .await
            .unwrap();
        branches.delete("/used").await.unwrap();

        assert_eq!(history.usage_for(id).await.unwrap().count, 0);
    }

    #[tokio::test]
    async fn pattern_add_list_roundtrip() {
        use pattern_repository::SeaOrmPatternRepository;
        use unclip_core::{PatternEntry, PatternTarget};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let patterns = SeaOrmPatternRepository::new(db);

        patterns
            .add(&PatternEntry::new(
                "coin locker",
                PatternTarget::O2m {
                    name: "object".into(),
                    value: "locker".into(),
                },
            ))
            .await
            .unwrap();
        patterns
            .add(&PatternEntry::new(
                "akira",
                PatternTarget::Branch {
                    path: "/movie/akira".into(),
                },
            ))
            .await
            .unwrap();

        let listed = patterns.list().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].entry.pattern, "coin locker");
        assert!(listed[0].enabled);

        let enabled = patterns.all_enabled().await.unwrap();
        assert_eq!(enabled.len(), 2);
        // Branch target round-trips its path.
        assert!(enabled.iter().any(|e| matches!(
            &e.target,
            PatternTarget::Branch { path } if path == "/movie/akira"
        )));
    }

    #[tokio::test]
    async fn pattern_add_rejects_invalid_entries() {
        use unclip_core::{PatternEntry, PatternTarget};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let patterns = SeaOrmPatternRepository::new(db);
        let invalid = [
            PatternEntry::new("   ", PatternTarget::Branch { path: "/ok".into() }),
            PatternEntry::new(
                "known",
                PatternTarget::O2m {
                    name: "tag".into(),
                    value: String::new(),
                },
            ),
            PatternEntry::new(
                "known",
                PatternTarget::Branch {
                    path: "relative".into(),
                },
            ),
        ];

        for entry in invalid {
            assert!(patterns.add(&entry).await.is_err(), "accepted {entry:?}");
        }
        assert!(patterns.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pattern_disable_enable_remove() {
        use pattern_repository::SeaOrmPatternRepository;
        use unclip_core::{PatternEntry, PatternTarget};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let patterns = SeaOrmPatternRepository::new(db);

        let id = patterns
            .add(&PatternEntry::new(
                "locker",
                PatternTarget::O2m {
                    name: "object".into(),
                    value: "locker".into(),
                },
            ))
            .await
            .unwrap();

        // Disable removes it from matcher input but keeps the row.
        assert!(patterns.set_enabled(id, false).await.unwrap());
        assert!(patterns.all_enabled().await.unwrap().is_empty());
        assert_eq!(patterns.list().await.unwrap().len(), 1);
        assert!(!patterns.list().await.unwrap()[0].enabled);

        // Re-enable.
        assert!(patterns.set_enabled(id, true).await.unwrap());
        assert_eq!(patterns.all_enabled().await.unwrap().len(), 1);

        // Remove deletes the row; a second remove reports no match.
        assert!(patterns.remove(id).await.unwrap());
        assert!(patterns.list().await.unwrap().is_empty());
        assert!(!patterns.remove(id).await.unwrap());
        assert!(!patterns.set_enabled(id, true).await.unwrap());
    }

    #[tokio::test]
    async fn attach_reference_rejects_invalid_reference() {
        let repo = repo().await;
        repo.add(Branch::new("/ueno/cafe")).await.unwrap();

        let err = repo
            .attach_reference(
                "/ueno/cafe",
                &Reference {
                    kind: String::new(),
                    value: "refs/cafe.jpg".into(),
                    note: None,
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("reference type must not be empty"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn frame_save_rejects_empty_constraint_names_and_values() {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);

        let mut slot = Slot {
            name: "bad".into(),
            under: None,
            require_o2o: Default::default(),
            default_o2o: Default::default(),
            avoid_o2o: Default::default(),
            require_o2m: Default::default(),
            prefer_o2m: Default::default(),
            avoid_o2m: Default::default(),
            count: 1,
            avoid_recent: false,
            weighted: false,
            metadata_suggest: Vec::new(),
        };
        slot.require_o2o.insert(String::new(), "story".into());
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![slot],
        };

        let err = frames.save_frame(frame).await.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "got: {err}");

        let mut slot = Slot {
            name: "bad".into(),
            under: None,
            require_o2o: Default::default(),
            default_o2o: Default::default(),
            avoid_o2o: Default::default(),
            require_o2m: Default::default(),
            prefer_o2m: Default::default(),
            avoid_o2m: Default::default(),
            count: 1,
            avoid_recent: false,
            weighted: false,
            metadata_suggest: Vec::new(),
        };
        slot.require_o2m.insert("mood".into(), vec![String::new()]);
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![slot],
        };

        let err = frames.save_frame(frame).await.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "got: {err}");
    }

    #[tokio::test]
    async fn frame_save_rejects_contradictory_constraints() {
        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);
        let mut slot = Slot {
            name: "bad".into(),
            under: None,
            require_o2o: Default::default(),
            default_o2o: Default::default(),
            avoid_o2o: Default::default(),
            require_o2m: Default::default(),
            prefer_o2m: Default::default(),
            avoid_o2m: Default::default(),
            count: 1,
            avoid_recent: false,
            weighted: false,
            metadata_suggest: Vec::new(),
        };
        slot.require_o2o.insert("axis".into(), "place".into());
        slot.avoid_o2o.insert("axis".into(), "place".into());
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![slot],
        };

        let err = frames.save_frame(frame).await.unwrap_err().to_string();
        assert!(err.contains("both required and avoided"), "got: {err}");
    }

    #[tokio::test]
    async fn history_rejects_oversized_ids_and_preserves_full_seed_range() {
        use history::{PacketRecord, SeaOrmHistoryRepository};
        use sea_orm::EntityTrait;
        use unclip_entity::selection_packets;

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let history = SeaOrmHistoryRepository::new(db.clone());

        let err = history
            .record_usage(i64::MAX, "sample", None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("branch id exceeds"), "got: {err}");

        let record = PacketRecord {
            id: "pkt-big-seed",
            frame_name: None,
            seed: Some(u64::MAX),
            query_json: None,
            packet_json: "{}",
        };
        history.save_packet(record).await.unwrap();
        let stored = selection_packets::Entity::find_by_id("pkt-big-seed")
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.seed, Some(-1));

        let record = PacketRecord {
            id: "pkt-big-branch",
            frame_name: None,
            seed: None,
            query_json: None,
            packet_json: "{}",
        };
        let err = history
            .save_packet_with_usages(record, "sample", &[i64::MAX])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("branch id exceeds"), "got: {err}");
    }

    #[tokio::test]
    async fn pattern_mutations_reject_ids_outside_sqlite_range() {
        use pattern_repository::SeaOrmPatternRepository;

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let patterns = SeaOrmPatternRepository::new(db);

        let err = patterns.remove(i64::MAX).await.unwrap_err().to_string();
        assert!(err.contains("pattern id exceeds"), "got: {err}");

        let err = patterns
            .set_enabled(i64::MAX, true)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern id exceeds"), "got: {err}");
    }
}
