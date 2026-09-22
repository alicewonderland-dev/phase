//! `CombatRelation::BlockedBySubject` — the persistent pairwise "creatures it
//! blocked this combat / this turn" primitive (issue #9179, PR 2).
//!
//! `CombatRelation::BlockingOrBlockedBy` reads live `combat.blocker_to_attacker`,
//! which `prune_object_from_combat` empties per CR 506.4 the moment either
//! creature leaves combat — exactly when a dies-trigger needs the answer. These
//! tests drive the real engine (`GameScenario`, real `declare_blockers_for_player`
//! / `place_blocking` / phase machinery), never hand-built `GameState` literals,
//! so a reverted edit actually reaches them.

use engine::game::combat::AttackTarget;
use engine::game::filter::{
    matches_target_filter, matches_target_filter_on_zone_change_record, FilterContext,
};
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::types::ability::{
    CombatHistoryScope, CombatRelation, CombatRelationSubject, FilterProp, TargetFilter,
    TypedFilter,
};
use engine::types::actions::GameAction;
use engine::types::game_state::WaitingFor;
use engine::types::identifiers::{ObjectId, ObjectIncarnationRef};
use engine::types::keywords::Keyword;
use engine::types::mana::ManaCost;
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

const MURDER: &str = "Destroy target creature.";
const EPHEMERATE: &str =
    "Exile target creature you control, then return it to the battlefield under its owner's control.";
const FULL_THROTTLE: &str = "After this main phase, there are two additional combat phases.
At the beginning of each combat this turn, untap all creatures that attacked this turn.";

/// Drive from `PreCombatMain` through a single declare-blockers step: pass to
/// declare-attackers, declare `attacks`/`bands`, pass the CR 508.2 post-attack
/// priority window, then declare `blocks`. Leaves the runner paused right after
/// blockers are declared, with `state.combat` still live — the model is
/// `banding_combat.rs::drive_to_first_damage_prompt`, stopped one step earlier.
fn drive_declare_blockers(
    runner: &mut GameRunner,
    attacks: Vec<(ObjectId, AttackTarget)>,
    bands: Vec<Vec<ObjectId>>,
    blocks: Vec<(ObjectId, ObjectId)>,
) {
    runner.pass_both_players();
    runner
        .act(GameAction::DeclareAttackers { attacks, bands })
        .expect("DeclareAttackers should succeed");
    if matches!(runner.state().waiting_for, WaitingFor::Priority { .. }) {
        runner.pass_both_players();
    }
    runner
        .act(GameAction::DeclareBlockers {
            assignments: blocks,
        })
        .expect("DeclareBlockers should succeed");
}

/// Drive past the end of the current turn regardless of what's waiting,
/// declaring no attackers/blockers along the way. Mirrors
/// `delayed_parent_target_incarnation.rs::advance_past_end_of_turn`.
fn advance_past_end_of_turn(runner: &mut GameRunner) {
    let start_turn = runner.state().turn_number;
    let mut guard = 0;
    while runner.state().turn_number == start_turn {
        guard += 1;
        assert!(
            guard < 256,
            "turn never ended; phase = {:?}, waiting_for = {:?}",
            runner.state().phase,
            runner.state().waiting_for,
        );
        let action = match &runner.state().waiting_for {
            WaitingFor::DeclareAttackers { .. } => GameAction::DeclareAttackers {
                attacks: vec![],
                bands: vec![],
            },
            WaitingFor::DeclareBlockers { .. } => GameAction::DeclareBlockers {
                assignments: vec![],
            },
            _ => GameAction::PassPriority,
        };
        if let Err(e) = runner.act(action) {
            panic!(
                "advancing past end of turn failed: {e:?} (phase = {:?}, waiting_for = {:?})",
                runner.state().phase,
                runner.state().waiting_for,
            );
        }
    }
}

/// Drive until `state.combat` becomes `None` (CR 511.3's end of the end of
/// combat step), declaring no attackers/blockers along the way.
fn advance_until_combat_ends(runner: &mut GameRunner) {
    let mut guard = 0;
    while runner.state().combat.is_some() {
        guard += 1;
        assert!(
            guard < 256,
            "combat never ended; phase = {:?}, waiting_for = {:?}",
            runner.state().phase,
            runner.state().waiting_for,
        );
        let action = match &runner.state().waiting_for {
            WaitingFor::DeclareAttackers { .. } => GameAction::DeclareAttackers {
                attacks: vec![],
                bands: vec![],
            },
            WaitingFor::DeclareBlockers { .. } => GameAction::DeclareBlockers {
                assignments: vec![],
            },
            _ => GameAction::PassPriority,
        };
        if let Err(e) = runner.act(action) {
            panic!(
                "advancing until combat ends failed: {e:?} (phase = {:?}, waiting_for = {:?})",
                runner.state().phase,
                runner.state().waiting_for,
            );
        }
    }
}

