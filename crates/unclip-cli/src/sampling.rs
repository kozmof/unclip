//! `sample`, `compose`, and usage-reporting (`used`/`stats`/`stale`) handlers.

use std::collections::HashSet;

use anyhow::{ensure, Context};
use unclip_core::{validate_path, Frame, SampleParams, SampleQuery, Selection, SelectionPacket};
use unclip_io::Format;
use unclip_sample::{random_packet_id, random_seed, rng_from_seed, sample, score, Reservoir};
use unclip_store::{now, BranchReader, HistoryRepository, PacketUsageRecord};

/// How many recent usage rows define the "recently used" set.
const RECENT_LIMIT: u64 = 50;

/// Page size for streaming commands (`export`, `stats`, `sample`); keeps each
/// SQL page bounded while results are consumed instead of accumulated.
const STREAM_PAGE_SIZE: u64 = 1_000;

/// Prevent a typo or hostile input from preallocating an unbounded packet
/// batch. Larger jobs can be split across invocations with explicit seeds.
const MAX_COMPOSE_COUNT: usize = 1_000;

/// Filters shared by `sample`, `stats`, and `stale`.
pub struct FilterInput {
    pub under: Option<String>,
    pub require_o2o: Vec<(String, String)>,
    pub avoid_o2o: Vec<(String, String)>,
    pub require_o2m: Vec<(String, String)>,
    pub prefer_o2m: Vec<(String, String)>,
    pub avoid_o2m: Vec<(String, String)>,
}

impl FilterInput {
    /// Combine the shared filter flags with a command-specific `prefer_o2m`
    /// list. Commands that never score (`query`, `stats`, `stale`, `export`)
    /// pass an empty one — they do not expose the flag at all.
    pub fn from_args(args: crate::cli::FilterArgs, prefer_o2m: Vec<(String, String)>) -> Self {
        Self {
            under: args.under,
            require_o2o: args.o2o,
            avoid_o2o: args.avoid_o2o,
            require_o2m: args.require_o2m,
            prefer_o2m,
            avoid_o2m: args.avoid_o2m,
        }
    }

    /// Assemble the hard/soft filter from the parsed flags. Sampling controls
    /// (count/weighted/avoid_recent) are not part of the filter — callers that
    /// draw build a [`SampleParams`] separately.
    pub fn into_query(self) -> anyhow::Result<SampleQuery> {
        if let Some(under) = &self.under {
            validate_path(under).with_context(|| format!("invalid --under scope `{under}`"))?;
        }
        let mut q = SampleQuery {
            under: self.under,
            ..Default::default()
        };
        crate::commands::merge_o2o(&mut q.require_o2o, self.require_o2o)?;
        // avoid_o2o accumulates: several values of one name can be excluded
        // at once (matching avoid_o2m), unlike require_o2o's one per name.
        for (name, value) in self.avoid_o2o {
            q.avoid_o2o.entry(name).or_default().push(value);
        }
        for (name, value) in self.require_o2m {
            q.require_o2m.entry(name).or_default().push(value);
        }
        for (name, value) in self.prefer_o2m {
            q.prefer_o2m.entry(name).or_default().push(value);
        }
        for (name, value) in self.avoid_o2m {
            q.avoid_o2m.entry(name).or_default().push(value);
        }
        Ok(q)
    }
}

/// Arguments for `sample`.
pub struct SampleInput {
    pub filter: FilterInput,
    pub count: usize,
    pub weighted: bool,
    pub avoid_recent: bool,
    pub seed: Option<u64>,
    pub format: Format,
    pub dry_run: bool,
}

pub async fn sample_cmd(
    branches: &impl BranchReader,
    history: &impl HistoryRepository,
    input: SampleInput,
) -> anyhow::Result<()> {
    let SampleInput {
        filter,
        count,
        weighted,
        avoid_recent,
        seed,
        format,
        dry_run,
    } = input;

    let query = filter.into_query()?;
    let params = SampleParams {
        count,
        weighted,
        avoid_recent,
    };
    run_sample(branches, history, query, params, seed, format, dry_run).await
}

