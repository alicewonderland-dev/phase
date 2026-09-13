use draft_core::types::DraftCardInstance;
use engine::database::CardDatabase;
use phase_ai::config::AiDifficulty;
use phase_ai::draft_eval;
use rand::Rng;

/// Select a card index from the pack for a bot to pick.
///
/// Strategy scales with difficulty per D-02:
/// - VeryEasy: pure random
/// - Easy: rarity-weighted
/// - Medium / Hard: `phase_ai::draft_eval` card quality + rarity + color discipline + curve
/// - VeryHard: same, with stricter color discipline (an off-color penalty)
///
/// (Medium falls back to the lighter color + rarity + curve heuristic when no
/// CardDatabase is loaded, via [`pick_by_evaluation`].)
///
/// Returns the index into the `pack` slice.
pub fn bot_pick(
    pack: &[DraftCardInstance],
    difficulty: AiDifficulty,
    prior_picks: &[DraftCardInstance],
    card_db: Option<&CardDatabase>,
    rng: &mut impl Rng,
) -> usize {
    if pack.is_empty() {
        return 0;
    }

    match difficulty {
        AiDifficulty::VeryEasy => rng.random_range(0..pack.len()),
        AiDifficulty::Easy => pick_by_rarity(pack),
        AiDifficulty::Medium | AiDifficulty::Hard => {
            pick_by_evaluation(pack, prior_picks, card_db, false)
        }
        AiDifficulty::VeryHard | AiDifficulty::CEDH => {
            pick_by_evaluation(pack, prior_picks, card_db, true)
        }
    }
}

/// Select `count` distinct card indices for a bot's pick step, or every index
/// when the pack holds fewer than `count`.
///
/// CR 903.13b: a Commander Draft seat drafts two cards per step, so a bot in
/// such a pod must return two indices or the round never completes. Composed
/// from [`bot_pick`] applied to the shrinking remainder; *which* cards a bot
/// takes for a multi-card step is deliberately untuned (out of scope).
///
/// Returns indices into the original `pack` slice, in selection order.
pub fn bot_picks(
    pack: &[DraftCardInstance],
    count: usize,
    difficulty: AiDifficulty,
    prior_picks: &[DraftCardInstance],
    card_db: Option<&CardDatabase>,
    rng: &mut impl Rng,
) -> Vec<usize> {
    // Candidates carry their original index so the caller can map back to
    // `instance_id`s before mutating anything. Held as two parallel vectors
    // rather than a `Vec<(usize, _)>` so that `bot_pick` can borrow the cards as
    // the contiguous slice it takes, without rebuilding one per iteration:
    // `swap_remove(position)` applies the same permutation to both, so they stay
    // aligned, and the whole walk costs one clone of the pack instead of
    // `count + 1`.
    let mut candidates: Vec<DraftCardInstance> = pack.to_vec();
    let mut original_indices: Vec<usize> = (0..pack.len()).collect();
    let mut picked = Vec::with_capacity(count.min(candidates.len()));

    for _ in 0..count.min(pack.len()) {
        let position = bot_pick(&candidates, difficulty, prior_picks, card_db, rng);
        candidates.swap_remove(position);
        picked.push(original_indices.swap_remove(position));
    }

    picked
}

