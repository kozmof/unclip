//! unclip-core — pure domain model with no persistence dependencies.
//!
//! The model has six primary concepts: path, o2o, o2m, metadata,
//! frame, packet. These types stay independent from any SeaORM entity so the
//! store layer can map to/from them freely.

pub mod branch;
pub mod error;
pub mod frame;
pub mod packet;
pub mod pattern;
pub mod query;
pub mod reference;
pub mod validate;

pub use branch::{is_under, parent_of, Branch};
pub use error::{CoreError, Result};
pub use frame::{Frame, Slot};
pub use packet::{Selection, SelectionPacket, PACKET_KIND, PACKET_VERSION};
pub use pattern::{PatternEntry, PatternTarget};
pub use query::{SampleParams, SampleQuery};
pub use reference::Reference;
pub use validate::{validate_branch, validate_packet, validate_path};

#[cfg(test)]
mod tests {
    use super::*;

    /// The "recommended branch" YAML.
    const RECOMMENDED_BRANCH: &str = r#"
path: /ikebukuro/station/coin-locker
title: Coin Locker Area
description: A small coin locker area near a busy station corridor.
o2o:
  domain: story
  axis: place
  use: scene-anchor
o2m:
  density:
    - crowded
  mood:
    - hidden
    - tense
  topic:
    - transit
    - locker
    - waiting
  situation:
    - mistaken-exchange
weight: 1.0
metadata:
  sensory:
    visual:
      - metal locker doors
      - handwritten number labels
      - commuters passing behind
    sound:
      - coin return clink
      - train announcement echo
  affordances:
    - a key or bag can be exchanged by mistake
    - someone can wait without being noticed
references:
  - type: file
    value: refs/ikebukuro_exit.jpg
  - type: url
    value: https://example.com/reference
"#;

    #[test]
    fn minimal_branch_roundtrips() {
        let yaml = "path: /ikebukuro/station/exit\n";
        let branch: Branch = serde_norway::from_str(yaml).unwrap();
        assert_eq!(branch.path, "/ikebukuro/station/exit");
        assert_eq!(branch.weight, 1.0);
        assert!(branch.o2o.is_empty());
        assert!(branch.metadata.is_null());

        // Re-serializing a minimal branch keeps it minimal (no empty noise).
        let out = serde_norway::to_string(&branch).unwrap();
        assert!(out.contains("path: /ikebukuro/station/exit"));
        assert!(!out.contains("o2o"));
        assert!(!out.contains("metadata"));
    }

    #[test]
    fn recommended_branch_roundtrips_stably() {
        let branch: Branch = serde_norway::from_str(RECOMMENDED_BRANCH).unwrap();

        // Structural spot-checks.
        assert_eq!(branch.o2o.get("axis").unwrap(), "place");
        assert_eq!(branch.o2m.get("mood").unwrap(), &vec!["hidden", "tense"]);
        assert_eq!(branch.references.len(), 2);
        assert_eq!(branch.references[0].kind, "file");

        // Round-trip equality: serialize -> deserialize yields the same value.
        let yaml = serde_norway::to_string(&branch).unwrap();
        let again: Branch = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(branch, again);
    }

    #[test]
    fn parent_path_navigation() {
        assert_eq!(
            parent_of("/ikebukuro/station/exit").as_deref(),
            Some("/ikebukuro/station")
        );
        assert_eq!(parent_of("/ikebukuro").as_deref(), None);
        assert_eq!(parent_of("/").as_deref(), None);
    }

    #[test]
    fn packet_roundtrips() {
        let mut packet = SelectionPacket::new(Some("story".into()), Some(123456));
        packet.selections.push(Selection {
            slot: Some("place".into()),
            branch: Branch::new("/ikebukuro/station/coin-locker"),
        });

        assert_eq!(packet.version, PACKET_VERSION);
        assert_eq!(packet.kind, PACKET_KIND);

        let yaml = serde_norway::to_string(&packet).unwrap();
        let again: SelectionPacket = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(packet, again);
    }

    const STORY_PLACE_SLOT: &str = r#"
name: place
under: /ikebukuro
require_o2o:
  domain: story
  axis: place
default_o2o:
  use: scene-anchor
avoid_o2m:
  topic:
    - cafe
metadata_suggest:
  - sensory
  - affordances
"#;

