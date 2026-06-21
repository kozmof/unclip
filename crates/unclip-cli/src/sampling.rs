//! `sample`, `compose`, and usage-reporting (`used`/`stats`/`stale`) handlers.

use anyhow::Context;
use unclip_core::{Branch, SampleQuery, Selection, SelectionPacket};
use unclip_io::Format;
use unclip_sample::{random_packet_id, random_seed, rng_from_seed, sample};
use unclip_store::{now, BranchRepository, PacketRecord, SeaOrmHistoryRepository};

/// How many recent usage rows define the "recently used" set.
const RECENT_LIMIT: u64 = 50;

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
    pub fn into_query(self, count: usize, weighted: bool, avoid_recent: bool) -> anyhow::Result<SampleQuery> {
        let mut q = SampleQuery {
            under: self.under,
            count,
            weighted,
            avoid_recent,
            ..Default::default()
        };
        crate::commands::merge_o2o(&mut q.require_o2o, self.require_o2o)?;
        for (name, value) in self.avoid_o2o {
            q.avoid_o2o.insert(name, value);
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
    branches: &impl BranchRepository,
    history: &SeaOrmHistoryRepository,
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

    let query = filter.into_query(count, weighted, avoid_recent)?;
    let candidates = branches.find(query.clone()).await?;

    let recent = if avoid_recent {
        history.recent_branch_ids(RECENT_LIMIT).await?
    } else {
        Default::default()
    };

    let seed = seed.unwrap_or_else(random_seed);
    let mut rng = rng_from_seed(seed);
    let chosen: Vec<Branch> = sample(&candidates, &query, &recent, &mut rng)
        .into_iter()
        .cloned()
        .collect();

    let mut packet = SelectionPacket::new(None, Some(seed));
    packet.created_at = Some(now());
    packet.query = Some(serde_json::to_value(&query)?);
    packet.selections = chosen
        .iter()
        .map(|b| Selection {
            slot: None,
            branch: b.clone(),
        })
        .collect();

    print!("{}", unclip_io::render_packet(&packet, format)?);

    if !dry_run {
        let id = random_packet_id();
        persist_packet(history, &id, None, "sample", &packet).await?;
    }
    Ok(())
}

/// clap value parser for `--format`.
pub fn parse_format(s: &str) -> anyhow::Result<Format> {
    s.parse()
}

/// A `--under` override for compose: a slot-specific or global scope.
///
/// `slot: None` is a global override applied to any slot without a more
/// specific one.
#[derive(Debug, Clone)]
pub struct UnderOverride {
    pub slot: Option<String>,
    pub path: String,
}

/// Parse `slot:/path` (slot-specific) or `/path` (global) overrides.
pub fn parse_under_override(raw: &str) -> anyhow::Result<UnderOverride> {
    match raw.split_once(':') {
        Some((slot, path)) if !slot.is_empty() => Ok(UnderOverride {
            slot: Some(slot.to_string()),
            path: path.to_string(),
        }),
        _ => Ok(UnderOverride {
            slot: None,
            path: raw.to_string(),
        }),
    }
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
    branches: &impl BranchRepository,
    frames: &impl unclip_store::FrameRepository,
    history: &SeaOrmHistoryRepository,
    input: ComposeInput,
) -> anyhow::Result<()> {
    let frame = frames
        .get_frame(&input.frame)
        .await?
        .with_context(|| format!("frame not found: {}", input.frame))?;

    let base_seed = input.seed.unwrap_or_else(random_seed);
    let mut packets = Vec::with_capacity(input.count);

    // The recent set is independent of slot and packet; fetch it once if any
    // slot needs it.
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
        let candidates = branches.find(query.clone()).await?;
        slot_plans.push((slot, query, candidates));
    }

    for k in 0..input.count {
        let seed = base_seed.wrapping_add(k as u64);
        let mut rng = rng_from_seed(seed);

        let mut packet = SelectionPacket::new(Some(frame.name.clone()), Some(seed));
        packet.created_at = Some(now());
        packet.query = Some(serde_json::json!({ "frame": frame.name }));

        for (slot, query, candidates) in &slot_plans {
            let slot_recent = if slot.avoid_recent { &recent } else { &empty_recent };
            let chosen = sample(candidates, query, slot_recent, &mut rng);
            for branch in chosen {
                packet.selections.push(Selection {
                    slot: Some(slot.name.clone()),
                    branch: branch.clone(),
                });
            }
        }
        packets.push(packet);
    }

    print!("{}", unclip_io::render_packets(&packets, input.format)?);

    if !input.dry_run {
        for packet in &packets {
            // Packet id is random (seed-independent) so re-runs do not collide.
            let id = random_packet_id();
            persist_packet(history, &id, Some(&frame.name), "compose", packet).await?;
        }
    }
    Ok(())
}