/// Pick the highest-rarity card. Ties broken by first occurrence.
fn pick_by_rarity(pack: &[DraftCardInstance]) -> usize {
    pack.iter()
        .enumerate()
        .max_by_key(|(_, c)| rarity_score(&c.rarity))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Lighter heuristic: score = rarity * 2 + color_bonus + curve_bonus, using the
/// enriched DraftCardInstance fields (colors, cmc) directly. Used as the no-DB
/// fallback inside [`pick_by_evaluation`].
fn pick_by_color_and_rarity(
    pack: &[DraftCardInstance],
    prior_picks: &[DraftCardInstance],
) -> usize {
    let preferred_colors = color_preference(prior_picks);

    pack.iter()
        .enumerate()
        .max_by_key(|(_, card)| {
            let rarity = rarity_score(&card.rarity) as i16 * 2;
            let color_bonus = if card.colors.is_empty() {
                // Colorless cards are always on-color
                1i16
            } else if card.colors.iter().any(|c| preferred_colors.contains(c)) {
                3
            } else if preferred_colors.is_empty() {
                // No preference yet (early picks) — no bonus/penalty
                0
            } else {
                -1
            };
            let curve = curve_bonus(card.cmc, prior_picks.len() as u8);
            rarity + color_bonus + curve as i16
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Medium/Hard/VeryHard strategy: `phase_ai::draft_eval` card quality plus a rarity
/// prior, color discipline, and a curve bonus. `strict` (VeryHard) raises the
/// on-color bonus and adds an off-color penalty. Falls back to
/// [`pick_by_color_and_rarity`] if no CardDatabase is loaded.
fn pick_by_evaluation(
    pack: &[DraftCardInstance],
    prior_picks: &[DraftCardInstance],
    card_db: Option<&CardDatabase>,
    strict: bool,
) -> usize {
    let card_db = match card_db {
        Some(db) => db,
        None => return pick_by_color_and_rarity(pack, prior_picks),
    };

    let preferred_colors = color_preference(prior_picks);
    let pick_number = prior_picks.len() as u8;

    // Color bonus multiplier: stricter for VeryHard
    let on_color_bonus: f64 = if strict { 6.0 } else { 4.0 };
    let off_color_penalty: f64 = if strict { -2.0 } else { 0.0 };

    pack.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let score_a = eval_score(
                a,
                card_db,
                &preferred_colors,
                pick_number,
                on_color_bonus,
                off_color_penalty,
            );
            let score_b = eval_score(
                b,
                card_db,
                &preferred_colors,
                pick_number,
                on_color_bonus,
                off_color_penalty,
            );
            score_a
                .partial_cmp(&score_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Pick-context score for a card: intrinsic card quality (`phase_ai::draft_eval`)
/// plus a rarity prior, color discipline relative to prior picks, and a curve bonus.
fn eval_score(
    card: &DraftCardInstance,
    card_db: &CardDatabase,
    preferred_colors: &[String],
    pick_number: u8,
    on_color_bonus: f64,
    off_color_penalty: f64,
) -> f64 {
    let base = card_quality(card, Some(card_db));

    let color_bonus = if card.colors.is_empty() {
        1.0 // Colorless — always fine
    } else if preferred_colors.is_empty() {
        0.0 // No preference yet
    } else if card.colors.iter().any(|c| preferred_colors.contains(c)) {
        on_color_bonus
    } else {
        off_color_penalty
    };

    let curve = curve_bonus(card.cmc, pick_number) as f64;

    base + color_bonus + curve
}

/// Intrinsic card quality: the engine-data evaluator ([`draft_eval::evaluate_draft_card`])
/// plus a small rarity prior. Falls back to just the rarity prior when no
/// CardDatabase is loaded or the card face isn't found.
fn card_quality(card: &DraftCardInstance, card_db: Option<&CardDatabase>) -> f64 {
    let quality = card_db
        .and_then(|db| db.get_face_by_name(&card.name))
        .map(draft_eval::evaluate_draft_card_default)
        .unwrap_or(0.0);
    quality + draft_eval::rarity_prior(&card.rarity)
}

fn rarity_score(rarity: &str) -> u8 {
    match rarity {
        "mythic" => 4,
        "rare" => 3,
        "uncommon" => 2,
        "common" => 1,
        _ => 0,
    }
}

/// Extract the 1-2 most common colors from prior picks.
/// Returns empty vec if no clear preference (fewer than 3 prior picks).
///
/// Delegates to [`draft_eval::dominant_colors`], the single authority for
/// "which colors is this pile of cards in" — shared with the Winston valuation
/// layer's late color ramp, so the two never drift apart.
///
/// **This changes `bot_pick`'s behavior for pools with tied colors**, and that is
/// the point of the lift. The body that used to live here sorted `HashMap`
/// entries on count alone; Rust randomizes `HashMap` iteration order per map
/// instance, so tied colors came out in an arbitrary order and identical inputs
/// produced different picks. MEASURED: a four-way tie produced 12 distinct
/// answers over 2000 draws, and a W=3/U=2/B=2 pool was a coin flip between
/// `["W", "U"]` and `["W", "B"]`. `dominant_colors`'s total order pins it to
/// `["W", "B"]`. Behavior on strictly ordered pools is unchanged —
/// `dominant_colors_delegation_preserves_bot_pick_on_a_strictly_ordered_pool`.
fn color_preference(prior_picks: &[DraftCardInstance]) -> Vec<String> {
    let colors: Vec<&[String]> = prior_picks
        .iter()
        .map(|card| card.colors.as_slice())
        .collect();
    draft_eval::dominant_colors(&colors, 3)
}

/// Mana curve position bonus. Prefer CMC 2-4 creatures, especially early in draft.
fn curve_bonus(cmc: u8, pick_number: u8) -> i8 {
    let early = pick_number < 15; // First pack roughly

    match cmc {
        2 => {
            if early {
                2
            } else {
                1
            }
        }
        3 => {
            if early {
                2
            } else {
                1
            }
        }
        4 => 1,
        5 => 0,
        1 => 0,
        0 => 0, // lands, weird cards
        _ => {
            // CMC 6+: slight penalty, less so late
            if early {
                -1
            } else {
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    fn pack(size: usize) -> Vec<DraftCardInstance> {
        (0..size)
            .map(|i| DraftCardInstance {
                instance_id: format!("card-{i}"),
                name: format!("Card {i}"),
                set_code: "TST".to_string(),
                collector_number: format!("{i}"),
                rarity: "common".to_string(),
                colors: Vec::new(),
                cmc: 0,
                type_line: String::new(),
                draft_effect: None,
            })
            .collect()
    }

    fn colored(name: &str, colors: &[&str]) -> DraftCardInstance {
        DraftCardInstance {
            instance_id: format!("{name}-id"),
            name: name.to_string(),
            set_code: "TST".to_string(),
            collector_number: "1".to_string(),
            rarity: "common".to_string(),
            colors: colors.iter().map(|c| c.to_string()).collect(),
            cmc: 2,
            type_line: "Creature".to_string(),
            draft_effect: None,
        }
    }

    /// The `dominant_colors` lift is behavior-preserving where the old body was
    /// deterministic — i.e. on a pool with a STRICT color ordering. The tie case
    /// deliberately changes (see `color_preference`'s doc and
    /// `draft_eval::dominant_colors_is_total_on_ties`), so it is pinned there and
    /// not here.
    #[test]
    fn dominant_colors_delegation_preserves_bot_pick_on_a_strictly_ordered_pool() {
        // W=2, U=1: a strict ordering with no tie to break.
        let prior = vec![
            colored("Prior W1", &["W"]),
            colored("Prior W2", &["W"]),
            colored("Prior U1", &["U"]),
        ];
        assert_eq!(
            color_preference(&prior),
            vec!["W".to_string(), "U".to_string()],
            "a strictly ordered pool keeps its pre-lift preference"
        );

        // On-color first, off-color second: `max_by_key` returns the LAST maximum,
        // so an empty preference (which is what a changed sample floor produces)
        // flips the pick to index 1.
        let pack = vec![colored("On Color", &["W"]), colored("Off Color", &["G"])];
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        for _ in 0..200 {
            assert_eq!(
                bot_pick(&pack, AiDifficulty::Medium, &prior, None, &mut rng),
                0,
                "the on-color card must win, every time"
            );
        }

        // Below the sample floor there is no preference at all, and the tie
        // resolves the other way — the paired negative that proves the preference
        // is what drove the pick above.
        let short = vec![colored("Prior W1", &["W"]), colored("Prior W2", &["W"])];
        assert!(color_preference(&short).is_empty());
        assert_eq!(
            bot_pick(&pack, AiDifficulty::Medium, &short, None, &mut rng),
            1
        );
    }

    /// CR 903.13b: a bot in a two-card pod must return two usable indices.
    ///
    /// A direct unit test of a pre-wired helper: `count > 1` is not reachable
    /// from any production path in this phase, because the wasm bot loop is
    /// `Quick`-gated and `Quick` has `cards_per_pick == 1`. Saying so is more
    /// useful than implying production coverage.
    #[test]
    fn bot_picks_returns_n_distinct_indices() {
        let cards = pack(14);
        let mut rng = ChaCha20Rng::seed_from_u64(42);

        let picked = bot_picks(&cards, 2, AiDifficulty::Medium, &[], None, &mut rng);
        assert_eq!(picked.len(), 2);
        assert_ne!(picked[0], picked[1], "a bot cannot draft one card twice");
        assert!(picked.iter().all(|index| *index < cards.len()));

        // Clamped: a count larger than the pack yields every index exactly once.
        let small = pack(1);
        let clamped = bot_picks(&small, 2, AiDifficulty::Medium, &[], None, &mut rng);
        assert_eq!(clamped, vec![0]);

        // Degenerate counts.
        assert!(bot_picks(&cards, 0, AiDifficulty::Medium, &[], None, &mut rng).is_empty());
        assert!(bot_picks(&[], 2, AiDifficulty::Medium, &[], None, &mut rng).is_empty());

        // The single-card case must still agree with `bot_pick` itself, since
        // that is the path every existing kind takes.
        let mut rng_a = ChaCha20Rng::seed_from_u64(7);
        let mut rng_b = ChaCha20Rng::seed_from_u64(7);
        assert_eq!(
            bot_picks(&cards, 1, AiDifficulty::Medium, &[], None, &mut rng_a),
            vec![bot_pick(
                &cards,
                AiDifficulty::Medium,
                &[],
                None,
                &mut rng_b
            )]
        );
    }
}