    #[test]
    fn validate_branch_reports_violations() {
        let slot: Slot = serde_norway::from_str(STORY_PLACE_SLOT).unwrap();

        // A conforming branch.
        let mut ok = Branch::new("/ikebukuro/station/coin-locker");
        ok.o2o.insert("domain".into(), "story".into());
        ok.o2o.insert("axis".into(), "place".into());
        assert!(validate_branch(&slot, &ok).is_empty());

        // Wrong scope, missing required o2o, and an excluded o2m value.
        let mut bad = Branch::new("/ueno/cafe");
        bad.o2o.insert("domain".into(), "story".into());
        bad.o2m.insert("topic".into(), vec!["cafe".into()]);
        let violations = validate_branch(&slot, &bad);
        assert_eq!(violations.len(), 3, "got: {violations:?}");
    }

    #[test]
    fn validate_branch_checks_require_o2m() {
        let slot: Slot = serde_norway::from_str(
            "name: place\nrequire_o2m:\n  mood:\n    - tense\n    - hidden\n",
        )
        .unwrap();

        // Carries both required values -> no violations.
        let mut ok = Branch::new("/a");
        ok.o2m
            .insert("mood".into(), vec!["tense".into(), "hidden".into()]);
        assert!(validate_branch(&slot, &ok).is_empty());

        // Missing one required value -> exactly one violation.
        let mut partial = Branch::new("/b");
        partial.o2m.insert("mood".into(), vec!["tense".into()]);
        let violations = validate_branch(&slot, &partial);
        assert_eq!(violations.len(), 1, "got: {violations:?}");
        assert!(violations[0].contains("hidden"));
    }

    #[test]
    fn skeleton_seeds_o2o_and_metadata() {
        let slot: Slot = serde_norway::from_str(STORY_PLACE_SLOT).unwrap();
        let skel = slot.skeleton("/ikebukuro/station/coin-locker");
        assert_eq!(skel.o2o.get("domain").unwrap(), "story");
        assert_eq!(skel.o2o.get("axis").unwrap(), "place");
        assert_eq!(skel.o2o.get("use").unwrap(), "scene-anchor");
        assert!(skel.o2m.is_empty());
        assert!(skel.metadata.get("sensory").is_some());
    }

    #[test]
    fn validate_packet_checks_slots_and_counts() {
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![serde_norway::from_str::<Slot>(STORY_PLACE_SLOT).unwrap()],
        };

        // Packet missing the required `place` selection.
        let empty = SelectionPacket::new(Some("story".into()), None);
        let v = validate_packet(&frame, &empty);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("expects 1"));

        // Packet with a conforming selection.
        let mut packet = SelectionPacket::new(Some("story".into()), None);
        let mut branch = Branch::new("/ikebukuro/station/coin-locker");
        branch.o2o.insert("domain".into(), "story".into());
        branch.o2o.insert("axis".into(), "place".into());
        packet.selections.push(Selection {
            slot: Some("place".into()),
            branch,
        });
        assert!(validate_packet(&frame, &packet).is_empty());
    }

    #[test]
    fn validate_path_accepts_and_rejects() {
        assert!(validate_path("/ikebukuro/station/exit").is_ok());
        assert!(validate_path("/a").is_ok());

        for bad in ["/", "", "ikebukuro", "/a/", "/a//b", "/a/ b"] {
            assert!(
                validate_path(bad).is_err(),
                "expected `{bad}` to be invalid"
            );
        }
    }

    #[test]
    fn is_under_scope() {
        assert!(is_under("/a", "/a"));
        assert!(is_under("/a/b", "/a"));
        assert!(!is_under("/ab", "/a"));
        assert!(!is_under("/b", "/a"));
    }

    #[test]
    fn slot_to_query() {
        let yaml = r#"
name: place
require_o2o:
  domain: story
  axis: place
prefer_o2m:
  density:
    - crowded
avoid_o2m:
  topic:
    - cafe
count: 1
avoid_recent: true
"#;
        let slot: Slot = serde_norway::from_str(yaml).unwrap();
        let q = SampleQuery::from_slot(&slot, Some("/ikebukuro".into()));
        assert_eq!(q.under.as_deref(), Some("/ikebukuro"));
        assert_eq!(q.require_o2o.get("axis").unwrap(), "place");

        // Sampling controls split off into SampleParams.
        let p = SampleParams::from_slot(&slot);
        assert!(p.avoid_recent);
        assert_eq!(p.count, 1);
    }
}
