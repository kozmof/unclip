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

#![forbid(unsafe_code)]

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

    // Preference multiplication can overflow even when the persisted weight is
    // finite. Saturate so every score remains a valid sampling input.
    if s.is_finite() {
        s.max(MIN_SCORE)
    } else {
        f64::MAX
    }
}

/// A fixed-size weighted reservoir (Efraimidis–Spirakis "A-Res").
///
/// Offer every candidate exactly once, in a stable order, with its positive
/// score; the kept set is distributed exactly like sequential weighted
/// sampling without replacement, while holding only `take` candidates in
/// memory. This is what lets `sample` stream candidates page by page instead
/// of hydrating the whole filtered archive first.
///
/// Each `offer` consumes exactly one RNG draw, so for a fixed seed the result
/// depends only on the candidate sequence, not on page boundaries.
pub struct Reservoir {
    take: usize,
    /// `(key, branch)` pairs; keys are `u^(1/score)` with `u ~ U(0,1)`.
    kept: Vec<(f64, Branch)>,
}

impl Reservoir {
    pub fn new(take: usize) -> Self {
        Self {
            take,
            kept: Vec::with_capacity(take),
        }
    }

    /// Offer one candidate with its score (must be positive; [`score`]
    /// guarantees that via its `MIN_SCORE` floor).
    pub fn offer(&mut self, branch: Branch, score: f64, rng: &mut StdRng) {
        // Always draw, even when the candidate cannot be kept, so the RNG
        // stream stays aligned with the candidate sequence.
        let u: f64 = rng.gen();
        if self.take == 0 {
            return;
        }
        let key = u.powf(1.0 / score);
        if self.kept.len() < self.take {
            self.kept.push((key, branch));
            return;
        }
        let (min_index, min_key) = self
            .kept
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (i, *k))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("reservoir with take > 0 is non-empty here");
        if key > min_key {
            self.kept[min_index] = (key, branch);
        }
    }

    /// The selected branches, highest key first (the equivalent of draw order).
    pub fn into_branches(mut self) -> Vec<Branch> {
        self.kept.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.kept.into_iter().map(|(_, branch)| branch).collect()
    }
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

    // (index, score) pool we draw from and shrink as we pick. Scores are
    // normalized once by the pool's largest score: each is then ≤ 1, so any
    // partial sum is bounded by the pool length and cannot overflow, and
    // draws need no per-iteration re-normalization (scaling every score by
    // one constant leaves the draw proportions unchanged).
    let mut pool: Vec<(usize, f64)> = candidates
        .iter()
        .enumerate()
        .map(|(i, b)| (i, score(b, query, params, recent_ids)))
        .collect();
    let max_score = pool.iter().map(|(_, s)| *s).fold(0.0, f64::max);
    for (_, s) in &mut pool {
        *s /= max_score;
    }

    let mut chosen = Vec::with_capacity(take);
    for _ in 0..take {
        let total: f64 = pool.iter().map(|(_, s)| *s).sum();
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
    fn reservoir_is_deterministic_capped_and_unique() {
        let candidates: Vec<Branch> = (0..10).map(|i| branch(&format!("/b{i}"), i, 1.0)).collect();

        let draw = |seed: u64, take: usize| {
            let mut rng = rng_from_seed(seed);
            let mut reservoir = Reservoir::new(take);
            for candidate in &candidates {
                reservoir.offer(candidate.clone(), 1.0, &mut rng);
            }
            reservoir
                .into_branches()
                .into_iter()
                .map(|b| b.path)
                .collect::<Vec<_>>()
        };

        // Same seed, same selection; distinct entries; capped at candidates.
        let a = draw(42, 3);
        assert_eq!(a, draw(42, 3));
        assert_eq!(a.len(), 3);
        assert_eq!(a.iter().collect::<HashSet<_>>().len(), 3);
        assert_eq!(draw(1, 20).len(), 10);
        assert!(draw(1, 0).is_empty());
    }

    #[test]
    fn reservoir_prefers_heavier_scores() {
        // With scores this far apart the heavy candidate should essentially
        // always win. Seeds are fixed, so the assertion is deterministic.
        let heavy = branch("/heavy", 1, 0.0);
        let light = branch("/light", 2, 0.0);
        let mut heavy_wins = 0;
        for seed in 0..40 {
            let mut rng = rng_from_seed(seed);
            let mut reservoir = Reservoir::new(1);
            reservoir.offer(heavy.clone(), 1_000.0, &mut rng);
            reservoir.offer(light.clone(), 1.0, &mut rng);
            if reservoir.into_branches()[0].path == "/heavy" {
                heavy_wins += 1;
            }
        }
        assert!(heavy_wins >= 38, "heavy won only {heavy_wins}/40 draws");
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
    fn extreme_finite_weights_do_not_overflow_sampling() {
        let candidates = vec![branch("/a", 1, f64::MAX), branch("/b", 2, f64::MAX)];
        let mut rng = rng_from_seed(1);
        let mut weighted = params(1);
        weighted.weighted = true;

        let chosen = sample(
            &candidates,
            &SampleQuery::default(),
            &weighted,
            &HashSet::new(),
            &mut rng,
        );
        assert_eq!(chosen.len(), 1);
    }

    #[test]
    fn preference_overflow_saturates_to_a_finite_score() {
        let mut candidate = branch("/a", 1, f64::MAX);
        candidate.o2m.insert("tag".into(), vec!["match".into()]);
        let mut query = SampleQuery::default();
        query.prefer_o2m.insert("tag".into(), vec!["match".into()]);
        let mut weighted = params(1);
        weighted.weighted = true;

        assert!(score(&candidate, &query, &weighted, &HashSet::new()).is_finite());
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
