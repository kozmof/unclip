//! unclip-store — repository traits and SeaORM-backed persistence.

pub mod frame_mapper;
pub mod frame_repository;
pub mod history;
pub mod mapper;
pub mod pattern_repository;
pub mod repository;
pub mod seaorm;

pub use frame_repository::{FrameInfo, FrameRepository, SeaOrmFrameRepository};
pub use history::{now, PacketRecord, SeaOrmHistoryRepository, UsageSummary};
pub use pattern_repository::{SeaOrmPatternRepository, StoredPattern};
pub use repository::{BranchRepository, IndexedValue, SeaOrmBranchRepository};
pub use seaorm::{connect, connect_and_migrate};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use unclip_core::{Branch, Reference, SampleQuery};

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
        b.o2m.insert("mood".into(), vec!["hidden".into(), "tense".into()]);
        b.o2m.insert("topic".into(), vec!["locker".into(), "transit".into()]);
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
    async fn add_get_update_delete_roundtrip() {
        let repo = repo().await;
        let branch = sample_branch();
        repo.add(branch.clone()).await.unwrap();

        // get round-trips o2o/o2m/metadata/references (ignoring assigned id).
        let mut got = repo.get(&branch.path).await.unwrap().unwrap();
        assert!(got.id.is_some());
        got.id = None;
        assert_eq!(got, branch);

        // update mutates indexed values.
        let mut edited = got.clone();
        edited.o2m
            .insert("mood".into(), vec!["urgent".into()]);
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
        let mut found: Vec<_> = repo.find(q).await.unwrap().into_iter().map(|b| b.path).collect();
        found.sort();
        assert_eq!(found, vec!["/a_b", "/a_b/child"]);
    }

    #[tokio::test]
    async fn catalog_and_value_lookup() {
        let repo = repo().await;
        for (path, axis) in [
            ("/a", "place"),
            ("/b", "place"),
            ("/c", "time"),
        ] {
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
        use unclip_core::{Frame, Slot};
        use frame_repository::{FrameRepository, SeaOrmFrameRepository};

        let db = connect_and_migrate("sqlite::memory:").await.unwrap();
        let frames = SeaOrmFrameRepository::new(db);

        let place = Slot {
            name: "place".into(),
            under: Some("/ikebukuro".into()),
            require_o2o: [("domain".to_string(), "story".to_string()),
                         ("axis".to_string(), "place".to_string())]
                .into_iter()
                .collect(),
            default_o2o: [("use".to_string(), "scene-anchor".to_string())]
                .into_iter()
                .collect(),
            avoid_o2o: Default::default(),
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
        let frame = Frame {
            name: "story".into(),
            description: Some("Story frame".into()),
            slots: vec![place],
        };

        frames.save_frame(frame.clone()).await.unwrap();

        let got = frames.get_frame("story").await.unwrap().unwrap();
        assert_eq!(got, frame);

        let list = frames.list_frames().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slot_count, 1);

        // save_frame replaces (upsert), not duplicates.
        frames.save_frame(frame.clone()).await.unwrap();
        assert_eq!(frames.list_frames().await.unwrap().len(), 1);

        frames.delete_frame("story").await.unwrap();
        assert!(frames.get_frame("story").await.unwrap().is_none());
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
        let (added, updated) = repo
            .upsert_many(vec![a2, Branch::new("/c")])
            .await
            .unwrap();
        assert_eq!((added, updated), (1, 1));

        let got = repo.get("/a").await.unwrap().unwrap();
        assert_eq!(got.o2m.get("topic").unwrap(), &vec!["two".to_string()]);
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
            .attach_reference("/nope", &Reference { kind: "file".into(), value: "x".into(), note: None })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn pattern_add_list_roundtrip() {
        use pattern_repository::SeaOrmPatternRepository;
        use unclip_match::{PatternEntry, PatternTarget};

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
}
