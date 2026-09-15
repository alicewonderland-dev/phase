//! U2 — the superlative player predicate, wired to both grammatical
//! positions: the effect SUBJECT ("the player with the most `<property>`
//! gains control of ~") and the TARGET position ("target player with the
//! most `<property>` draws a card"), CR 102.1 + CR 608.2c.
//!
//! Verbatim Oracle text throughout (never paraphrased). Wild Dogs (Cycling)
//! and Sokenzan Renegade (Bushido 1) carry inline keywords, built via
//! `from_oracle_text_with_keywords` per the `/card-test` recipe (foot-gun 4:
//! feeding reminder text as plain Oracle text parses to
//! `Effect::Unimplemented`).
//!
//! U2 depends on U1 (HARD ORDERING CONSTRAINT, PLAN-v3 §Sizing): on these
//! three cards the intervening-if condition U1 adds is what makes a tie at
//! resolution unreachable, so `unique_recipient_from_filter`'s fail-closed
//! "ambiguous GiveControl recipient" path is never hit here. That coupling
//! is pinned by `u2_r3_multi_authority_tie_blocks_trigger` below.
//!
//! Several rows below assert `state.unimplemented_oracle_ids` is EMPTY as
//! their revert discriminator, alongside the controller check. This is
//! necessary on rows where the pre-fix and post-fix final CONTROLLER happens
//! to coincide (the leader already controls the permanent) — with U2
//! reverted the clause lowers to `Effect::Unimplemented{name:
//! "unbound_subject"}`, which on execution records the source's oracle id in
//! `state.unimplemented_oracle_ids` (`game/effects/mod.rs`); with U2 present
//! nothing unimplemented ever executes, so the set stays empty. This is a
//! genuine revert-failing assertion, not a restatement of the (identical)
//! final-controller observable.

use engine::game::scenario::{GameScenario, P0, P1};
use engine::parser::parse_oracle_text;
use engine::types::ability::{Effect, TargetRef};
use engine::types::actions::GameAction;
use engine::types::game_state::{CastPaymentMode, WaitingFor};
use engine::types::phase::Phase;
use engine::types::PlayerId;

/// A synthetic "the player to your right gains control of ~" upkeep trigger
/// — the shipped, PRE-EXISTING `parse_subject_application` seating-neighbor
/// arm (not part of this diff), used only to prove the harness reaches
/// `Effect::GiveControl` at all. Bucknard's-Everfull-Purse-shaped, not
/// verbatim (that card gates the same clause behind an activated ability,
/// which is orthogonal to what this row measures).
const NEIGHBOR_REACH_GUARD: &str =
    "At the beginning of your upkeep, the player to your right gains control of this artifact.";

/// Ghazbán Ogre / Wild Dogs' verbatim upkeep-trigger line (CR 119.3 axis).
const LIFE_AXIS_LINE: &str = "At the beginning of your upkeep, if a player has more life than \
                               each other player, the player with the most life gains control \
                               of this creature.";

/// Wild Dogs' full verbatim Oracle text, including the inline Cycling line.
const WILD_DOGS: &str = "At the beginning of your upkeep, if a player has more life than each \
                          other player, the player with the most life gains control of this \
                          creature.\nCycling {2} ({2}, Discard this card: Draw a card.)";

/// Sokenzan Renegade's full verbatim Oracle text, including the inline
/// Bushido line.
const SOKENZAN_RENEGADE: &str = "Bushido 1 (Whenever this creature blocks or becomes blocked, \
                                  it gets +1/+1 until end of turn.)\nAt the beginning of your \
                                  upkeep, if a player has more cards in hand than each other \
                                  player, the player who has the most cards in hand gains \
                                  control of this creature.";

/// The subfamily-B decline shape (object count, not a player property): the
/// property `alt` has no `creatures` arm, so this must stay `unbound_subject`
/// — honestly red, per PLAN-v3's scope decision to leave subfamily B
/// unmodelled.
const SUBFAMILY_B_DECLINE_LINE: &str = "At the beginning of your upkeep, the player with the \
                                         most creatures gains control of this creature.";

/// "target player with the most life draws a card" — the P11 target-position
/// regression this diff closes as a side effect of U2.2.
const TARGET_LEADER_DRAW: &str = "Target player with the most life draws a card.";

fn upkeep_scenario(player_count: u8, seed: u64) -> GameScenario {
    let mut scenario = GameScenario::new_n_player(player_count, seed);
    scenario.at_phase(Phase::Untap);
    scenario
}

/// U2-R0 — POSITIVE REACH-GUARD: the harness reaches `Effect::GiveControl`
/// at all, on a shape shipped BEFORE this diff. Unaffected by revert.
#[test]
fn u2_r0_positive_reach_guard_neighbor_control_moves() {
    let mut scenario = upkeep_scenario(3, 100);
    let card = scenario
        .add_creature(P0, "Neighbor Reach-Guard Test Card", 1, 1)
        .from_oracle_text(NEIGHBOR_REACH_GUARD)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    // CR 102.1 + CR 103.1: P0's "right" neighbor in seat order [P0,P1,P2] is
    // P2 (right = previous player; see `players::neighbor`).
    assert_eq!(
        runner.state().objects[&card].controller,
        PlayerId(2),
        "the harness must reach GiveControl and move control to the right neighbor"
    );
}