/// Hand `player` priority directly, mirroring
/// `cr733_resolved_combat_membership.rs::advance_to_declare_blockers_and_give_priority`.
/// After a real `DeclareBlockers` action CR 509.2 gives the ACTIVE player
/// priority, so a defending-player instant cast (e.g. destroying the blocker)
/// needs an explicit hand-off — casting is gated on `priority_player`, not
/// merely on holding the card.
fn give_priority(runner: &mut GameRunner, player: PlayerId) {
    let state = runner.state_mut();
    state.priority_player = player;
    state.waiting_for = WaitingFor::Priority { player };
}

/// `TargetFilter::Typed(creature)` with a single `CombatRelation` property,
/// subject always `Source` — the template shared by every test below, matching
/// `filter.rs`'s own `combat_relation_matches_creatures_blocking_or_blocked_by_parent_target`
/// unit test's construction shape.
fn combat_relation_filter(relation: CombatRelation) -> TargetFilter {
    TargetFilter::Typed(
        TypedFilter::creature().properties(vec![FilterProp::CombatRelation {
            relation,
            subject: CombatRelationSubject::Source,
        }]),
    )
}

/// T1: after a real declare-blockers step, the block-history ledgers hold
/// `{blocker -> {attacker incarnation}}`, `BlockedBySubject` matches the
/// blocked attacker with the blocker as source, and — the whole reason this
/// primitive is pairwise rather than the unary `creatures_blocked_this_turn` —
/// a second attacker the blocker never blocked matches on NEITHER window.
#[test]
fn declare_blockers_records_each_blocker_to_attacker_pair() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let blocked = scenario.add_creature(P0, "Blocked Attacker", 2, 2).id();
    let unblocked = scenario.add_creature(P0, "Unblocked Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![
            (blocked, AttackTarget::Player(P1)),
            (unblocked, AttackTarget::Player(P1)),
        ],
        vec![],
        vec![(blocker, blocked)],
    );

    let state = runner.state();
    let combat = state.combat.as_ref().expect("combat is live");

    // Reach guard: the block landed and `unblocked` is genuinely undeclared.
    assert!(
        combat
            .blocker_to_attacker
            .get(&blocker)
            .is_some_and(|a| a.contains(&blocked) && !a.contains(&unblocked)),
        "reach guard: the live reverse lookup must name only the blocked attacker"
    );

    let blocked_ref = ObjectIncarnationRef::from_object(&state.objects[&blocked]);
    let unblocked_ref = ObjectIncarnationRef::from_object(&state.objects[&unblocked]);

    // Direct ledger reads (E3, E4, E5).
    assert!(
        combat
            .creature_blocked_attackers_this_combat
            .get(&blocker)
            .is_some_and(
                |attackers| attackers.contains(&blocked_ref) && !attackers.contains(&unblocked_ref)
            ),
        "the combat-scoped ledger must hold exactly the declared pair"
    );
    assert!(
        state
            .creature_blocked_attackers_this_turn
            .get(&blocker)
            .is_some_and(
                |attackers| attackers.contains(&blocked_ref) && !attackers.contains(&unblocked_ref)
            ),
        "the turn-scoped ledger must hold exactly the declared pair"
    );

    // Evaluator reads (E10).
    let ctx = FilterContext::from_source_with_controller(blocker, P1);
    for scope in [CombatHistoryScope::ThisCombat, CombatHistoryScope::ThisTurn] {
        let filter = combat_relation_filter(CombatRelation::BlockedBySubject { scope });
        assert!(
            matches_target_filter(state, blocked, &filter, &ctx),
            "{scope:?}: the blocked attacker must match"
        );
        assert!(
            !matches_target_filter(state, unblocked, &filter, &ctx),
            "{scope:?}: an attacker the blocker never blocked must not match"
        );
    }
}

