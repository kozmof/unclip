//! unclip-core — pure domain model with no persistence dependencies.
//!
//! The model has six primary concepts (DRAFT §2): path, o2o, o2m, metadata,
//! frame, packet. These types stay independent from any SeaORM entity so the
//! store layer can map to/from them freely.

pub mod branch;
pub mod error;
pub mod frame;
pub mod packet;
pub mod query;
pub mod reference;

pub use branch::{parent_of, Branch};
pub use error::{CoreError, Result};
pub use frame::{Frame, Slot};
pub use packet::{Selection, SelectionPacket, PACKET_KIND, PACKET_VERSION};
pub use query::SampleQuery;
pub use reference::Reference;

#[cfg(test)]
mod tests {
    use super::*;

    /// The "recommended branch" YAML from DRAFT §8.
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
        let branch: Branch = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(branch.path, "/ikebukuro/station/exit");
        assert_eq!(branch.weight, 1.0);
        assert!(branch.o2o.is_empty());
        assert!(branch.metadata.is_null());

        // Re-serializing a minimal branch keeps it minimal (no empty noise).
        let out = serde_yaml::to_string(&branch).unwrap();
        assert!(out.contains("path: /ikebukuro/station/exit"));
        assert!(!out.contains("o2o"));
        assert!(!out.contains("metadata"));
    }

    #[test]
    fn recommended_branch_roundtrips_stably() {
        let branch: Branch = serde_yaml::from_str(RECOMMENDED_BRANCH).unwrap();

        // Structural spot-checks.
        assert_eq!(branch.o2o.get("axis").unwrap(), "place");
        assert_eq!(branch.o2m.get("mood").unwrap(), &vec!["hidden", "tense"]);
        assert_eq!(branch.references.len(), 2);
        assert_eq!(branch.references[0].kind, "file");

        // Round-trip equality: serialize -> deserialize yields the same value.
        let yaml = serde_yaml::to_string(&branch).unwrap();
        let again: Branch = serde_yaml::from_str(&yaml).unwrap();
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

        let yaml = serde_yaml::to_string(&packet).unwrap();
        let again: SelectionPacket = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(packet, again);
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
        let slot: Slot = serde_yaml::from_str(yaml).unwrap();
        let q = SampleQuery::from_slot(&slot, Some("/ikebukuro".into()));
        assert_eq!(q.under.as_deref(), Some("/ikebukuro"));
        assert_eq!(q.require_o2o.get("axis").unwrap(), "place");
        assert!(q.avoid_recent);
        assert_eq!(q.count, 1);
    }
}
