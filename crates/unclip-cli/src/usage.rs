//! Usage reporting: `used`, `stats`, `stale`.
//!
//! These read the usage history that `sample` and `compose` write. They share
//! [`crate::sampling::FilterInput`] with the sampling commands but never draw,
//! so they live apart from the sampling pipeline itself.

use anyhow::Context;
use unclip_store::{BranchReader, HistoryRepository, PageCursor};

use crate::sampling::FilterInput;

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
    let mut pages = PageCursor::new();
    while let Some(page) = pages.next(branches, &query).await? {
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