fn override_for(slot_name: &str, overrides: &[UnderOverride]) -> Option<String> {
    // Slot-specific override wins; otherwise the first global override.
    overrides
        .iter()
        .find(|o| o.slot.as_deref() == Some(slot_name))
        .or_else(|| overrides.iter().find(|o| o.slot.is_none()))
        .map(|o| o.path.clone())
}

/// `unclip export` — find branches by filter and render them.
pub async fn export_cmd(
    branches: &impl BranchRepository,
    filter: FilterInput,
    format: Format,
) -> anyhow::Result<()> {
    let query = filter.into_query(usize::MAX, false, false)?;
    let mut matched = branches.find(query).await?;
    matched.sort_by(|a, b| a.path.cmp(&b.path));
    print!("{}", unclip_io::render_branches(&matched, format)?);
    Ok(())
}

/// `unclip used <path>`.
pub async fn used_cmd(
    branches: &impl BranchRepository,
    history: &SeaOrmHistoryRepository,
    path: &str,
) -> anyhow::Result<()> {
    let branch = branches
        .get(path)
        .await?
        .with_context(|| format!("branch not found: {path}"))?;
    let id = branch.id.context("branch has no id")?;
    let summary = history.usage_for(id).await?;
    println!("{path}\tused {} time(s)", summary.count);
    if let Some(last) = summary.last_used {
        println!("last used: {last}");
    }
    Ok(())
}

/// `unclip stats` — aggregate usage over a filter.
pub async fn stats_cmd(
    branches: &impl BranchRepository,
    history: &SeaOrmHistoryRepository,
    filter: FilterInput,
) -> anyhow::Result<()> {
    let query = filter.into_query(usize::MAX, false, false)?;
    let matched = branches.find(query).await?;

    let ids: Vec<i64> = matched.iter().filter_map(|b| b.id).collect();
    let summaries = history.usage_summaries(&ids).await?;

    let mut total_uses = 0u64;
    let mut unused = 0u64;
    for id in &ids {
        let count = summaries.get(id).map(|s| s.count).unwrap_or(0);
        total_uses += count;
        if count == 0 {
            unused += 1;
        }
    }
    println!("branches: {}", matched.len());
    println!("total uses: {total_uses}");
    println!("unused: {unused}");
    Ok(())
}

/// `unclip stale` — branches matching a filter, least-used first.
pub async fn stale_cmd(
    branches: &impl BranchRepository,
    history: &SeaOrmHistoryRepository,
    filter: FilterInput,
) -> anyhow::Result<()> {
    let query = filter.into_query(usize::MAX, false, false)?;
    let matched = branches.find(query).await?;

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
        eprintln!("(no matching branches)");
        return Ok(());
    }
    for (path, count, _) in rows {
        println!("{path}\tuses={count}");
    }
    Ok(())
}

/// Persist a packet and the usage rows for its selected branches in one
/// transaction, so history can never record a packet without its usages.
async fn persist_packet(
    history: &SeaOrmHistoryRepository,
    id: &str,
    frame_name: Option<&str>,
    command: &str,
    packet: &SelectionPacket,
) -> anyhow::Result<()> {
    let packet_json = serde_json::to_string(packet)?;
    let query_json = packet.query.as_ref().map(|q| q.to_string());
    let branch_ids: Vec<i64> = packet
        .selections
        .iter()
        .filter_map(|s| s.branch.id)
        .collect();
    history
        .save_packet_with_usages(
            PacketRecord {
                id,
                frame_name,
                seed: packet.seed,
                query_json: query_json.as_deref(),
                packet_json: &packet_json,
            },
            command,
            &branch_ids,
        )
        .await
}