/// U2-R1 — Ghazbán Ogre under P0, unique life leader IS P0 (controller).
/// The final-controller observable (`P0`) is IDENTICAL whether U2 is present
/// or reverted (P0 already controls it), so that alone does not discriminate
/// — passes either way, like U1-R1/R3. The `unimplemented_oracle_ids` check
/// is what actually discriminates: reverted, the clause lowers to
/// `Effect::Unimplemented{"unbound_subject"}`, which records the source's id
/// on execution; fixed, nothing unimplemented ever executes.
#[test]
fn u2_r1_leader_is_controller_resolves_cleanly() {
    let mut scenario = upkeep_scenario(3, 101);
    scenario
        .with_life(P0, 20)
        .with_life(P1, 10)
        .with_life(PlayerId(2), 10);
    let card = scenario
        .add_creature(P0, "Ghazbán Ogre", 2, 3)
        .from_oracle_text(LIFE_AXIS_LINE)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().objects[&card].controller, P0);
    assert!(
        runner.state().unimplemented_oracle_ids.is_empty(),
        "REVERT-FAIL: with U2 reverted the subject lowers to \
         Effect::Unimplemented{{\"unbound_subject\"}}, which records the source \
         id here on execution; got {:?}",
        runner.state().unimplemented_oracle_ids
    );
}

/// U2-R2 — LOAD-BEARING: the leader is NOT the controller. REVERT-FAIL:
/// with U2 reverted the clause is `Effect::Unimplemented{unbound_subject}`
/// and NO control move occurs (control stays with P0); fixed, control moves
/// to the leader P1. Also pins that the recipient filter selects the
/// LEADER, not the controller.
#[test]
fn u2_r2_leader_not_controller_control_moves_to_leader() {
    let mut scenario = upkeep_scenario(3, 102);
    scenario
        .with_life(P0, 10)
        .with_life(P1, 20)
        .with_life(PlayerId(2), 10);
    let card = scenario
        .add_creature(P0, "Ghazbán Ogre", 2, 3)
        .from_oracle_text(LIFE_AXIS_LINE)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&card].controller,
        P1,
        "control must move to the unique life leader P1, not stay with the controller P0"
    );
}

/// Hostile fixture — MULTI-AUTHORITY: a 4-player board where two
/// NON-CONTROLLER players tie at the top and the controller is third. With
/// U1 present, the intervening-if removes the ability at resolution (CR
/// 603.4) before U2's recipient filter is ever evaluated, so
/// `unique_recipient_from_filter`'s fail-closed "ambiguous GiveControl
/// recipient" path is never reached on this card. NOT independently
/// revert-discriminating (§5.0) — pins the U1<->U2 coupling instead.
#[test]
fn u2_r3_multi_authority_tie_blocks_trigger() {
    let mut scenario = upkeep_scenario(4, 103);
    scenario
        .with_life(P0, 10)
        .with_life(P1, 20)
        .with_life(PlayerId(2), 20)
        .with_life(PlayerId(3), 5);
    let card = scenario
        .add_creature(P0, "Ghazbán Ogre", 2, 3)
        .from_oracle_text(LIFE_AXIS_LINE)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&card].controller,
        P0,
        "a tie among non-controller leader candidates must block the trigger via U1's \
         condition before U2's recipient filter is ever evaluated"
    );
}

/// U2-R4a — P11 TARGET-POSITION REGRESSION, legality half. Board is 20/20/10
/// (P0 and P1 TIED for the max, P2 strictly lower) rather than a unique
/// leader: with a single legal candidate the cast pipeline's
/// `auto_select_targets` (CR 602.2b-style single-legal-option resolution,
/// `game/ability_utils.rs`) would silently pick it and skip straight to
/// `Priority`, never surfacing `WaitingFor::TargetSelection` for this test to
/// inspect. Two tied candidates force a real prompt. REVERT-FAIL: today the
/// qualifier is swallowed and the filter is bare `TargetFilter::Player`, so
/// P2 (strictly behind) is ALSO a legal target. Manually drives to the
/// `TargetSelection` window (rather than the `SpellCast` driver) so the
/// legal set can be inspected directly.
#[test]
fn u2_r4a_target_position_leader_is_only_legal_target() {
    let mut scenario = GameScenario::new_n_player(3, 104);
    scenario.at_phase(Phase::PreCombatMain);
    scenario
        .with_life(P0, 20)
        .with_life(P1, 20)
        .with_life(PlayerId(2), 10);
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Test Draw Instant", true, TARGET_LEADER_DRAW)
        .id();
    let mut runner = scenario.build();
    let card_id = runner.state().objects[&spell].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: spell,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Auto,
        })
        .expect("casting the spell must be accepted");
    let WaitingFor::TargetSelection { target_slots, .. } = &runner.state().waiting_for else {
        panic!(
            "expected WaitingFor::TargetSelection (two tied candidates must force a real \
             prompt, not auto-select), got {:?}",
            runner.state().waiting_for
        );
    };
    assert_eq!(target_slots.len(), 1, "exactly one target slot expected");
    let legal = &target_slots[0].legal_targets;
    assert!(
        legal.contains(&TargetRef::Player(P0)),
        "the tied life leader P0 must be a legal target, got {legal:?}"
    );
    assert!(
        legal.contains(&TargetRef::Player(P1)),
        "the tied life leader P1 must be a legal target, got {legal:?}"
    );
    assert!(
        !legal.contains(&TargetRef::Player(PlayerId(2))),
        "REVERT-FAIL: the strictly-lower P2 must NOT be a legal target, got {legal:?}"
    );
}