/// Draw one packet from a fully assembled query/params pair.
///
/// Shared by `sample` (which builds the pair from flags) and `replay` (which
/// reconstructs it from a packet's embedded provenance).
///
/// Candidates are streamed page by page through a weighted [`Reservoir`], so
/// the command holds at most one page plus `count` branches in memory and is
/// not subject to the bounded `find` candidate ceiling. Pages arrive in path
/// order and each candidate consumes one RNG draw, so a fixed seed still
/// reproduces the same selection.
async fn run_sample(
    branches: &impl BranchReader,
    history: &impl HistoryRepository,
    query: SampleQuery,
    params: SampleParams,
    seed: Option<u64>,
    format: Format,
    dry_run: bool,
) -> anyhow::Result<()> {
    ensure!(params.count > 0, "sample count must be greater than zero");

    let recent = if params.avoid_recent {
        history.recent_branch_ids(RECENT_LIMIT).await?
    } else {
        Default::default()
    };

    let seed = seed.unwrap_or_else(random_seed);
    let mut rng = rng_from_seed(seed);
    let mut reservoir = Reservoir::new(params.count);
    let mut after_path: Option<String> = None;
    loop {
        let page = branches
            .find_page(&query, after_path.as_deref(), STREAM_PAGE_SIZE)
            .await?;
        let done = (page.len() as u64) < STREAM_PAGE_SIZE;
        after_path = page.last().map(|branch| branch.path.clone());
        for branch in page {
            let s = score(&branch, &query, &params, &recent);
            reservoir.offer(branch, s, &mut rng);
        }
        if done {
            break;
        }
    }

    let mut packet = SelectionPacket::new(None, Some(seed));
    packet.created_at = Some(now());
    packet.query = Some(query_provenance(&query, &params)?);
    packet.selections = reservoir
        .into_branches()
        .into_iter()
        .map(|branch| Selection { slot: None, branch })
        .collect();

    let rendered = unclip_io::render_packet(&packet, format)?;

    // Commit before emission so a successfully emitted packet is always
    // recoverable from the packet store. Stdout and SQLite cannot share an
    // atomic commit: an output failure may therefore leave a persisted packet,
    // while `--dry-run` remains side-effect free.
    if !dry_run {
        let record = packet_usage_record(None, &packet)?;
        history
            .save_packets_with_usages(std::slice::from_ref(&record), "sample")
            .await?;
    }
    crate::output::write_stdout(&rendered)?;
    Ok(())
}

/// clap value parser for `--format`.
pub fn parse_format(s: &str) -> anyhow::Result<Format> {
    s.parse()
}

/// A `--under` override for compose: a slot-specific or global scope.
///
/// `slot: None` is a global override applied to any slot without a more
/// specific one. Serializable so compose can record its overrides in packet
/// provenance and `replay` can reconstruct them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnderOverride {
    pub slot: Option<String>,
    pub path: String,
}

/// Parse `slot:/path` (slot-specific) or `/path` (global) overrides.
pub fn parse_under_override(raw: &str) -> anyhow::Result<UnderOverride> {
    let parsed = match raw.split_once(':') {
        Some((slot, path)) if !slot.is_empty() => UnderOverride {
            slot: Some(slot.to_string()),
            path: path.to_string(),
        },
        _ => UnderOverride {
            slot: None,
            path: raw.to_string(),
        },
    };
    validate_path(&parsed.path)
        .with_context(|| format!("invalid --under scope `{}`", parsed.path))?;
    Ok(parsed)
}

/// Arguments for `compose`.
pub struct ComposeInput {
    pub frame: String,
    pub under: Vec<UnderOverride>,
    pub count: usize,
    pub seed: Option<u64>,
    pub format: Format,
    pub dry_run: bool,
}

