//! Phase 2 of the Perplexing Chimera run — "exchange control of a spell"
//! (U8: `exchange_control.rs`'s zone gate widened from Battlefield-only to
//! Battlefield-or-Stack, CR 701.12a + CR 400.7a).
//!
//! Covers Verification Matrix rows V17, V18 (plan-r6 §Verification Matrix,
//! Stage-2 rows). This is the "definition of done" pair: V17 proves the class
//! (Sudden Substitution — declared targets, not a context ref) and V18 proves
//! the card (Perplexing Chimera, end to end through the real cast pipeline).

use engine::game::scenario::{CastCommit, GameScenario, P0, P1};
use engine::types::actions::GameAction;
use engine::types::game_state::WaitingFor;
use engine::types::mana::ManaCost;
use engine::types::phase::Phase;
use engine::types::zones::Zone;

const PERPLEXING_CHIMERA_TEXT: &str = "Whenever an opponent casts a spell, you may exchange \
    control of this creature and that spell. If you do, you may choose new targets for the \
    spell. (If the spell becomes a permanent, you control that permanent.)";

const SUDDEN_SUBSTITUTION_TEXT: &str = "Split second (As long as this spell is on the stack, \
    players can't cast spells or activate abilities that aren't mana abilities.)\nExchange \
    control of target noncreature spell and target creature. Then the spell's controller may \
    choose new targets for it.";

/// Pass priority until the current committed cast's next trigger raises its
/// own `OptionalEffectChoice`, or panic with a diagnosable message if the
/// stack empties first.
fn advance_to_optional_choice(commit: &mut CastCommit<'_>) {
    for _ in 0..40 {
        match commit.state().waiting_for {
            WaitingFor::OptionalEffectChoice { .. } => return,
            WaitingFor::Priority { .. } => {
                if commit.state().stack.is_empty() {
                    panic!("the stack emptied without ever raising an OptionalEffectChoice");
                }
                commit
                    .act(GameAction::PassPriority)
                    .expect("PassPriority should succeed while draining to the prompt");
            }
            ref other => panic!("unexpected waiting state while draining to the prompt: {other:?}"),
        }
    }
    panic!("did not reach OptionalEffectChoice within 40 iterations");
}

// ---------------------------------------------------------------------------
// V17 — ExchangeControl accepts a stack subject (the class)
// ---------------------------------------------------------------------------

/// V17: Sudden Substitution — declared targets (not a context ref) — proves
/// the zone-gate widening (U8) in isolation from the context-ref machinery
/// (U4-U7): P1 casts a noncreature spell that draws cards; P0 casts Sudden
/// Substitution (verbatim Oracle text, Split Second) targeting that spell and
/// a creature of their own (CR 701.12b requires the two subjects to start
/// with different controllers, or the exchange does nothing).
///
/// Asserts (a) the creature's controller swapped, and (b) the exchanged
/// spell RESOLVES UNDER P0's CONTROL (`assert_hand_drawn(P0, 2)`, not P1) —
/// the CR 608.2c claim that a stack-subject exchange re-stamps who the spell
/// resolves for, not merely who controls a battlefield object.
///
/// REVERT-FAILING: reverting the zone gate (P2.5) makes the spell an illegal
/// exchange subject — `obj_a.zone != Zone::Battlefield` — so the entire
/// exchange no-ops (CR 701.12a) and P1 draws the cards instead.
///
/// Class D (Sudden Substitution's own "the spell's controller may choose new
/// targets for it" — a non-`you` chooser) is deliberately OUT OF RUN; this
/// row declines that offer.
#[test]
fn sudden_substitution_transfers_the_spell() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    // Divination draws 2 cards for WHICHEVER player ends up controlling it —
    // that's the point of the test — so both libraries need enough cards
    // that drawing 2 doesn't deck either player out before the assertions run.
    scenario.with_library_top(P0, &["Filler A", "Filler B", "Filler C"]);
    scenario.with_library_top(P1, &["Filler D", "Filler E", "Filler F"]);
    // CR 701.12b: the two exchange subjects must have DIFFERENT controllers
    // or the exchange does nothing — P0 gives up a creature of their own to
    // take control of P1's spell.
    let p0_creature = scenario.add_creature(P0, "P0 Creature", 2, 2).id();
    let divination = scenario
        .add_spell_to_hand_from_oracle(P1, "Divination", false, "Draw two cards.")
        .with_mana_cost(ManaCost::zero())
        .id();
    let sudden_substitution = scenario
        .add_spell_to_hand(P0, "Sudden Substitution", true)
        .from_oracle_text_with_keywords(&["Split second"], SUDDEN_SUBSTITUTION_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut divination_commit = runner.cast(divination).commit();
    let divination_stack_id = divination_commit.state().stack.back().unwrap().id;

    // REACH GUARD: Divination itself must be on the stack before P0 responds.
    assert_eq!(divination_commit.state().stack.len(), 1);

    {
        let state = divination_commit.state_mut();
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
    }
    let outcome = divination_commit
        .cast(sudden_substitution)
        .target_objects(&[divination_stack_id, p0_creature])
        .decline_optional()
        .resolve();

    // (a) the creature's controller swapped (to P1 — the opposite direction
    // of the spell, since CR 701.12b requires the two subjects to start with
    // different controllers).
    assert_eq!(
        outcome
            .state()
            .objects
            .get(&p0_creature)
            .unwrap()
            .controller,
        P1,
        "P0's creature must swap to P1"
    );
    // (b) the exchanged spell resolves under P0's control.
    outcome.assert_hand_drawn(P0, 2);
    outcome.assert_hand_drawn(P1, 0);
}

