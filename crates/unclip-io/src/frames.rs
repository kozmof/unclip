//! Parsing of frame-definition YAML files.
//!
//! ```yaml
//! frames:
//!   story:
//!     description: optional
//!     slots:
//!       - name: place
//!         require_o2o:
//!           domain: story
//!           axis: place
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use unclip_core::{Frame, Slot};

#[derive(Debug, Deserialize)]
struct FramesFile {
    #[serde(default)]
    frames: BTreeMap<String, FrameBody>,
}

#[derive(Debug, Deserialize)]
struct FrameBody {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    slots: Vec<Slot>,
}

/// Parse a frames file from a YAML string into domain `Frame`s, ordered by name.
pub fn parse_frames(yaml: &str) -> anyhow::Result<Vec<Frame>> {
    let file: FramesFile = serde_yaml::from_str(yaml)?;
    Ok(file
        .frames
        .into_iter()
        .map(|(name, body)| Frame {
            name,
            description: body.description,
            slots: body.slots,
        })
        .collect())
}

/// Load and parse a frames file from disk.
pub fn load_frames(path: &Path) -> anyhow::Result<Vec<Frame>> {
    let text = std::fs::read_to_string(path)?;
    parse_frames(&text)
}

/// Split a `frame` or `frame.slot` selector into its parts.
pub fn split_frame_selector(selector: &str) -> (&str, Option<&str>) {
    match selector.split_once('.') {
        Some((frame, slot)) => (frame, Some(slot)),
        None => (selector, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the frame definition.
    const STORY_FRAME: &str = r#"
frames:
  story:
    slots:
      - name: place
        require_o2o:
          domain: story
          axis: place
        default_o2o:
          use: scene-anchor
        prefer_o2m:
          density:
            - crowded
        avoid_o2m:
          topic:
            - cafe
        metadata_suggest:
          - sensory
          - affordances
      - name: avoid
        require_o2o:
          domain: story
          use: avoid
        count: 2
"#;

    #[test]
    fn parses_story_frame() {
        let frames = parse_frames(STORY_FRAME).unwrap();
        assert_eq!(frames.len(), 1);
        let story = &frames[0];
        assert_eq!(story.name, "story");
        assert_eq!(story.slots.len(), 2);

        let place = story.slot("place").unwrap();
        assert_eq!(place.require_o2o.get("axis").unwrap(), "place");
        assert_eq!(place.default_o2o.get("use").unwrap(), "scene-anchor");
        assert_eq!(place.count, 1); // default

        let avoid = story.slot("avoid").unwrap();
        assert_eq!(avoid.count, 2);
    }

    #[test]
    fn split_selector() {
        assert_eq!(split_frame_selector("story"), ("story", None));
        assert_eq!(
            split_frame_selector("story.place"),
            ("story", Some("place"))
        );
    }
}