pub async fn compose_cmd(
    branches: &impl BranchReader,
    frames: &impl unclip_store::FrameRepository,
    history: &impl HistoryRepository,
    input: ComposeInput,
) -> anyhow::Result<()> {
    let frame = frames
        .get_frame(&input.frame)
        .await?
        .with_context(|| format!("frame not found: {}", input.frame))?;
    validate_under_overrides(&frame, &input.under)?;
    ensure!(input.count > 0, "compose count must be greater than zero");
    ensure!(
        input.count <= MAX_COMPOSE_COUNT,
        "compose count must not exceed {MAX_COMPOSE_COUNT}"
    );

    let base_seed = input.seed.unwrap_or_else(random_seed);
    let mut packets = Vec::with_capacity(input.count);

    // The recent set is independent of slot and packet; fetch it once if any
    // slot needs it. It is deliberately fixed for the whole invocation:
    // selections drawn by earlier packets in this batch do not join the
    // recent set for later ones, keeping every packet's draw distribution
    // identical and reproducible from `base_seed`.
    let recent = if frame.slots.iter().any(|s| s.avoid_recent) {
        history.recent_branch_ids(RECENT_LIMIT).await?
    } else {
        Default::default()
    };
    let empty_recent = std::collections::HashSet::new();

    // Candidate sets depend only on the slot and its `under` scope, both fixed
    // across the batch — fetch them once per slot rather than per packet.
    let mut slot_plans = Vec::with_capacity(frame.slots.len());
    for slot in &frame.slots {
        let under = override_for(&slot.name, &input.under).or_else(|| slot.under.clone());
        let query = SampleQuery::from_slot(slot, under);
        let params = SampleParams::from_slot(slot);
        let candidates = branches.find(query.clone()).await?;
        ensure!(
            candidates.len() >= params.count,
            "slot `{}` requires {} selection(s), but only {} candidate(s) match",
            slot.name,
            params.count,
            candidates.len()
        );
        slot_plans.push((slot, query, params, candidates));
    }

    for k in 0..input.count {
        let seed = base_seed.wrapping_add(k as u64);
        let mut rng = rng_from_seed(seed);

        let mut packet = SelectionPacket::new(Some(frame.name.clone()), Some(seed));
        packet.created_at = Some(now());
        packet.query = Some(compose_provenance(&frame.name, &input.under));

        for (slot, query, params, candidates) in &slot_plans {
            let slot_recent = if slot.avoid_recent {
                &recent
            } else {
                &empty_recent
            };
            let chosen = sample(candidates, query, params, slot_recent, &mut rng);
            for branch in chosen {
                packet.selections.push(Selection {
                    slot: Some(slot.name.clone()),
                    branch: branch.clone(),
                });
            }
        }
        packets.push(packet);
    }

    let rendered = unclip_io::render_packets(&packets, input.format)?;

    // Match `sample`'s persistence-first contract: anything emitted has already
    // been durably recorded, though a later output failure can leave a record
    // that its intended consumer did not receive.
    if !input.dry_run {
        let records = packets
            .iter()
            .map(|packet| packet_usage_record(Some(&frame.name), packet))
            .collect::<anyhow::Result<Vec<_>>>()?;
        history
            .save_packets_with_usages(&records, "compose")
            .await?;
    }
    crate::output::write_stdout(&rendered)?;
    Ok(())
}

/// Build compose's packet `query` provenance: the frame plus any `--under`
/// overrides, so a packet records everything needed to re-draw it.
fn compose_provenance(frame: &str, overrides: &[UnderOverride]) -> serde_json::Value {
    if overrides.is_empty() {
        serde_json::json!({ "frame": frame })
    } else {
        serde_json::json!({ "frame": frame, "under": overrides })
    }
}

/// Arguments for `replay`.
pub struct ReplayInput {
    pub file: std::path::PathBuf,
    pub seed: Option<u64>,
    pub format: Format,
    pub dry_run: bool,
}

