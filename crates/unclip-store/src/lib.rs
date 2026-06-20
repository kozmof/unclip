//! unclip-store — repository traits and SeaORM-backed persistence.

pub mod mapper;
pub mod repository;
pub mod seaorm;

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
        let mut q = SampleQuery::default();
        q.under = Some("/ikebukuro".into());
        q.require_o2o.insert("axis".into(), "place".into());
        let found = repo.find(q).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/ikebukuro/station/exit");
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
}
