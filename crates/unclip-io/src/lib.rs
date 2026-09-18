//! unclip-io — YAML/JSON/JSONL import and export.

#![forbid(unsafe_code)]

pub mod branch_io;
pub mod domain;
pub mod engine_profile;
pub mod format;
pub mod frames;
pub mod measurement_frame;
pub mod measurement_profile;
pub mod observation;
pub mod packet;
pub mod text;

pub use branch_io::{load_branches_file, parse_branches, parse_branches_jsonl, render_branches};
pub use domain::{load_domain, parse_domain, render_domain};
pub use engine_profile::{
    load_engine_profile, parse_engine_profile, EngineProfileDocument, ParsedEngineProfile,
    PluginConfig,
};
pub use format::Format;
pub use frames::{load_frames, parse_frames, split_frame_selector};
pub use measurement_frame::{
    load_measurement_frame, parse_measurement_frame, render_measurement_frame,
    MeasurementFrameDocument,
};
pub use measurement_profile::{
    parse_measurement_profile, parse_measurement_profile_jsonl, render_measurement_profile,
};
pub use observation::{
    load_manual_observation, parse_manual_observation, render_manual_observation,
    ManualObservationDocument,
};
pub use packet::{render_packet, render_packets};
pub use text::{read_text_file, MAX_TEXT_BYTES};
