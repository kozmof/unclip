//! unclip-core — pure domain model with no persistence dependencies.
//!
//! The model has six primary concepts: path, o2o, o2m, metadata,
//! frame, packet. These types stay independent from any SeaORM entity so the
//! store layer can map to/from them freely.

#![forbid(unsafe_code)]

pub mod branch;
pub mod error;
pub mod frame;
mod frame_validation;
pub mod packet;
pub mod pattern;
pub mod query;
mod query_validation;
pub mod reference;
pub mod validate;

pub use branch::{ancestor_paths, is_under, parent_of, Branch};
pub use error::{CoreError, Result};
pub use frame::{Frame, Slot};
pub use frame_validation::validate_frame;
pub use packet::{Selection, SelectionPacket, PACKET_KIND, PACKET_VERSION};
pub use pattern::{validate_pattern_entry, PatternEntry, PatternTarget};
pub use query::{SampleParams, SampleQuery};
pub use query_validation::validate_sample_query;
pub use reference::Reference;
pub use validate::{
    validate_branch, validate_branch_record, validate_packet, validate_path, validate_reference,
    MAX_BRANCH_COLLECTION_ITEMS, MAX_BRANCH_RECORD_BYTES, MAX_DOMAIN_STRING_BYTES,
    MAX_FRAME_COLLECTION_ITEMS, MAX_PATH_BYTES, MAX_QUERY_FILTER_ITEMS,
};

#[cfg(test)]
mod tests {
    use std::rc::Rc;

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
    fn equality_ignores_persistence_bookkeeping() {
        // `id`/`revision` are `#[serde(skip)]` storage fields, so a branch read
        // back from the database must still compare equal to the same branch
        // parsed from a file — that comparison is what import/export and
        // round-trip tests are actually asserting.
        let parsed = Branch::new("/ikebukuro/station/exit");
        let mut stored = parsed.clone();
        stored.id = Some(42);
        stored.revision = Some("2026-08-01T00:00:00.000Z".into());
        assert_eq!(parsed, stored);

        // Domain fields still distinguish branches.
        let mut different = parsed.clone();
        different.title = Some("Exit".into());
        assert_ne!(parsed, different);
    }

    #[test]
    fn parent_path_navigation() {
        assert_eq!(
            parent_of("/ikebukuro/station/exit"),
            Some("/ikebukuro/station")
        );
        assert_eq!(parent_of("/ikebukuro"), None);
        assert_eq!(parent_of("/"), None);
    }

    #[test]
    fn ancestor_paths_walks_the_whole_chain() {
        assert_eq!(
            ancestor_paths("/ikebukuro/station/exit").collect::<Vec<_>>(),
            vec!["/ikebukuro", "/ikebukuro/station"]
        );
        // A top-level path and the bare root have no ancestors.
        assert!(ancestor_paths("/ikebukuro").next().is_none());
        assert!(ancestor_paths("/").next().is_none());
        // A trailing slash is trimmed first, matching `parent_of`.
        assert_eq!(
            ancestor_paths("/a/b/c/").collect::<Vec<_>>(),
            vec!["/a", "/a/b"]
        );
        // The chain is exactly what repeated `parent_of` produces.
        let mut walked = Vec::new();
        let mut cursor = Some("/a/b/c");
        while let Some(path) = cursor.and_then(parent_of) {
            walked.push(path);
            cursor = Some(path);
        }
        walked.reverse();
        assert_eq!(walked, ancestor_paths("/a/b/c").collect::<Vec<_>>());
    }