/// T2: CR 702.22h propagates a block across a band, so a single declared block
/// against one band member records BOTH members — not just the chosen one.
#[test]
fn banding_block_records_the_whole_band_not_just_the_chosen_attacker() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let banded = scenario
        .add_creature(P0, "Banded Attacker", 2, 2)
        .with_keyword(Keyword::Banding)
        .id();
    let plain = scenario.add_creature(P0, "Plain Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 4, 4).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![
            (banded, AttackTarget::Player(P1)),
            (plain, AttackTarget::Player(P1)),
        ],
        vec![vec![banded, plain]],
        vec![(blocker, banded)],
    );

    let state = runner.state();
    let combat = state.combat.as_ref().expect("combat is live");

    // Reach guard: banding really did propagate the block to the plain member.
    assert!(
        combat
            .blocker_to_attacker
            .get(&blocker)
            .is_some_and(|a| a.contains(&banded) && a.contains(&plain)),
        "reach guard: CR 702.22h must propagate the block across the band"
    );

    let banded_ref = ObjectIncarnationRef::from_object(&state.objects[&banded]);
    let plain_ref = ObjectIncarnationRef::from_object(&state.objects[&plain]);
    let recorded = combat
        .creature_blocked_attackers_this_combat
        .get(&blocker)
        .expect("the blocker must have a ledger entry");
    assert!(
        recorded.contains(&banded_ref) && recorded.contains(&plain_ref),
        "CR 702.22h + CR 702.22k: the ledger must hold the whole band, not just the chosen attacker"
    );
}

/// T3: the combat-scoped history ledger survives the blocker leaving the
/// battlefield, while the live `BlockingOrBlockedBy` relation — read straight
/// off `combat.blocker_to_attacker`, which CR 506.4 prunes — goes empty. This
/// pairing is the primitive's entire reason to exist.
#[test]
fn history_survives_the_blocker_leaving_the_battlefield_while_the_live_relation_goes_empty() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let murder = scenario
        .add_spell_to_hand_from_oracle(P1, "Murder", true, MURDER)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    // Reach guard, before destroying the blocker.
    assert!(
        runner
            .state()
            .combat
            .as_ref()
            .expect("combat is live")
            .blocker_to_attacker
            .get(&blocker)
            .is_some_and(|a| a.contains(&attacker)),
        "reach guard: the live reverse lookup must name the attacker before the blocker dies"
    );

    give_priority(&mut runner, P1);
    runner.cast(murder).target_object(blocker).resolve();
    runner.advance_until_stack_empty();

    let state = runner.state();
    assert!(
        state.objects[&blocker].zone == Zone::Graveyard,
        "reach guard: the blocker must actually have died"
    );

    let ctx = FilterContext::from_source_with_controller(blocker, P1);
    let history_filter = combat_relation_filter(CombatRelation::BlockedBySubject {
        scope: CombatHistoryScope::ThisCombat,
    });
    let live_filter = combat_relation_filter(CombatRelation::BlockingOrBlockedBy);

    assert!(
        matches_target_filter(state, attacker, &history_filter, &ctx),
        "CR 509.1g + CR 400.7: the history relation must still answer after the blocker died"
    );
    assert!(
        !matches_target_filter(state, attacker, &live_filter, &ctx),
        "CR 506.4: the live relation must be pruned once the blocker leaves combat"
    );
}

/// T4: the look-back evaluator (`zone_change_record_matches_property`) answers
/// `BlockedBySubject` for a departed candidate, while `BlockingOrBlockedBy`
/// correctly fails closed there (E11).
#[test]
fn look_back_evaluator_answers_the_history_relation_for_a_departed_candidate() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let murder = scenario
        .add_spell_to_hand_from_oracle(P1, "Murder", true, MURDER)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    give_priority(&mut runner, P1);
    runner.cast(murder).target_object(attacker).resolve();
    runner.advance_until_stack_empty();

    let state = runner.state();
    let record = state
        .zone_changes_this_turn
        .iter()
        .rev()
        .find(|r| r.object_id == attacker && r.to_zone == Zone::Graveyard)
        .expect("reach guard: the attacker's death must be recorded as a zone change");

    let ctx = FilterContext::from_source_with_controller(blocker, P1);
    let history_filter = combat_relation_filter(CombatRelation::BlockedBySubject {
        scope: CombatHistoryScope::ThisCombat,
    });
    let live_filter = combat_relation_filter(CombatRelation::BlockingOrBlockedBy);

    assert!(
        matches_target_filter_on_zone_change_record(state, record, &history_filter, &ctx),
        "CR 509.1g + CR 608.2i: the look-back leg must answer from the block-history ledger"
    );
    assert!(
        !matches_target_filter_on_zone_change_record(state, record, &live_filter, &ctx),
        "CR 506.4: the live relation has nothing for a departed record to match"
    );
}