/// `unclip replay <packet-file>` — re-run the sampling a packet records.
///
/// The packet's embedded `query` provenance supplies the filter (or the frame
/// and its `--under` overrides); its `seed` reproduces the same selections
/// unless `--seed` overrides it. Like `sample`/`compose`, a replay records
/// usage and persists the new packet unless `--dry-run` is passed.
pub async fn replay_cmd(
    branches: &impl BranchReader,
    frames: &impl unclip_store::FrameRepository,
    history: &impl HistoryRepository,
    input: ReplayInput,
) -> anyhow::Result<()> {
    use unclip_core::{PACKET_KIND, PACKET_VERSION};

    let text = unclip_io::read_text_file(&input.file, "packet file")?;
    let packet: SelectionPacket = serde_norway::from_str(&text)?;
    ensure!(
        packet.version == PACKET_VERSION,
        "packet version {} is unsupported; expected {PACKET_VERSION}",
        packet.version
    );
    ensure!(
        packet.kind == PACKET_KIND,
        "packet kind `{}` is invalid; expected `{PACKET_KIND}`",
        packet.kind
    );
    let seed = input.seed.or(packet.seed);

    match packet.frame {
        Some(frame_name) => {
            // Compose provenance: frame name plus optional under overrides.
            let under = packet
                .query
                .as_ref()
                .and_then(|q| q.get("under"))
                .map(|v| serde_json::from_value::<Vec<UnderOverride>>(v.clone()))
                .transpose()
                .context("packet has malformed `under` provenance")?
                .unwrap_or_default();
            compose_cmd(
                branches,
                frames,
                history,
                ComposeInput {
                    frame: frame_name,
                    under,
                    count: 1,
                    seed,
                    format: input.format,
                    dry_run: input.dry_run,
                },
            )
            .await
        }
        None => {
            // Sample provenance: one object carrying the query fields plus the
            // flattened sampling controls; parse both types from it.
            let provenance = packet
                .query
                .context("packet has no embedded query; cannot replay")?;
            let query: SampleQuery = serde_json::from_value(provenance.clone())
                .context("packet has malformed query provenance")?;
            let params: SampleParams = serde_json::from_value(provenance)
                .context("packet has malformed sampling-control provenance")?;
            run_sample(
                branches,
                history,
                query,
                params,
                seed,
                input.format,
                input.dry_run,
            )
            .await
        }
    }
}

fn override_for(slot_name: &str, overrides: &[UnderOverride]) -> Option<String> {
    // Slot-specific override wins; otherwise the first global override.
    overrides
        .iter()
        .find(|o| o.slot.as_deref() == Some(slot_name))
        .or_else(|| overrides.iter().find(|o| o.slot.is_none()))
        .map(|o| o.path.clone())
}

fn validate_under_overrides(frame: &Frame, overrides: &[UnderOverride]) -> anyhow::Result<()> {
    let mut seen_slots = HashSet::new();
    let mut saw_global = false;

    for override_ in overrides {
        match &override_.slot {
            Some(slot) => {
                ensure!(
                    frame.slot(slot).is_some(),
                    "frame `{}` has no slot `{slot}`",
                    frame.name
                );
                ensure!(
                    seen_slots.insert(slot),
                    "duplicate --under override for slot `{slot}`"
                );
            }
            None => {
                ensure!(!saw_global, "duplicate global --under override");
                saw_global = true;
            }
        }
    }
    Ok(())
}

/// `unclip export` — find branches by filter and render them.
///
/// JSONL has no document wrapper, so it streams: each page is rendered and
/// written as it loads instead of retaining the whole hydrated result set.
/// YAML/JSON produce a single wrapped document and still buffer (bounded by
/// the store's bulk-result ceiling).
pub async fn export_cmd(
    branches: &impl BranchReader,
    filter: FilterInput,
    format: Format,
) -> anyhow::Result<()> {
    let query = filter.into_query()?;
    match format {
        Format::Jsonl => {
            let mut after_path: Option<String> = None;
            loop {
                let page = branches
                    .find_page(&query, after_path.as_deref(), STREAM_PAGE_SIZE)
                    .await?;
                let done = (page.len() as u64) < STREAM_PAGE_SIZE;
                after_path = page.last().map(|branch| branch.path.clone());
                crate::output::write_stdout(&unclip_io::render_branches(&page, format)?)?;
                if done {
                    return Ok(());
                }
            }
        }
        Format::Yaml | Format::Json => {
            // `find_all` pages in path order, so the result needs no re-sort.
            let matched = branches.find_all(query).await?;
            crate::output::write_stdout(&unclip_io::render_branches(&matched, format)?)?;
            Ok(())
        }
    }
}

/// `unclip used <path>`.
pub async fn used_cmd(
    branches: &impl BranchReader,
    history: &impl HistoryRepository,
    path: &str,
) -> anyhow::Result<()> {
    let branch = branches
        .get(path)
        .await?
        .with_context(|| format!("branch not found: {path}"))?;
    let id = branch.id.context("branch has no id")?;
    let summary = history.usage_for(id).await?;
    crate::output::outln!("{path}\tused {} time(s)", summary.count);
    if let Some(last) = summary.last_used {
        crate::output::outln!("last used: {last}");
    }
    Ok(())
}

