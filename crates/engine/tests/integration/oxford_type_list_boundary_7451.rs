//! Issue #7451 — the trigger condition/effect boundary must span the WHOLE
//! Oxford-comma type list in a trigger's EFFECT subject, not truncate to the
//! list's last item. `oracle_trigger.rs::is_new_sentence_not_type_continuation`
//! walks past the list's own commas instead of stopping at the first one, so
//! before the fix the effect handed downstream keeps only the FINAL list item.
//!
//! V1 drives the real Oracle parse -> `split_trigger` -> effect chain ->
//! `Effect::PumpAll` -> `evaluate_layers` pipeline and is revert-failing. V2
//! and V4 are PINS: their production seams are unaffected by this change
//! (protection lists and quantity counts are already correct at the arity the
//! issue complains about), included here so the new file documents the whole
//! issue rather than only the piece U1 repairs.
//!
//! Oracle text is verbatim, fetched from `client/public/card-data.json` at the
//! branch base.

use engine::game::combat::AttackTarget;
use engine::game::keywords::source_matches_card_type;
use engine::game::layers::evaluate_layers;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::parser::oracle::parse_oracle_text;
use engine::types::ability::ContinuousModification;
use engine::types::identifiers::ObjectId;
use engine::types::keywords::{Keyword, ProtectionTarget};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;

const VALLEY_FLOODCALLER: &str = "Flash\nYou may cast noncreature spells as though they had flash.\nWhenever you cast a noncreature spell, Birds, Frogs, Otters, and Rats you control get +1/+1 until end of turn. Untap them.";

const VALLEY_ROTCALLER: &str = "Menace\nWhenever this creature attacks, each opponent loses X life and you gain X life, where X is the number of other Squirrels, Bats, Lizards, and Rats you control.";

fn effective_pt(runner: &mut GameRunner, id: ObjectId) -> (i32, i32) {
    runner.state_mut().layers_dirty.mark_full();
    evaluate_layers(runner.state_mut());
    let object = &runner.state().objects[&id];
    (
        object.power.expect("creature has power"),
        object.toughness.expect("creature has toughness"),
    )
}

fn life(runner: &GameRunner, player: PlayerId) -> i32 {
    runner.state().players[player.0 as usize].life
}

/// V1 — Valley Floodcaller's cast trigger must pump every listed subtype, not
/// just the last one in the Oxford-comma list. Revert-failing: before the fix,
/// `find_effect_boundary` walks past the list's own commas and lands on the
/// LAST one, so the effect handed to the effect parser is only
/// `"Rats you control get +1/+1 until end of turn"` — Bird, Frog, Otter and
/// the self-inclusive Floodcaller are all left unpumped.
#[test]
fn valley_floodcaller_pumps_every_listed_subtype() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bird = scenario
        .add_creature(P0, "Birdy", 2, 2)
        .with_subtypes(vec!["Bird"])
        .id();
    let frog = scenario
        .add_creature(P0, "Froggy", 2, 2)
        .with_subtypes(vec!["Frog"])
        .id();
    let otter = scenario
        .add_creature(P0, "Ottery", 2, 2)
        .with_subtypes(vec!["Otter"])
        .id();
    let rat = scenario
        .add_creature(P0, "Ratty", 2, 2)
        .with_subtypes(vec!["Rat"])
        .id();
    let bear = scenario.add_creature(P0, "Beary", 2, 2).id();
    let floodcaller = scenario
        .add_creature_from_oracle(P0, "Valley Floodcaller", 2, 2, VALLEY_FLOODCALLER)
        .with_subtypes(vec!["Otter", "Wizard"]) // CR 205.3m: the printed type line
        .id();
    let bolt = scenario.add_bolt_to_hand(P0);
    let mut runner: GameRunner = scenario.build();

    runner.cast(bolt).target_player(P1).resolve();
    runner.advance_until_stack_empty();

    assert_eq!(
        effective_pt(&mut runner, bird),
        (3, 3),
        "Bird must be pumped"
    );
    assert_eq!(
        effective_pt(&mut runner, frog),
        (3, 3),
        "Frog must be pumped"
    );
    assert_eq!(
        effective_pt(&mut runner, otter),
        (3, 3),
        "Otter must be pumped"
    );
    assert_eq!(effective_pt(&mut runner, rat), (3, 3), "Rat must be pumped");
    assert_eq!(
        effective_pt(&mut runner, floodcaller),
        (3, 3),
        "Valley Floodcaller is itself an Otter (CR 205.3m) and the filter carries \
         no FilterProp::Another, so CR 109.5 puts the source in its own pumped \
         population"
    );
    // Paired positive reach-guard: at least one creature's P/T actually changed
    // above, so this negative is not vacuous.
    assert_eq!(
        effective_pt(&mut runner, bear),
        (2, 2),
        "a plain creature outside all four listed subtypes must stay unpumped"
    );
}