/// T5: `ThisCombat` and `ThisTurn` disagree across two combat phases in one
/// turn (CR 500.8). A block declared in combat 1 is absent from `ThisCombat`
/// once combat 2's fresh `CombatState` is installed, but still present in
/// `ThisTurn`. Built on the in-tree two-extra-combats harness
/// (`issue_828_full_throttle.rs::full_throttle_turn_advances_through_two_extra_combats`).
#[test]
fn this_combat_and_this_turn_disagree_across_two_combat_phases() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    // Asymmetric P/T so only the blocker dies in combat 1's damage step: a
    // dead attacker would leave no legal attacker for combat 2's
    // DeclareAttackers window to offer, and the engine skips straight past an
    // attacker-less declare-attackers step without ever raising it.
    let attacker = scenario.add_creature(P0, "Attacker", 3, 3).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let throttle = scenario
        .add_spell_to_hand_from_oracle(P0, "Full Throttle", false, FULL_THROTTLE)
        .with_mana_cost(ManaCost::generic(0))
        .id();

    let mut runner = scenario.build();
    runner.cast(throttle).resolve();
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().extra_phases.len(),
        2,
        "reach guard: Full Throttle must schedule two extra combats"
    );

    let mut declare_attackers_rounds = 0;
    let mut checked_combat_two = false;
    for _ in 0..600 {
        if checked_combat_two {
            break;
        }
        match runner.state().waiting_for.clone() {
            WaitingFor::DeclareAttackers { .. } => {
                declare_attackers_rounds += 1;
                if declare_attackers_rounds == 2 {
                    // Combat 2's `CombatState` was just installed fresh at this
                    // `Phase::BeginCombat` entry (§4.1: unconditional replacement).
                    let state = runner.state();
                    let combat = state.combat.as_ref().expect("combat 2 is live");
                    assert!(
                        !combat
                            .creature_blocked_attackers_this_combat
                            .contains_key(&blocker),
                        "combat 1's block must not survive into combat 2's fresh CombatState"
                    );
                    assert!(
                        state
                            .creature_blocked_attackers_this_turn
                            .get(&blocker)
                            .is_some_and(|a| a.contains(&ObjectIncarnationRef::from_object(
                                &state.objects[&attacker]
                            ))),
                        "reach guard: the turn-scoped ledger must still hold combat 1's block"
                    );

                    let ctx = FilterContext::from_source_with_controller(blocker, P1);
                    let this_combat = combat_relation_filter(CombatRelation::BlockedBySubject {
                        scope: CombatHistoryScope::ThisCombat,
                    });
                    let this_turn = combat_relation_filter(CombatRelation::BlockedBySubject {
                        scope: CombatHistoryScope::ThisTurn,
                    });
                    assert!(
                        !matches_target_filter(state, attacker, &this_combat, &ctx),
                        "ThisCombat must not see combat 1's block during combat 2"
                    );
                    assert!(
                        matches_target_filter(state, attacker, &this_turn, &ctx),
                        "ThisTurn must still see combat 1's block during combat 2 (CR 500.8)"
                    );
                    checked_combat_two = true;
                }
                let attacks = if declare_attackers_rounds == 1 {
                    vec![(attacker, AttackTarget::Player(P1))]
                } else {
                    vec![]
                };
                runner
                    .act(GameAction::DeclareAttackers {
                        attacks,
                        bands: vec![],
                    })
                    .expect("declare attackers");
            }
            WaitingFor::DeclareBlockers { .. } => {
                let assignments = if declare_attackers_rounds == 1 {
                    vec![(blocker, attacker)]
                } else {
                    vec![]
                };
                runner
                    .act(GameAction::DeclareBlockers { assignments })
                    .expect("declare blockers");
            }
            WaitingFor::Priority { .. } => {
                runner.pass_both_players();
            }
            _ => {
                runner.act(GameAction::PassPriority).ok();
            }
        }
    }

    assert!(
        checked_combat_two,
        "must have reached combat 2's DeclareAttackers window to assert the disagreement; \
         stalled at phase = {:?}, waiting_for = {:?}, declare_attackers_rounds = {declare_attackers_rounds}, \
         extra_phases = {:?}",
        runner.state().phase,
        runner.state().waiting_for,
        runner.state().extra_phases,
    );
}

