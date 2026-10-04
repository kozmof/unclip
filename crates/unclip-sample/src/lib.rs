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

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::rc::Rc;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use unclip_core::{Branch, SampleParams, SampleQuery};

/// Each matched `prefer_o2m` value multiplies the score by this much.
const PREFER_BONUS_PER_MATCH: f64 = 0.5;
/// Multiplier applied to a recently-used branch when `avoid_recent` is set.
const RECENT_PENALTY: f64 = 0.25;
/// Floor so a candidate with weight 0 can still be chosen if nothing else is.
const MIN_SCORE: f64 = 1e-6;
/// Ceiling that keeps a dominant score from collapsing into a tie.
///
/// [`Reservoir`] keys a candidate as `u^(1/score)`. Once `score` reaches about
/// `1e30`, `1/score` is small enough that `u^(1/score)` rounds to exactly `1.0`
/// for *every* `u`, so all such candidates hold the identical key and the
/// strictly-greater comparison in `offer` lets whichever arrived first hold its
/// slot against all of them. That turns weighted sampling into first-come order
/// precisely among the candidates that should dominate it.
///
/// At `1e12` the keys stay distinct, and nothing below the ceiling changes: the
/// gap between `1e12` and a larger score was never representable in the key
/// anyway. Saturating here rather than at [`f64::MAX`] costs no ordering that
/// f64 could express.
const MAX_SCORE: f64 = 1e12;

/// The RNG every seeded selection draws from.
///
/// Named as ChaCha12 rather than `rand::rngs::StdRng`. `StdRng` is ChaCha12
/// today, but rand documents its algorithm as free to change between releases,
/// and a packet's recorded seed is only replayable while the algorithm behind
/// it holds still. Naming the algorithm makes changing it a visible decision
/// instead of a side effect of a dependency bump.
pub type SampleRng = ChaCha12Rng;

