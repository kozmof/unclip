//! The output format shared by packet and branch rendering.

use std::str::FromStr;

/// Output format for rendered packets and branch exports.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse() {
        assert_eq!("yaml".parse::<Format>().unwrap(), Format::Yaml);
        assert_eq!("JSON".parse::<Format>().unwrap(), Format::Json);
        assert_eq!("jsonl".parse::<Format>().unwrap(), Format::Jsonl);
        assert!("xml".parse::<Format>().is_err());
    }
}