    #[test]
    fn packet_roundtrips() {
        let mut packet = SelectionPacket::new(Some("story".into()), Some(123456));
        packet.selections.push(Selection {
            slot: Some("place".into()),
            branch: Rc::new(Branch::new("/ikebukuro/station/coin-locker")),
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
    fn validate_branch_record_rejects_negative_weight() {
        let mut branch = Branch::new("/negative");
        branch.weight = -1.0;
        assert!(validate_branch_record(&branch).is_err());
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

        // Un-slotted selections are not valid for a frame packet.
        let mut unslotted = SelectionPacket::new(Some("story".into()), None);
        unslotted.selections.push(Selection {
            slot: None,
            branch: Rc::new(Branch::new("/ikebukuro/station/coin-locker")),
        });
        let v = validate_packet(&frame, &unslotted);
        assert!(v.iter().any(|reason| reason.contains("has no slot")));

        // Packet with a conforming selection.
        let mut packet = SelectionPacket::new(Some("story".into()), None);
        let mut branch = Branch::new("/ikebukuro/station/coin-locker");
        branch.o2o.insert("domain".into(), "story".into());
        branch.o2o.insert("axis".into(), "place".into());
        packet.selections.push(Selection {
            slot: Some("place".into()),
            branch: Rc::new(branch),
        });
        assert!(validate_packet(&frame, &packet).is_empty());

        let mut incompatible = packet.clone();
        incompatible.version = PACKET_VERSION + 1;
        incompatible.kind = "other.kind".into();
        incompatible.frame = Some("other".into());
        incompatible
            .selections
            .push(incompatible.selections[0].clone());
        let violations = validate_packet(&frame, &incompatible);
        assert!(violations.iter().any(|reason| reason.contains("version")));
        assert!(violations.iter().any(|reason| reason.contains("kind")));
        assert!(violations
            .iter()
            .any(|reason| reason.contains("packet frame")));
        assert!(violations.iter().any(|reason| reason.contains("got 2")));
    }

    #[test]
    fn validate_packet_rejects_duplicate_branches_in_one_slot() {
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![Slot {
                name: "place".into(),
                count: 2,
                ..Default::default()
            }],
        };
        // Two selections of the *same* branch satisfy the slot count but not
        // the without-replacement contract.
        let mut packet = SelectionPacket::new(Some("story".into()), None);
        for _ in 0..2 {
            packet.selections.push(Selection {
                slot: Some("place".into()),
                branch: Rc::new(Branch::new("/ikebukuro/station/coin-locker")),
            });
        }
        let violations = validate_packet(&frame, &packet);
        assert_eq!(violations.len(), 1, "got: {violations:?}");
        assert!(violations[0].contains("more than once"));

        // The same branch in *different* slots is legitimate.
        let frame_two_slots = Frame {
            name: "story".into(),
            description: None,
            slots: vec![
                Slot {
                    name: "place".into(),
                    ..Default::default()
                },
                Slot {
                    name: "mood".into(),
                    ..Default::default()
                },
            ],
        };
        let mut cross_slot = SelectionPacket::new(Some("story".into()), None);
        for slot in ["place", "mood"] {
            cross_slot.selections.push(Selection {
                slot: Some(slot.into()),
                branch: Rc::new(Branch::new("/ikebukuro/station/coin-locker")),
            });
        }
        assert!(validate_packet(&frame_two_slots, &cross_slot).is_empty());
    }

    #[test]
    fn packet_rejects_unknown_fields() {
        let yaml = r#"
version: 1
kind: unclip.selection
frame: story
selections: []
unexpected: true
"#;
        assert!(serde_norway::from_str::<SelectionPacket>(yaml).is_err());
    }

    #[test]
    fn validate_branch_record_rejects_invalid_state() {
        let mut branch = Branch::new("/bad");
        branch.weight = f64::NAN;
        assert!(validate_branch_record(&branch).is_err());

        let mut branch = Branch::new("/bad");
        branch.o2o.insert("axis".into(), "".into());
        assert!(validate_branch_record(&branch).is_err());

        let mut branch = Branch::new("/bad");
        branch
            .o2m
            .insert("topic".into(), vec!["line\nbreak".into()]);
        assert!(validate_branch_record(&branch).is_err());
    }

    #[test]
    fn validate_branch_record_rejects_oversized_records() {
        assert!(validate_path(&format!("/{}", "x".repeat(MAX_PATH_BYTES))).is_err());

        let mut branch = Branch::new("/oversized-value");
        branch
            .o2o
            .insert("axis".into(), "x".repeat(MAX_DOMAIN_STRING_BYTES + 1));
        assert!(validate_branch_record(&branch).is_err());

        let mut branch = Branch::new("/oversized-metadata");
        branch.metadata = serde_json::json!({ "payload": "x".repeat(MAX_BRANCH_RECORD_BYTES) });
        assert!(validate_branch_record(&branch).is_err());
    }

    #[test]
    fn validate_path_accepts_and_rejects() {
        assert!(validate_path("/ikebukuro/station/exit").is_ok());
        assert!(validate_path("/a").is_ok());

        for bad in ["/", "", "ikebukuro", "/a/", "/a//b", "/a/ b", "/a/\0b"] {
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

        // Sampling controls split off into SampleParams, which reads the slot
        // before the filter consumes it.
        let p = SampleParams::from_slot(&slot);
        assert!(p.avoid_recent);
        assert_eq!(p.count, 1);

        let q = SampleQuery::from_slot(slot, Some("/ikebukuro".into()));
        assert_eq!(q.under.as_deref(), Some("/ikebukuro"));
        assert_eq!(q.require_o2o.get("axis").unwrap(), "place");
    }

    #[test]
    fn avoid_o2o_accepts_one_or_many_and_excludes_each_value() {
        // Legacy single-string form parses as a one-element list.
        let slot: Slot =
            serde_norway::from_str("name: place\navoid_o2o:\n  use: background\n").unwrap();
        assert_eq!(
            slot.avoid_o2o.get("use"),
            Some(&vec!["background".to_string()])
        );

        // List form: several avoided values of one name.
        let slot: Slot = serde_norway::from_str(
            "name: place\navoid_o2o:\n  use:\n    - background\n    - prop\n",
        )
        .unwrap();
        assert_eq!(slot.avoid_o2o.get("use").map(Vec::len), Some(2));

        // The canonical serialized shape (a list) re-parses identically.
        let yaml = serde_norway::to_string(&slot).unwrap();
        let again: Slot = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(slot, again);

        // Every listed value excludes a carrying branch.
        for value in ["background", "prop"] {
            let mut branch = Branch::new("/x");
            branch.o2o.insert("use".into(), value.into());
            let violations = validate_branch(&slot, &branch);
            assert_eq!(violations.len(), 1, "got: {violations:?}");
            assert!(violations[0].contains("excluded"));
        }
    }

    #[test]
    fn displayed_text_rejects_terminal_control_characters() {
        let mut branch = Branch::new("/unsafe-title");
        branch.title = Some("safe\u{1b}[31mred".into());
        assert!(validate_branch_record(&branch).is_err());

        let reference = Reference {
            kind: "file".into(),
            value: "notes.md".into(),
            note: Some("note\u{1b}[2J".into()),
        };
        assert!(validate_reference(&reference).is_err());
    }

    #[test]
    fn packet_validation_rejects_intrinsically_invalid_branches() {
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: vec![Slot {
                name: "place".into(),
                ..Default::default()
            }],
        };
        let mut packet = SelectionPacket::new(Some("story".into()), None);
        packet.selections.push(Selection {
            slot: Some("place".into()),
            branch: Rc::new(Branch::new("relative-path")),
        });

        let violations = validate_packet(&frame, &packet);
        assert!(violations
            .iter()
            .any(|reason| reason.contains("contains an invalid branch")));
    }

    #[test]
    fn frame_and_query_validation_are_core_domain_rules() {
        let frame = Frame {
            name: "story".into(),
            description: Some("unsafe\u{1b}[2J".into()),
            slots: Vec::new(),
        };
        assert!(matches!(
            validate_frame(&frame),
            Err(CoreError::InvalidFrame { .. })
        ));

        let mut query = SampleQuery::default();
        query.avoid_o2m.insert(
            "topic".into(),
            (0..MAX_QUERY_FILTER_ITEMS)
                .map(|index| format!("value-{index}"))
                .collect(),
        );
        assert!(matches!(
            validate_sample_query(&query),
            Err(CoreError::InvalidQuery(_))
        ));
    }
}
