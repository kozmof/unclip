//! Rendering selection packets to YAML / JSON / JSONL.

use std::str::FromStr;

use unclip_core::SelectionPacket;

/// Output format for packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Yaml,
    Json,
    Jsonl,
}

impl FromStr for Format {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => Ok(Format::Yaml),
            "json" => Ok(Format::Json),
            "jsonl" | "ndjson" => Ok(Format::Jsonl),
            other => anyhow::bail!("unknown format `{other}` (expected yaml, json, or jsonl)"),
        }
    }
}

/// Render a single packet (no trailing newline beyond the format's own).
pub fn render_packet(packet: &SelectionPacket, format: Format) -> anyhow::Result<String> {
    Ok(match format {
        Format::Yaml => serde_yaml::to_string(packet)?,
        Format::Json => format!("{}\n", serde_json::to_string_pretty(packet)?),
        Format::Jsonl => format!("{}\n", serde_json::to_string(packet)?),
    })
}

/// Render a batch of packets.
///
/// - YAML: multiple documents separated by `---`.
/// - JSON: a single object when there is one packet, otherwise an array.
/// - JSONL: one packet per line (the natural batch format).
pub fn render_packets(packets: &[SelectionPacket], format: Format) -> anyhow::Result<String> {
    match format {
        Format::Yaml => {
            let docs: Result<Vec<_>, _> = packets.iter().map(serde_yaml::to_string).collect();
            Ok(docs?.join("---\n"))
        }
        Format::Json => {
            if packets.len() == 1 {
                Ok(format!("{}\n", serde_json::to_string_pretty(&packets[0])?))
            } else {
                Ok(format!("{}\n", serde_json::to_string_pretty(packets)?))
            }
        }
        Format::Jsonl => {
            let mut out = String::new();
            for packet in packets {
                out.push_str(&serde_json::to_string(packet)?);
                out.push('\n');
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_core::{Branch, Selection, SelectionPacket};

    fn packet(seed: u64) -> SelectionPacket {
        let mut p = SelectionPacket::new(Some("story".into()), Some(seed));
        p.selections.push(Selection {
            slot: Some("place".into()),
            branch: Branch::new("/ikebukuro/station/coin-locker"),
        });
        p
    }

    #[test]
    fn formats_parse() {
        assert_eq!("yaml".parse::<Format>().unwrap(), Format::Yaml);
        assert_eq!("JSON".parse::<Format>().unwrap(), Format::Json);
        assert_eq!("jsonl".parse::<Format>().unwrap(), Format::Jsonl);
        assert!("xml".parse::<Format>().is_err());
    }

    #[test]
    fn jsonl_one_line_per_packet() {
        let out = render_packets(&[packet(1), packet(2)], Format::Jsonl).unwrap();
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line is valid standalone JSON.
        for line in lines {
            let _: SelectionPacket = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn json_single_vs_array() {
        let one = render_packets(&[packet(1)], Format::Json).unwrap();
        assert!(one.trim_start().starts_with('{'));
        let many = render_packets(&[packet(1), packet(2)], Format::Json).unwrap();
        assert!(many.trim_start().starts_with('['));
    }

    #[test]
    fn yaml_roundtrips_single() {
        let p = packet(123);
        let rendered = render_packet(&p, Format::Yaml).unwrap();
        let back: SelectionPacket = serde_yaml::from_str(&rendered).unwrap();
        assert_eq!(back, p);
    }
}
