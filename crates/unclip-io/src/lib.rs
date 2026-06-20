//! unclip-io — YAML/JSON/JSONL import and export.

pub mod frames;

pub use frames::{load_frames, parse_frames, split_frame_selector};