/// Build a seeded RNG.
pub fn rng_from_seed(seed: u64) -> SampleRng {
    SampleRng::seed_from_u64(seed)
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
    // finite. Saturate so every score remains a valid sampling input, and clamp
    // to a ceiling the reservoir key can still tell apart — see [`MAX_SCORE`].
    if s.is_finite() {
        s.clamp(MIN_SCORE, MAX_SCORE)
    } else {
        MAX_SCORE
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
    /// Kept candidates as a min-heap on the reservoir key, so the one the next
    /// candidate must beat is always at the top.
    ///
    /// This was a `Vec` scanned linearly for its minimum on every offer, which
    /// made a full pass `O(candidates × take)`; the heap makes it
    /// `O(candidates × log take)`. See [`Keyed`] for the one behaviour that is
    /// specified rather than inherited.
    kept: BinaryHeap<Keyed>,
    /// How many offers have been accepted, stamped onto each kept candidate to
    /// break key ties by arrival.
    accepted: u64,
}

/// One kept candidate, ordered smallest-key-first so a max-heap yields the
/// candidate a new offer has to beat.
///
/// # Ties
///
/// Equal keys are broken by arrival: among several equally-smallest keys, the
/// one accepted earliest is evicted first. A heap's own choice among equal
/// elements is unspecified, so without this rule a seeded run would not be
/// reproducible at all.
///
/// This *states* a rule the previous implementation only had by accident. That
/// one scanned a `Vec` with `min_by`, which returns the first of several equal
/// minima — first by vector index, and a replacement wrote the incoming
/// candidate into the evicted slot, so index order stopped matching arrival
/// order after the first eviction. Both rules are arbitrary among tied keys and
/// neither changes the sampling distribution: A-Res depends only on retaining
/// the `take` largest keys, and exactly-equal keys are interchangeable in that
/// set. But they can pick different branches, so a seed that produced a packet
/// containing a tied candidate may now produce its twin. Ties are rare by
/// construction — [`MAX_SCORE`] exists to keep keys distinct — and require two
/// `u^(1/score)` draws to collide bit-for-bit.
struct Keyed {
    key: f64,
    /// Acceptance order, not offer order: a rejected offer never takes a number,
    /// so this counts only candidates that have occupied a slot.
    sequence: u64,
    branch: Branch,
}

impl PartialEq for Keyed {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Keyed {}

impl Ord for Keyed {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed on both fields: smallest key on top, and among equal keys
        // the earliest arrival on top.
        other
            .key
            .total_cmp(&self.key)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for Keyed {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Reservoir {
    pub fn new(take: usize) -> Self {
        Self {
            take,
            kept: BinaryHeap::with_capacity(take),
            accepted: 0,
        }
    }

    /// Offer one candidate with its score (must be positive; [`score`]
    /// guarantees that via its `MIN_SCORE` floor).
    pub fn offer(&mut self, branch: Branch, score: f64, rng: &mut SampleRng) {
        // Always draw, even when the candidate cannot be kept, so the RNG
        // stream stays aligned with the candidate sequence.
        let u: f64 = rng.gen();
        if self.take == 0 {
            return;
        }
        let key = u.powf(1.0 / score);
        let sequence = self.accepted;
        if self.kept.len() < self.take {
            self.accepted += 1;
            self.kept.push(Keyed {
                key,
                sequence,
                branch,
            });
            return;
        }
        let mut smallest = self
            .kept
            .peek_mut()
            .expect("reservoir with take > 0 is non-empty here");
        // Strictly greater, so a tie leaves the incumbent in place — the rule
        // `MAX_SCORE` is documented against.
        if key > smallest.key {
            self.accepted += 1;
            *smallest = Keyed {
                key,
                sequence,
                branch,
            };
        }
        // Dropping the `PeekMut` restores the heap invariant.
    }

    /// The selected branches, highest key first (the equivalent of draw order).
    pub fn into_branches(self) -> Vec<Branch> {
        let mut kept = self.kept.into_vec();
        // Ties resolve by arrival for the same reason they do in the heap: the
        // emitted order must not depend on where a candidate landed in it.
        kept.sort_by(|a, b| {
            b.key
                .total_cmp(&a.key)
                .then_with(|| a.sequence.cmp(&b.sequence))
        });
        kept.into_iter().map(|keyed| keyed.branch).collect()
    }
}

/// Select up to `params.count` branches from `candidates` by weighted random
/// selection without replacement. Returns `Rc` clones of the chosen
/// candidates (a refcount bump, not a deep copy), so a caller that draws
/// repeatedly from the same shared pool — e.g. `compose`, once per output
/// packet — does not pay for a full `Branch` clone on every selection.
///
/// This draws from a shrinking pool, while [`Reservoir`] keys each candidate.
/// The two select with the same distribution (the `equivalence` tests hold
/// them to it) but consume the RNG differently, so one seed picks different
/// branches through each. A seed is reproducible only through the path that
/// recorded it: `sample` packets through the reservoir, `compose` through this.
pub fn sample(
    candidates: &[Rc<Branch>],
    query: &SampleQuery,
    params: &SampleParams,
    recent_ids: &HashSet<i64>,
    rng: &mut SampleRng,
) -> Vec<Rc<Branch>> {
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
        chosen.push(Rc::clone(&candidates[branch_index]));
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Packets recorded while sampling went through `StdRng` must replay
    /// unchanged now that the algorithm is named directly.
    #[test]
    fn sample_rng_matches_the_std_rng_it_replaced() {
        for seed in [0, 1, 7, u64::MAX] {
            let mut ours = rng_from_seed(seed);
            let mut std = rand::rngs::StdRng::seed_from_u64(seed);
            for _ in 0..64 {
                assert_eq!(ours.gen::<u64>(), std.gen::<u64>());
            }
        }
    }

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
        let candidates: Vec<Rc<Branch>> = (0..10)
            .map(|i| Rc::new(branch(&format!("/b{i}"), i, 1.0)))
            .collect();
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

    /// Tied keys evict by arrival, and the rule holds after an eviction has
    /// already shuffled the heap.
    ///
    /// Equal keys are forced here the direct way — `offer` takes the score
    /// already scored, so an infinite one makes `u^(1/score)` exactly `1.0` for
    /// every draw. [`score`] clamps to [`MAX_SCORE`] before this point, which
    /// is what keeps keys distinct in practice; this test is about what happens
    /// when they nonetheless collide. Without a stated tie-break the heap could
    /// evict either candidate, so this pins the rule rather than merely
    /// observing today's output.
    #[test]
    fn tied_reservoir_keys_evict_in_arrival_order() {
        let paths = ["/first", "/second", "/third", "/fourth"];
        let draw = |seed: u64| {
            let mut rng = rng_from_seed(seed);
            let mut reservoir = Reservoir::new(2);
            for (index, path) in paths.iter().enumerate() {
                // Every candidate saturates to MAX_SCORE, so all four keys are
                // bit-identical and only the tie-break decides the outcome.
                reservoir.offer(branch(path, index as i64, 0.0), f64::INFINITY, &mut rng);
            }
            reservoir
                .into_branches()
                .into_iter()
                .map(|b| b.path)
                .collect::<Vec<_>>()
        };

        // A tie never displaces the incumbent, so the first two arrivals hold
        // their slots against every later one.
        assert_eq!(draw(7), vec!["/first".to_owned(), "/second".to_owned()]);
        // Reproducible across seeds precisely because the keys are equal: the
        // rule, not the RNG, settles this.
        assert_eq!(draw(7), draw(99));
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
        let candidates = vec![Rc::new(branch("/a", 1, 1.0)), Rc::new(branch("/b", 2, 1.0))];
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
        let candidates = vec![
            Rc::new(branch("/a", 1, f64::MAX)),
            Rc::new(branch("/b", 2, f64::MAX)),
        ];
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

#[cfg(test)]
mod saturation_tests {
    use super::*;

    #[test]
    fn a_dominant_score_still_sorts_by_its_random_draw() {
        // Every candidate saturates, so the reservoir has nothing but the draw
        // to order them by. Before `MAX_SCORE` each key rounded to exactly 1.0
        // and `offer`'s strictly-greater test kept whichever came first,
        // regardless of what was drawn afterwards.
        let mut reservoir = Reservoir::new(1);
        let mut rng = rng_from_seed(7);
        let mut branches = Vec::new();
        for index in 0..8 {
            let mut branch = Branch::new(format!("/candidate-{index}"));
            branch.weight = f64::MAX;
            branches.push(branch);
        }
        for branch in &branches {
            reservoir.offer(branch.clone(), score_of(branch), &mut rng);
        }
        let kept = reservoir.into_branches();
        assert_eq!(kept.len(), 1);

        // Whichever candidate wins, it is not simply the first one for every
        // seed: the draw decides. Scan seeds and require at least two distinct
        // winners, which a collapsed key can never produce.
        let winners = (0..32u64)
            .map(|seed| {
                let mut reservoir = Reservoir::new(1);
                let mut rng = rng_from_seed(seed);
                for branch in &branches {
                    reservoir.offer(branch.clone(), score_of(branch), &mut rng);
                }
                reservoir.into_branches()[0].path.clone()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            winners.len() > 1,
            "a saturated score collapsed into first-come order: {winners:?}"
        );
    }

    #[test]
    fn saturation_is_clamped_rather_than_infinite() {
        let mut branch = Branch::new("/huge");
        branch.weight = f64::MAX;
        let s = score_of(&branch);
        assert!(s.is_finite());
        assert_eq!(s, MAX_SCORE);

        // A zero weight still scores at the floor, so it remains selectable.
        let mut branch = Branch::new("/zero");
        branch.weight = 0.0;
        assert_eq!(score_of(&branch), MIN_SCORE);
    }

    fn score_of(branch: &Branch) -> f64 {
        score(
            branch,
            &SampleQuery::default(),
            &SampleParams {
                weighted: true,
                ..SampleParams::default()
            },
            &HashSet::new(),
        )
    }
}

#[cfg(test)]
mod equivalence {
    use super::*;
    use std::collections::BTreeMap;

    fn branch(path: &str, id: i64, weight: f64) -> Branch {
        let mut b = Branch::new(path);
        b.id = Some(id);
        b.weight = weight;
        b
    }

    /// Selection frequency per path over many independent seeded draws.
    fn frequencies(
        weights: &[f64],
        take: usize,
        trials: u64,
        mut draw: impl FnMut(&[Rc<Branch>], usize, &mut SampleRng) -> Vec<String>,
    ) -> BTreeMap<String, f64> {
        let candidates: Vec<Rc<Branch>> = weights
            .iter()
            .enumerate()
            .map(|(i, weight)| Rc::new(branch(&format!("/b{i}"), i as i64, *weight)))
            .collect();
        let mut counts: BTreeMap<String, f64> = candidates
            .iter()
            .map(|candidate| (candidate.path.clone(), 0.0))
            .collect();
        for seed in 0..trials {
            let mut rng = rng_from_seed(seed);
            for path in draw(&candidates, take, &mut rng) {
                *counts.get_mut(&path).expect("known path") += 1.0;
            }
        }
        counts
            .into_iter()
            .map(|(path, count)| (path, count / trials as f64))
            .collect()
    }

    /// The streaming and in-memory samplers must draw from the same
    /// distribution.
    ///
    /// There are two implementations of weighted selection without replacement
    /// because they are bounded differently: [`Reservoir`] holds `take`
    /// candidates and lets `sample` stream pages, while [`sample`] keeps the
    /// pool so `compose` can draw from it once per slot. Nothing asserted they
    /// agree, so the two could drift into sampling differently — and which one
    /// a command used would change its results for reasons unrelated to the
    /// query.
    ///
    /// This compares empirical selection frequencies over 4,000 seeded draws.
    /// The seeds are fixed, so the test is deterministic; the tolerance covers
    /// sampling error at that trial count, not disagreement between the two
    /// algorithms.
    #[test]
    fn reservoir_and_pool_samplers_agree_in_distribution() {
        const TRIALS: u64 = 4_000;
        let weights = [1.0, 2.0, 4.0, 8.0];
        let query = SampleQuery::default();

        for take in [1, 2, 3] {
            let pooled = frequencies(&weights, take, TRIALS, |candidates, take, rng| {
                let params = SampleParams {
                    count: take,
                    weighted: true,
                    ..Default::default()
                };
                sample(candidates, &query, &params, &HashSet::new(), rng)
                    .iter()
                    .map(|candidate| candidate.path.clone())
                    .collect()
            });

            let streamed = frequencies(&weights, take, TRIALS, |candidates, take, rng| {
                let params = SampleParams {
                    count: take,
                    weighted: true,
                    ..Default::default()
                };
                let mut reservoir = Reservoir::new(take);
                for candidate in candidates {
                    let score = score(candidate, &query, &params, &HashSet::new());
                    reservoir.offer((**candidate).clone(), score, rng);
                }
                reservoir
                    .into_branches()
                    .iter()
                    .map(|candidate| candidate.path.clone())
                    .collect()
            });

            for (path, pooled_rate) in &pooled {
                let streamed_rate = streamed[path];
                assert!(
                    (pooled_rate - streamed_rate).abs() < 0.03,
                    "take={take} {path}: pool sampler {pooled_rate:.3} vs reservoir \
                     {streamed_rate:.3} — the two samplers disagree by more than \
                     sampling error at {TRIALS} trials"
                );
            }

            // Both must also honour the weight ordering, so an agreement on two
            // uniform distributions could not pass this.
            let ordered = pooled.values().copied().collect::<Vec<_>>();
            assert!(
                ordered.windows(2).all(|pair| pair[0] < pair[1]),
                "take={take}: selection rates must increase with weight, got {ordered:?}"
            );
        }
    }
}