/// U2-R4b — P11 TARGET-POSITION REGRESSION, resolution half: casting the
/// spell targeting the unique leader P0 resolves the draw to P0.
#[test]
fn u2_r4b_target_position_leader_draws_card() {
    let mut scenario = GameScenario::new_n_player(3, 105);
    scenario.at_phase(Phase::PreCombatMain);
    scenario
        .with_life(P0, 20)
        .with_life(P1, 10)
        .with_life(PlayerId(2), 10);
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Test Draw Instant", true, TARGET_LEADER_DRAW)
        .id();
    // Give P0 a card to draw so the draw is observable.
    scenario.with_library_top(P0, &["Library Card"]);
    let mut runner = scenario.build();
    let outcome = runner.cast(spell).target_player(P0).resolve();
    outcome.assert_hand_drawn(P0, 1);
}

/// U2-R5 — the `HandSize` axis end-to-end (Sokenzan Renegade), inline
/// Bushido keyword. Same leader-is-controller shape as R1: the
/// `unimplemented_oracle_ids` check is the revert discriminator.
#[test]
fn u2_r5_sokenzan_renegade_hand_axis_resolves_cleanly() {
    let mut scenario = upkeep_scenario(3, 106);
    let card = scenario
        .add_creature(P0, "Sokenzan Renegade", 3, 2)
        .from_oracle_text_with_keywords(&["Bushido"], SOKENZAN_RENEGADE)
        .id();
    scenario
        .with_cards_in_hand(P0, &["Card A1", "Card A2", "Card A3"])
        .with_cards_in_hand(P1, &["Card B1"])
        .with_cards_in_hand(PlayerId(2), &["Card C1"]);
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().objects[&card].controller, P0);
    assert!(
        runner.state().unimplemented_oracle_ids.is_empty(),
        "REVERT-FAIL: Sokenzan Renegade's subject must not lower to \
         Effect::Unimplemented; got {:?}",
        runner.state().unimplemented_oracle_ids
    );
}

/// U2-R6 — the LIFE axis end-to-end via Wild Dogs, inline Cycling keyword.
/// Pins that the inline Cycling line does not derail the upkeep line's
/// parse. Same leader-is-controller shape as R1: the
/// `unimplemented_oracle_ids` check is the revert discriminator.
#[test]
fn u2_r6_wild_dogs_life_axis_resolves_cleanly() {
    let mut scenario = upkeep_scenario(3, 107);
    scenario
        .with_life(P0, 20)
        .with_life(P1, 10)
        .with_life(PlayerId(2), 10);
    let card = scenario
        .add_creature(P0, "Wild Dogs", 3, 3)
        .from_oracle_text_with_keywords(&["Cycling"], WILD_DOGS)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_upkeep();
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().objects[&card].controller, P0);
    assert!(
        runner.state().unimplemented_oracle_ids.is_empty(),
        "REVERT-FAIL: Wild Dogs' subject must not lower to Effect::Unimplemented \
         (and the inline Cycling line must not derail the upkeep line's parse); \
         got {:?}",
        runner.state().unimplemented_oracle_ids
    );
}

/// SHAPE — subfamily-B decline path stays honestly red: the property `alt`
/// has no `creatures` arm, so this clause must remain
/// `Effect::Unimplemented{"unbound_subject"}`, never a silently-wrong
/// binding. Parser-shape-only is acceptable here per the `/card-test`
/// recipe: the semantics stay genuinely unsupported (red), not a runtime
/// behavior claim.
#[test]
fn subfamily_b_object_count_noun_declines_honestly() {
    let parsed = parse_oracle_text(
        SUBFAMILY_B_DECLINE_LINE,
        "Subfamily-B Decline Test Card",
        &[],
        &["Creature".to_string()],
        &[],
    );
    let trigger = parsed
        .triggers
        .into_iter()
        .next()
        .expect("the phase trigger itself must still parse");
    let execute = trigger
        .execute
        .expect("the trigger must carry an execute ability, even an Unimplemented one");
    assert!(
        matches!(
            execute.effect.as_ref(),
            Effect::Unimplemented { name, .. } if name == "unbound_subject"
        ),
        "the object-count noun 'creatures' must decline honestly as unbound_subject, \
         got {:?}",
        execute.effect
    );
}
