//! unclip-sample — sampling pipeline over the branch archive.
//!
//! The sampler operates on already-filtered candidates (hard scope/o2o/o2m
//! filters are applied by the store). It scores each candidate and draws
//! `count` of them by weighted random selection without replacement, using a
//! seeded RNG so results are reproducible.
//!
//! ```text
//! score = weight × prefer_o2m_bonus × recent_usage_penalty
//! ```
//!
//! `novelty_bonus` (embedding-based) is intentionally out of MVP scope.

use std::collections::HashSet;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use unclip_core::{Branch, SampleParams, SampleQuery};

/// Each matched `prefer_o2m` value multiplies the score by this much.
const PREFER_BONUS_PER_MATCH: f64 = 0.5;
/// Multiplier applied to a recently-used branch when `avoid_recent` is set.
const RECENT_PENALTY: f64 = 0.25;
/// Floor so a candidate with weight 0 can still be chosen if nothing else is.
const MIN_SCORE: f64 = 1e-6;

/// Build a seeded RNG.
pub fn rng_from_seed(seed: u64) -> StdRng {
    StdRng::seed_from_u64(seed)
}

/// Draw a fresh random seed from system entropy.
pub fn random_seed() -> u64 {
    rand::thread_rng().gen()
}

/// Generate a random packet id (128-bit, hex) from system entropy.
///
/// The id is deliberately independent of the sampling seed: re-running a
/// `sample`/`compose` with a fixed `--seed` reproduces the *selections*, but
/// each run draws a fresh packet id so persisting it cannot collide on the
/// `selection_packets` primary key.
pub fn random_packet_id() -> String {
    format!("{:032x}", rand::thread_rng().gen::<u128>())
}

/// Score a single candidate against the query and recency set.
pub fn score(
    branch: &Branch,
    query: &SampleQuery,
    params: &SampleParams,
    recent_ids: &HashSet<i64>,
) -> f64 {
    let mut s = if params.weighted {
        branch.weight.max(0.0)
    } else {
        1.0
    };

    let mut matches = 0usize;
    for (name, values) in &query.prefer_o2m {
        if let Some(branch_values) = branch.o2m.get(name) {
            matches += values.iter().filter(|v| branch_values.contains(v)).count();
        }
    }
    s *= 1.0 + PREFER_BONUS_PER_MATCH * matches as f64;

    if params.avoid_recent {
        if let Some(id) = branch.id {
            if recent_ids.contains(&id) {
                s *= RECENT_PENALTY;
            }
        }
    }

    s.max(MIN_SCORE)
}

/// Select up to `params.count` branches from `candidates` by weighted random
/// selection without replacement. Returns references into `candidates`.
pub fn sample<'a>(
    candidates: &'a [Branch],
    query: &SampleQuery,
    params: &SampleParams,
    recent_ids: &HashSet<i64>,
    rng: &mut StdRng,
) -> Vec<&'a Branch> {
    let take = params.count.min(candidates.len());
    if take == 0 {
        return Vec::new();
    }

    // (index, score) pool we draw from and shrink as we pick.
    let mut pool: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(i, b)| (i, score(b, query, params, recent_ids)))
        .collect();

    let mut chosen = Vec::with_capacity(take);
    for _ in 0..take {
        let total: f64 = pool.iter().map(|(_, s)| *s).sum();
        // total >= MIN_SCORE * pool.len() > 0, so the draw is well-defined.
        let mut r = rng.gen_range(0.0..total);
        let mut picked = pool.len() - 1;
        for (idx, (_, s)) in pool.iter().enumerate() {
            if r < *s {
                picked = idx;
                break;
            }
            r -= *s;
        }
        let (branch_index, _) = pool.swap_remove(picked);
        chosen.push(&candidates[branch_index]);
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn branch(path: &str, id: i64, weight: f64) -> Branch {
        let mut b = Branch::new(path);
        b.id = Some(id);
        b.weight = weight;
        b
    }

    fn params(count: usize) -> SampleParams {
        SampleParams {
            count,
            ..Default::default()
        }
    }

    #[test]
    fn deterministic_for_same_seed() {
        let candidates: Vec<Branch> = (0..10).map(|i| branch(&format!("/b{i}"), i, 1.0)).collect();
        let q = SampleQuery::default();
        let p = params(3);
        let recent = HashSet::new();

        let a = {
            let mut rng = rng_from_seed(42);
            sample(&candidates, &q, &p, &recent, &mut rng)
                .iter()
                .map(|b| b.path.clone())
                .collect::<Vec<_>>()
        };
        let b = {
            let mut rng = rng_from_seed(42);
            sample(&candidates, &q, &p, &recent, &mut rng)
                .iter()
                .map(|b| b.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(a, b);
        assert_eq!(a.len(), 3);
        // No duplicates (without replacement).
        let unique: HashSet<_> = a.iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn count_capped_at_candidates() {
        let candidates = vec![branch("/a", 1, 1.0), branch("/b", 2, 1.0)];
        let mut rng = rng_from_seed(1);
        let chosen = sample(
            &candidates,
            &SampleQuery::default(),
            &params(5),
            &HashSet::new(),
            &mut rng,
        );
        assert_eq!(chosen.len(), 2);
    }

    #[test]
    fn prefer_bonus_increases_score() {
        let mut preferred = branch("/p", 1, 1.0);
        preferred
            .o2m
            .insert("density".into(), vec!["crowded".into()]);
        let plain = branch("/q", 2, 1.0);

        let mut q = SampleQuery::default();
        let mut prefer = BTreeMap::new();
        prefer.insert("density".to_string(), vec!["crowded".to_string()]);
        q.prefer_o2m = prefer;

        let p = params(1);
        let recent = HashSet::new();
        assert!(score(&preferred, &q, &p, &recent) > score(&plain, &q, &p, &recent));
    }

    #[test]
    fn recent_penalty_applies_only_when_avoid_recent() {
        let b = branch("/x", 7, 1.0);
        let recent: HashSet<i64> = [7].into_iter().collect();

        let q = SampleQuery::default();
        let mut p = params(1);
        assert_eq!(
            score(&b, &q, &p, &recent),
            score(&b, &q, &p, &HashSet::new())
        );

        p.avoid_recent = true;
        assert!(score(&b, &q, &p, &recent) < score(&b, &q, &p, &HashSet::new()));
    }
}