/// `unclip stats` — aggregate usage over a filter.
pub async fn stats_cmd(
    branches: &impl BranchReader,
    history: &impl HistoryRepository,
    filter: FilterInput,
) -> anyhow::Result<()> {
    let query = filter.into_query()?;

    // Aggregate page by page: the counters are all that is retained, so the
    // command is bounded by the page size rather than the bulk-result ceiling.
    let mut matched = 0u64;
    let mut total_uses = 0u64;
    let mut unused = 0u64;
    let mut after_path: Option<String> = None;
    loop {
        let page = branches
            .find_page(&query, after_path.as_deref(), STREAM_PAGE_SIZE)
            .await?;
        let done = (page.len() as u64) < STREAM_PAGE_SIZE;
        after_path = page.last().map(|branch| branch.path.clone());
        matched += page.len() as u64;

        let ids: Vec<i64> = page.iter().filter_map(|b| b.id).collect();
        let summaries = history.usage_summaries(&ids).await?;
        for id in &ids {
            let count = summaries.get(id).map(|s| s.count).unwrap_or(0);
            total_uses += count;
            if count == 0 {
                unused += 1;
            }
        }
        if done {
            break;
        }
    }
    crate::output::outln!("branches: {matched}");
    crate::output::outln!("total uses: {total_uses}");
    crate::output::outln!("unused: {unused}");
    Ok(())
}

/// `unclip stale` — branches matching a filter, least-used first.
pub async fn stale_cmd(
    branches: &impl BranchReader,
    history: &impl HistoryRepository,
    filter: FilterInput,
) -> anyhow::Result<()> {
    let query = filter.into_query()?;
    let matched = branches.find_all(query).await?;

    let ids: Vec<i64> = matched.iter().filter_map(|b| b.id).collect();
    let summaries = history.usage_summaries(&ids).await?;

    let mut rows = Vec::with_capacity(matched.len());
    for branch in matched {
        let summary = branch
            .id
            .and_then(|id| summaries.get(&id).cloned())
            .unwrap_or_default();
        rows.push((branch.path, summary.count, summary.last_used));
    }
    // Least used first; ties broken by oldest last-used (None sorts first).
    rows.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));

    if rows.is_empty() {
        crate::output::errln!("(no matching branches)");
        return Ok(());
    }
    for (path, count, last_used) in rows {
        crate::output::outln!(
            "{path}\tuses={count}\tlast={}",
            last_used.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// Build the packet `query` provenance value: the filter plus the sampling
/// controls, flattened into one object so a packet records exactly how it was
/// drawn (count/weighted/avoid_recent) alongside what it was drawn from.
fn query_provenance(
    query: &SampleQuery,
    params: &SampleParams,
) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(query)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("count".into(), params.count.into());
        obj.insert("weighted".into(), params.weighted.into());
        obj.insert("avoid_recent".into(), params.avoid_recent.into());
    }
    Ok(value)
}