/// T6: the turn-scoped block-history ledger clears at the turn boundary (E9),
/// exactly like its unary sibling `creatures_blocked_this_turn`.
#[test]
fn turn_boundary_clears_the_per_turn_block_history() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    // Reach guard: without this, deleting E9's clear would leave this test
    // green against a ledger that was empty either way.
    assert!(
        !runner
            .state()
            .creature_blocked_attackers_this_turn
            .is_empty(),
        "reach guard: the turn-scoped ledger must be populated before the turn boundary"
    );

    advance_past_end_of_turn(&mut runner);

    assert!(
        !runner
            .state()
            .creature_blocked_attackers_this_turn
            .contains_key(&blocker),
        "the turn-scoped ledger must clear at the next turn's boundary"
    );
}

/// T7: the combat-scoped ledger is unreachable once `state.combat` is `None`
/// (CR 511.3, the end of the end of combat step) — no edit of its own, a guard
/// for §4.1's lifetime claim.
#[test]
fn combat_scoped_history_is_gone_after_the_end_of_combat_step() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    assert!(
        !runner
            .state()
            .combat
            .as_ref()
            .expect("combat is live")
            .creature_blocked_attackers_this_combat
            .is_empty(),
        "reach guard: the combat-scoped ledger must be populated before combat ends"
    );

    advance_until_combat_ends(&mut runner);

    assert!(
        runner.state().combat.is_none(),
        "CR 511.3: combat must actually have ended for this test to mean anything"
    );
}

/// T9 — the CR 400.7 test. A blocked attacker that left and returned (blink)
/// is a new object at the same `ObjectId`; the ledger's captured incarnation
/// must not match its successor.
#[test]
fn an_attacker_that_left_and_returned_is_a_new_object_and_does_not_match() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let ephemerate = scenario
        .add_spell_to_hand_from_oracle(P0, "Ephemerate", true, EPHEMERATE)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    let ctx = FilterContext::from_source_with_controller(blocker, P1);
    let filter = combat_relation_filter(CombatRelation::BlockedBySubject {
        scope: CombatHistoryScope::ThisCombat,
    });

    // Positive control: before the blink, the relation matches.
    assert!(
        matches_target_filter(runner.state(), attacker, &filter, &ctx),
        "reach guard: the relation must match its own recorded incarnation"
    );

    let incarnation_before = runner.state().objects[&attacker].incarnation;
    runner.cast(ephemerate).target_object(attacker).resolve();
    runner.advance_until_stack_empty();
    let incarnation_after = runner.state().objects[&attacker].incarnation;
    assert_ne!(
        incarnation_after, incarnation_before,
        "reach guard: the blink must bump the attacker's incarnation (CR 400.7)"
    );

    assert!(
        !matches_target_filter(runner.state(), attacker, &filter, &ctx),
        "CR 400.7: a re-entered object is a new object the ledger never blocked"
    );
}

/// T12a: `CombatState::creature_blocked_attackers_this_combat` must participate
/// in `impl PartialEq for CombatState` (E3), or an omission from the loop-cover
/// gate (`analysis::resource::eq_except_growable`) would be indistinguishable
/// from the fields that omission is deliberate for.
#[test]
fn combat_scoped_block_history_participates_in_state_equality() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    let state = runner.state().clone();
    assert!(
        !state
            .combat
            .as_ref()
            .expect("combat is live")
            .creature_blocked_attackers_this_combat
            .is_empty(),
        "reach guard: the combat-scoped ledger must be populated"
    );

    let mut clone = state.clone();
    clone
        .combat
        .as_mut()
        .expect("combat is live")
        .creature_blocked_attackers_this_combat
        .clear();

    assert!(
        state != clone,
        "E3's PartialEq registration must make a cleared combat-scoped ledger visible"
    );
}

/// T12b: `GameState::creature_blocked_attackers_this_turn` must participate in
/// `impl PartialEq for GameState` (E4), the sibling of T12a on the turn-scoped
/// field.
#[test]
fn turn_scoped_block_history_participates_in_state_equality() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Attacker", 2, 2).id();
    let blocker = scenario.add_creature(P1, "Blocker", 2, 2).id();
    let mut runner = scenario.build();

    drive_declare_blockers(
        &mut runner,
        vec![(attacker, AttackTarget::Player(P1))],
        vec![],
        vec![(blocker, attacker)],
    );

    let state = runner.state().clone();
    assert!(
        !state.creature_blocked_attackers_this_turn.is_empty(),
        "reach guard: the turn-scoped ledger must be populated"
    );

    let mut clone = state.clone();
    clone.creature_blocked_attackers_this_turn.clear();

    assert!(
        state != clone,
        "E4's PartialEq registration must make a cleared turn-scoped ledger visible"
    );
}