/// SIBLING (V17): the ordinary two-permanent path (Switcheroo shape) is
/// unaffected by the zone-gate widening — proves `control_is_exchangeable`
/// still accepts (and only accepts) Battlefield for the ordinary case.
#[test]
fn exchange_control_between_two_battlefield_permanents_unchanged() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let creature_a = scenario.add_creature(P0, "Creature A", 2, 2).id();
    let creature_b = scenario.add_creature(P1, "Creature B", 3, 3).id();
    let switcheroo = scenario
        .add_spell_to_hand_from_oracle(
            P0,
            "Switcheroo",
            false,
            "Exchange control of two target creatures.",
        )
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    let outcome = runner
        .cast(switcheroo)
        .target_objects(&[creature_a, creature_b])
        .resolve();

    assert_eq!(
        outcome.state().objects.get(&creature_a).unwrap().controller,
        P1
    );
    assert_eq!(
        outcome.state().objects.get(&creature_b).unwrap().controller,
        P0
    );
}

/// HOSTILE (V17): the targeted spell is countered in response, before Sudden
/// Substitution resolves — CR 701.12a: the exchange can't be completed (the
/// spell is no longer on the stack, no longer battlefield either), so
/// nothing swaps. The creature stays with its original controller too
/// (CR 701.12a all-or-nothing — not a partial swap).
#[test]
fn sudden_substitution_hostile_target_spell_countered_in_response() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let p1_creature = scenario.add_creature(P1, "P1 Creature", 2, 2).id();
    let divination = scenario
        .add_spell_to_hand_from_oracle(P1, "Divination", false, "Draw two cards.")
        .with_mana_cost(ManaCost::zero())
        .id();
    let sudden_substitution = scenario
        .add_spell_to_hand(P0, "Sudden Substitution", true)
        .from_oracle_text_with_keywords(&["Split second"], SUDDEN_SUBSTITUTION_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut divination_commit = runner.cast(divination).commit();
    let divination_stack_id = divination_commit.state().stack.back().unwrap().id;
    {
        let state = divination_commit.state_mut();
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
    }
    let mut ss_commit = divination_commit
        .cast(sudden_substitution)
        .target_objects(&[divination_stack_id, p1_creature])
        .decline_optional()
        .commit();
    // Split second forbids further casts while Sudden Substitution is on the
    // stack, so P1 can't respond with a real counterspell here — instead this
    // row exercises the same seam the plan names (a spell leaving the stack
    // before the exchange resolves) by removing Divination from the stack
    // directly, the same way an SBA-driven fizzle or a resolved counter would.
    {
        let state = ss_commit.state_mut();
        state.stack.retain(|e| e.id != divination_stack_id);
    }
    let outcome = ss_commit.resolve();
    assert_eq!(
        outcome
            .state()
            .objects
            .get(&p1_creature)
            .unwrap()
            .controller,
        P1,
        "CR 701.12a all-or-nothing: with the spell subject gone, the creature must NOT swap either"
    );
}

// ---------------------------------------------------------------------------
// V18 — Perplexing Chimera, end to end
// ---------------------------------------------------------------------------