/// Build the record that persists a packet and its usage rows atomically.
fn packet_usage_record(
    frame_name: Option<&str>,
    packet: &SelectionPacket,
) -> anyhow::Result<PacketUsageRecord> {
    Ok(PacketUsageRecord {
        // Packet ids are seed-independent so repeated deterministic draws do
        // not collide in the packet store.
        id: random_packet_id(),
        frame_name: frame_name.map(str::to_string),
        seed: packet.seed,
        query_json: packet.query.as_ref().map(serde_json::Value::to_string),
        packet_json: serde_json::to_string(packet)?,
        branch_ids: packet
            .selections
            .iter()
            .filter_map(|selection| selection.branch.id)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn filter() -> FilterInput {
        FilterInput {
            under: None,
            require_o2o: Vec::new(),
            avoid_o2o: Vec::new(),
            require_o2m: Vec::new(),
            prefer_o2m: Vec::new(),
            avoid_o2m: Vec::new(),
        }
    }

    #[test]
    fn into_query_groups_flags_by_kind() {
        let q = FilterInput {
            under: Some("/ikebukuro".into()),
            require_o2o: vec![("place".into(), "cafe".into())],
            avoid_o2o: vec![("mood".into(), "tense".into())],
            require_o2m: vec![
                ("tag".into(), "rain".into()),
                ("tag".into(), "night".into()),
            ],
            prefer_o2m: vec![("density".into(), "crowded".into())],
            avoid_o2m: vec![("tag".into(), "sunny".into())],
        }
        .into_query()
        .unwrap();

        assert_eq!(q.under.as_deref(), Some("/ikebukuro"));
        assert_eq!(q.require_o2o.get("place").map(String::as_str), Some("cafe"));
        assert_eq!(q.avoid_o2o.get("mood"), Some(&vec!["tense".to_string()]));
        // Repeated o2m names accumulate into a set of values under one name.
        assert_eq!(
            q.require_o2m.get("tag"),
            Some(&vec!["rain".to_string(), "night".to_string()])
        );
        assert_eq!(
            q.prefer_o2m.get("density"),
            Some(&vec!["crowded".to_string()])
        );
        assert_eq!(q.avoid_o2m.get("tag"), Some(&vec!["sunny".to_string()]));
    }

    #[test]
    fn into_query_rejects_duplicate_require_o2o_name() {
        let mut f = filter();
        f.require_o2o = vec![
            ("place".into(), "cafe".into()),
            ("place".into(), "park".into()),
        ];
        assert!(f.into_query().is_err());
    }

    #[test]
    fn into_query_accumulates_repeated_avoid_o2o_names() {
        // Excluding several values of one o2o name is legitimate (a branch can
        // only carry one of them, so avoiding many just widens the exclusion).
        let mut f = filter();
        f.avoid_o2o = vec![("m".into(), "a".into()), ("m".into(), "b".into())];
        let q = f.into_query().unwrap();
        assert_eq!(
            q.avoid_o2o.get("m"),
            Some(&vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn into_query_rejects_invalid_scope() {
        let mut f = filter();
        f.under = Some("relative/path".into());
        assert!(f.into_query().is_err());
    }

    #[test]
    fn parse_under_override_distinguishes_slot_specific_and_global() {
        let scoped = parse_under_override("place:/ikebukuro/station").unwrap();
        assert_eq!(scoped.slot.as_deref(), Some("place"));
        assert_eq!(scoped.path, "/ikebukuro/station");

        let global = parse_under_override("/ikebukuro").unwrap();
        assert_eq!(global.slot, None);
        assert_eq!(global.path, "/ikebukuro");

        assert!(parse_under_override(":/x").is_err());
        assert!(parse_under_override("place:relative").is_err());
    }

    #[test]
    fn override_for_prefers_slot_specific_then_global() {
        let overrides = vec![
            UnderOverride {
                slot: None,
                path: "/global".into(),
            },
            UnderOverride {
                slot: Some("place".into()),
                path: "/place-scope".into(),
            },
        ];
        // A slot-specific override wins over the global one.
        assert_eq!(
            override_for("place", &overrides).as_deref(),
            Some("/place-scope")
        );
        // A slot with no specific override falls back to the global.
        assert_eq!(override_for("mood", &overrides).as_deref(), Some("/global"));
        // No overrides at all yields None.
        assert_eq!(override_for("place", &[]), None);
    }

    #[test]
    fn under_overrides_reject_unknown_slots_and_duplicates() {
        let frame = Frame {
            name: "story".into(),
            description: None,
            slots: Vec::new(),
        };
        assert!(validate_under_overrides(
            &frame,
            &[UnderOverride {
                slot: Some("missing".into()),
                path: "/x".into(),
            }],
        )
        .is_err());
        assert!(validate_under_overrides(
            &frame,
            &[
                UnderOverride {
                    slot: None,
                    path: "/x".into(),
                },
                UnderOverride {
                    slot: None,
                    path: "/y".into(),
                },
            ],
        )
        .is_err());
    }

    #[test]
    fn query_provenance_flattens_sampling_controls() {
        let query = SampleQuery {
            under: Some("/x".into()),
            ..Default::default()
        };
        let params = SampleParams {
            count: 3,
            weighted: true,
            avoid_recent: true,
        };
        let value = query_provenance(&query, &params).unwrap();
        let obj = value.as_object().expect("provenance is a JSON object");
        // The filter is carried verbatim...
        assert_eq!(obj.get("under").and_then(|v| v.as_str()), Some("/x"));
        // ...alongside the sampling controls flattened onto the same object.
        assert_eq!(obj.get("count").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(obj.get("weighted").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            obj.get("avoid_recent").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