/// V2 — protection-list arity PIN. This seam (`expand_protection_parts` /
/// `Keyword::Protection` / `source_matches_card_type`) is untouched by this
/// change; it is already correct at arity today. Scoped to arity + per-member
/// resolution of the three subtype members; makes no claim about a
/// non-card-type quality (Tinfoil Helm's "hybrid mana", a separate,
/// pre-existing gap).
#[test]
fn protection_from_subtype_list_keeps_every_member() {
    let parsed = parse_oracle_text(
        "This creature has protection from Krakens, Leviathans, and Serpents.",
        "Oxford Ward",
        &[],
        &["Creature".to_string()],
        &[],
    );
    let qualities: Vec<String> = parsed
        .statics
        .iter()
        .flat_map(|d| d.modifications.iter())
        .filter_map(|m| match m {
            ContinuousModification::AddKeyword {
                keyword: Keyword::Protection(ProtectionTarget::CardType(q)),
            } => Some(q.clone()),
            ContinuousModification::AddKeyword {
                keyword: Keyword::Protection(ProtectionTarget::Quality(q)),
            } => Some(q.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        qualities.len(),
        3,
        "expected exactly three Protection modifications, in printed order, got {qualities:?}"
    );

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let kraken = scenario
        .add_creature(P0, "Test Kraken", 1, 1)
        .with_subtypes(vec!["Kraken"])
        .id();
    let leviathan = scenario
        .add_creature(P0, "Test Leviathan", 1, 1)
        .with_subtypes(vec!["Leviathan"])
        .id();
    let serpent = scenario
        .add_creature(P0, "Test Serpent", 1, 1)
        .with_subtypes(vec!["Serpent"])
        .id();
    let runner: GameRunner = scenario.build();

    let kraken_obj = &runner.state().objects[&kraken];
    let leviathan_obj = &runner.state().objects[&leviathan];
    let serpent_obj = &runner.state().objects[&serpent];

    assert!(
        source_matches_card_type(kraken_obj, &qualities[0]),
        "the first listed quality must match a Kraken"
    );
    assert!(
        source_matches_card_type(leviathan_obj, &qualities[1]),
        "the second listed quality must match a Leviathan"
    );
    assert!(
        source_matches_card_type(serpent_obj, &qualities[2]),
        "the third listed quality must match a Serpent"
    );
    // Paired negative: per-member resolution, not a wildcard match.
    assert!(
        !source_matches_card_type(kraken_obj, &qualities[2]),
        "a Kraken must NOT match the third (Serpent) quality"
    );
}

/// V4 — quantity-count PIN. `QuantityRef::ObjectCount` / `game/quantity.rs` is
/// untouched by this change: Valley Rotcaller's own `"attacks,"` boundary
/// already isolates the effect well before the four-subtype list is reached,
/// so the count is already computed over the whole list today. The negative
/// sibling is the reach-guard: it proves the count is genuinely computed, not
/// a hard-coded four.
#[test]
fn rotcaller_counts_every_listed_subtype() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario
        .add_creature(P0, "Squirrelly", 1, 1)
        .with_subtypes(vec!["Squirrel"]);
    scenario
        .add_creature(P0, "Batty", 1, 1)
        .with_subtypes(vec!["Bat"]);
    scenario
        .add_creature(P0, "Lizardy", 1, 1)
        .with_subtypes(vec!["Lizard"]);
    scenario
        .add_creature(P0, "Ratty", 1, 1)
        .with_subtypes(vec!["Rat"]);
    scenario.add_creature(P0, "Beary", 2, 2);
    let rotcaller = scenario
        .add_creature_from_oracle(P0, "Valley Rotcaller", 1, 3, VALLEY_ROTCALLER)
        .with_subtypes(vec!["Squirrel", "Warlock"]) // CR 205.3m: the printed type line
        .id();
    let mut runner: GameRunner = scenario.build();

    let p0_before = life(&runner, P0);
    let p1_before = life(&runner, P1);

    runner.advance_to_combat();
    runner
        .declare_attackers(&[(rotcaller, AttackTarget::Player(P1))])
        .expect("declaring the sole attacker should succeed");
    runner.advance_until_stack_empty();

    assert_eq!(
        life(&runner, P1) - p1_before,
        -4,
        "each opponent must lose X life, X = the four OTHER listed-subtype creatures \
         (Rotcaller itself excluded by \"other\"; the Bear excluded by subtype)"
    );
    assert_eq!(
        life(&runner, P0) - p0_before,
        4,
        "the attacking player must gain the same X"
    );
}

/// Negative sibling of [`rotcaller_counts_every_listed_subtype`] and its reach
/// guard: with only ONE listed-subtype creature on board, X must be exactly
/// 1, not the four-creature figure the row above pins.
#[test]
fn rotcaller_counts_exactly_one_when_only_one_listed_subtype_present() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario
        .add_creature(P0, "Ratty", 1, 1)
        .with_subtypes(vec!["Rat"]);
    let rotcaller = scenario
        .add_creature_from_oracle(P0, "Valley Rotcaller", 1, 3, VALLEY_ROTCALLER)
        .with_subtypes(vec!["Squirrel", "Warlock"])
        .id();
    let mut runner: GameRunner = scenario.build();

    let p0_before = life(&runner, P0);
    let p1_before = life(&runner, P1);

    runner.advance_to_combat();
    runner
        .declare_attackers(&[(rotcaller, AttackTarget::Player(P1))])
        .expect("declaring the sole attacker should succeed");
    runner.advance_until_stack_empty();

    assert_eq!(life(&runner, P1) - p1_before, -1, "X must be 1, not 4");
    assert_eq!(life(&runner, P0) - p0_before, 1, "X must be 1, not 4");
}