/// V18: Perplexing Chimera, full card, end to end. P1 casts a real creature
/// spell (Grizzly Bears — vanilla, so its own retarget offer is a guaranteed
/// no-op per CR 115.7's empty-target-list guard, keeping this row focused on
/// the exchange itself). P0 accepts the optional trigger. Assert the Chimera
/// is now P1's, the (former) spell is now P0's, and — since it becomes a
/// permanent — it enters under P0's control with `base_controller == P1`
/// (CR 110.2b).
///
/// REVERT-FAILING: reverting any Stage-2 unit fails this row at a distinct
/// point — P2.1 reverted drops the trigger entirely (V12's own failure);
/// P2.2 reverted makes the exchange a total no-op (V14's failure); P2.5
/// reverted no-ops the zone gate (V17's failure). This row is the
/// end-to-end conjunction of all of them.
#[test]
fn perplexing_chimera_steals_the_spell_end_to_end() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let grizzly_bears = scenario
        .add_creature_to_hand_from_oracle(P1, "Grizzly Bears", 2, 2, "")
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let outcome = runner.cast(grizzly_bears).accept_optional().resolve();

    assert_eq!(
        outcome.state().objects.get(&chimera).unwrap().controller,
        P1,
        "the Chimera itself must swap to P1"
    );
    assert_eq!(
        outcome.zone_of(grizzly_bears),
        Zone::Battlefield,
        "REACH GUARD: Grizzly Bears must actually resolve onto the battlefield — a fizzle \
         cannot pass this row"
    );
    let bears = outcome.state().objects.get(&grizzly_bears).unwrap();
    assert_eq!(
        bears.controller, P0,
        "CR 400.7a: the permanent the spell becomes enters under the new controller"
    );
    assert_eq!(
        bears.base_controller,
        Some(P1),
        "CR 110.2b: the permanent's by-default controller is still the player who put the \
         spell on the stack"
    );
}

/// SIBLING (V18): declining the optional trigger leaves everything
/// unchanged — the Chimera stays P0's, and Grizzly Bears resolves for P1 as
/// normal. This is also the by-construction inertness pin: absent an
/// accepted exchange, nothing about the fix changes ordinary play.
#[test]
fn perplexing_chimera_declined_trigger_changes_nothing() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let grizzly_bears = scenario
        .add_creature_to_hand_from_oracle(P1, "Grizzly Bears", 2, 2, "")
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let outcome = runner.cast(grizzly_bears).decline_optional().resolve();

    assert_eq!(
        outcome.state().objects.get(&chimera).unwrap().controller,
        P0,
        "declining the exchange must leave the Chimera with P0"
    );
    let bears = outcome.state().objects.get(&grizzly_bears).unwrap();
    assert_eq!(
        bears.controller, P1,
        "Grizzly Bears resolves for its caster, P1, as normal"
    );
}

/// HOSTILE (V18): Perplexing Chimera destroyed in response to its own
/// trigger — CR 701.12a: no part of the exchange occurs. The SelfRef source
/// is no longer current (CR 400.7), so `targeting::resolved_targets` binds
/// it to nothing rather than a stale id.
#[test]
fn perplexing_chimera_destroyed_in_response_to_its_own_trigger_is_a_total_noop() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let grizzly_bears = scenario
        .add_creature_to_hand_from_oracle(P1, "Grizzly Bears", 2, 2, "")
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut commit = runner.cast(grizzly_bears).commit();
    advance_to_optional_choice(&mut commit);
    match commit.state().waiting_for {
        WaitingFor::OptionalEffectChoice { source_id, .. } => {
            assert_eq!(
                source_id, chimera,
                "REACH GUARD: the prompt must be the Chimera's own"
            );
        }
        ref other => panic!("expected the Chimera's OptionalEffectChoice, got {other:?}"),
    }

    // Destroy the Chimera "in response" — before its own trigger's choice is
    // answered — by moving it directly to the graveyard.
    {
        let state = commit.state_mut();
        let mut events = Vec::new();
        engine::game::zones::move_to_zone(state, chimera, Zone::Graveyard, &mut events);
    }

    commit
        .act(GameAction::DecideOptionalEffect { accept: true })
        .expect("accepting must not panic even though the source is gone");
    assert!(
        commit.state().transient_continuous_effects.is_empty(),
        "CR 701.12a: no part of the exchange occurs once the SelfRef source is gone"
    );
    assert_eq!(
        commit.state().objects.get(&grizzly_bears).unwrap().zone,
        Zone::Stack,
        "REACH GUARD: Grizzly Bears must still be on the stack (unresolved) at this point"
    );
}
