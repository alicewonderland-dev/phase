//! Phase 2 of the Perplexing Chimera run — "exchange control of a spell"
//! (U8: `exchange_control.rs`'s zone gate widened from Battlefield-only to
//! Battlefield-or-Stack, CR 701.12a + CR 400.7a).
//!
//! Covers Verification Matrix rows V17, V18 (plan-r6 §Verification Matrix,
//! Stage-2 rows). This is the "definition of done" pair: V17 proves the class
//! (Sudden Substitution — declared targets, not a context ref) and V18 proves
//! the card (Perplexing Chimera, end to end through the real cast pipeline).

use engine::game::scenario::{CastCommit, GameScenario, P0, P1};
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::card_type::CoreType;
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

/// The unrestricted single-target pump clause. A TARGETED triggering spell is
/// required for the Chimera rows: `change_targets` takes its
/// `current_targets.is_empty()` no-op arm for a vanilla creature spell, which
/// makes the retarget prompt structurally unreachable and the assertion vacuous.
const PUMP_TEXT: &str = "Target creature gets +1/+1 until end of turn.";

/// Verbatim from `client/public/card-data.json`.
const GILDED_DRAKE_TEXT: &str = "Flying\nWhen this creature enters, exchange control of this \
    creature and up to one target creature an opponent controls. If you don't or can't make an \
    exchange, sacrifice this creature. This ability still resolves if its target becomes illegal.";

/// The generic "steal a creature for the turn" clause, used to stage the CR
/// 701.12b same-controller collision through a REAL continuous effect. Writing
/// `state.objects[drake].controller = P1` directly does not work: the layer
/// flush at the end of resolution reverts it before any assertion can read it.
const GAIN_CONTROL_TEXT: &str = "Gain control of target creature until end of turn.";

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
    // CR 608.2b + CR 701.12a — INDEX-DISCIPLINE PIN for
    // `ability_utils::validate_targets_in_chain`'s ExchangeControl arm. That
    // arm PRUNES the illegal slot-A target, which slides the surviving slot-B
    // target into slot A's position; `exchange_control::resolve_slot` then
    // reads the creature into slot A and finds nothing for slot B. Pruning is
    // safe only because that second lookup runs dry and takes CR 701.12a's
    // all-or-nothing early return BEFORE any continuous effect is written.
    // Asserting "no continuous effect at all" (not merely "the creature kept
    // its controller") is what makes a future partially-completable
    // ExchangeControl fail here instead of silently exchanging the wrong
    // object.
    assert!(
        outcome.state().transient_continuous_effects.is_empty(),
        "no partial or mis-bound exchange may be written when one subject is illegal"
    );
}

/// SIBLING (V17) — CR 608.2b re-validation is now filter-aware for the
/// ORDINARY two-permanent path, not just the stack-subject one.
///
/// Before this change `ExchangeControl` fell to `validate_targets_in_chain`'s
/// generic `None` branch, which re-checked only `state.battlefield.contains`.
/// A target that stayed on the battlefield but stopped satisfying the
/// ability's own filter therefore survived re-validation and got exchanged.
/// The dedicated arm re-validates against each declared filter, so a creature
/// that is no longer a creature when Switcheroo resolves is illegal
/// (CR 608.2b: "its characteristics may have changed"), and with one subject
/// illegal the exchange can't be completed (CR 701.12a).
///
/// This row exists because the arm changes re-validation for EVERY card that
/// parses to `ExchangeControl`, not only the two cards in this run's scope.
///
/// REVERT-FAILING: restoring the generic battlefield-only check makes the
/// de-typed permanent a legal target again and both controllers swap.
#[test]
fn exchange_control_target_that_stops_matching_its_filter_is_illegal_on_resolution() {
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
    let mut commit = runner
        .cast(switcheroo)
        .target_objects(&[creature_a, creature_b])
        .commit();

    // REACH GUARD: both targets must have been accepted at announcement, or
    // this row would prove nothing about RESOLUTION-time re-validation.
    assert_eq!(
        commit.state().stack.len(),
        1,
        "REACH GUARD: Switcheroo must be on the stack with its two targets"
    );

    // "In response", creature B stops being a creature (it stays on the
    // battlefield, so the old battlefield-only check would still accept it).
    {
        let state = commit.state_mut();
        state
            .objects
            .get_mut(&creature_b)
            .expect("creature B exists")
            .card_types
            .core_types = vec![CoreType::Artifact];
    }

    let outcome = commit.resolve();

    assert!(
        outcome.state().transient_continuous_effects.is_empty(),
        "CR 608.2b + CR 701.12a: with one subject no longer matching its filter, no part of \
         the exchange occurs"
    );
    assert_eq!(
        outcome.state().objects.get(&creature_a).unwrap().controller,
        P0,
        "creature A must keep its controller"
    );
    assert_eq!(
        outcome.state().objects.get(&creature_b).unwrap().controller,
        P1,
        "creature B must keep its controller"
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
///
/// The triggering spell is a TARGETED one. With the vanilla creature spell
/// this row used to cast, `change_targets` took its `current_targets.is_empty()`
/// no-op arm and the retarget prompt was structurally unreachable — so the row
/// could not observe the "If you do" defect at all. A targeted spell makes the
/// prompt reachable, which is what turns the added assertion below into a real
/// discriminator rather than a tautology.
#[test]
fn perplexing_chimera_destroyed_in_response_to_its_own_trigger_is_a_total_noop() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let p0_bear = scenario.add_creature(P0, "P0 Bear", 2, 2).id();
    let p1_bear = scenario.add_creature(P1, "P1 Bear", 2, 2).id();
    let pump = scenario
        .add_spell_to_hand_from_oracle(P1, "Giant Growth", true, PUMP_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut commit = runner.cast(pump).target_objects(&[p1_bear]).commit();
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
        commit.state().objects.get(&pump).unwrap().zone,
        Zone::Stack,
        "REACH GUARD: the triggering spell must still be on the stack (unresolved) here"
    );

    // CR 608.2c: and no retarget offer is EVER raised, all the way to the end
    // of the resolution. Pre-fix the accept latch left the outcome flag true,
    // so "If you do, you may choose new targets for the spell" fired after an
    // exchange CR 701.12a had refused. Drained by hand rather than through a
    // declining policy, because a policy that auto-declines the second offer
    // would answer the very prompt this row exists to prove is never raised.
    for _ in 0..40 {
        match &commit.state().waiting_for {
            WaitingFor::RetargetChoice { .. } => {
                panic!("a retarget offer was raised for an exchange that never happened")
            }
            WaitingFor::OptionalEffectChoice { source_id, .. } => panic!(
                "a second optional offer was raised for an exchange that never happened \
                 (source {source_id:?})"
            ),
            WaitingFor::Priority { .. } => {
                if commit.state().stack.is_empty() {
                    break;
                }
                commit
                    .act(GameAction::PassPriority)
                    .expect("PassPriority should succeed while draining");
            }
            other => panic!("unexpected state while draining to the end: {other:?}"),
        }
    }

    // REACH GUARD: the spell really did resolve for its own caster, so the
    // drain above was not short-circuited by a fizzle.
    assert_eq!(
        commit.state().objects.get(&pump).unwrap().zone,
        Zone::Graveyard,
        "REACH GUARD: the triggering spell must have resolved"
    );
    assert_eq!(
        commit.state().objects.get(&p0_bear).unwrap().controller,
        P0,
        "P0's creature was never part of any exchange"
    );
    assert_eq!(commit.state().objects.get(&p1_bear).unwrap().controller, P1);
}

// ---------------------------------------------------------------------------
// V1 / V2 — an accepted Chimera trigger whose exchange did not occur
// ---------------------------------------------------------------------------

/// V1 — CR 701.12a + CR 608.2c: accepting the Chimera's "you may exchange
/// control of this creature and that spell" does NOT entitle you to the
/// printed "If you do, you may choose new targets for the spell" when the
/// exchange could not be made.
///
/// Production entry chain: `resolve_optional_effect_decision` (accept — which
/// LOWERS `optional` and LATCHES the performed flag true) → `resolve_ability_chain`
/// → the resolver-verdict block → the sub descent → `evaluate_condition`'s
/// `EffectOutcome { OptionalEffectPerformed }` arm.
///
/// First production branch reached: `exchange_control.rs`'s
/// `let Some(id_a) = resolve_slot(target_a, ..) else` arm — `resolved_targets`
/// binds nothing for `TargetFilter::SelfRef` once CR 400.7's currency check
/// fails on a Chimera that has left the battlefield.
///
/// REVERT-FAILING: pre-fix, nothing downstream of the accept latch ever lowers
/// the flag, so the gate reads true and the engine raises the "you may choose
/// new targets" offer (itself optional, hence a SECOND `OptionalEffectChoice`)
/// and then `RetargetChoice` — handing P0 a free retarget of an opponent's
/// spell it never gained control of. Post-fix the sub is skipped outright.
#[test]
fn an_accepted_chimera_exchange_that_did_not_happen_offers_no_retarget() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let _p0_bear = scenario.add_creature(P0, "P0 Bear", 2, 2).id();
    let p1_bear = scenario.add_creature(P1, "P1 Bear", 2, 2).id();
    let pump = scenario
        .add_spell_to_hand_from_oracle(P1, "Giant Growth", true, PUMP_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut commit = runner.cast(pump).target_objects(&[p1_bear]).commit();
    advance_to_optional_choice(&mut commit);

    // Destroy the Chimera in response, so the exchange CANNOT be made
    // (CR 701.12a all-or-nothing).
    {
        let state = commit.state_mut();
        let mut events = Vec::new();
        engine::game::zones::move_to_zone(state, chimera, Zone::Graveyard, &mut events);
    }
    commit
        .act(GameAction::DecideOptionalEffect { accept: true })
        .expect("accepting the exchange must be legal");

    // THE DISCRIMINATOR, read at the prompt boundary: the very next thing the
    // engine asks for is neither the "you may choose new targets" offer nor the
    // retarget itself.
    match commit.state().waiting_for {
        WaitingFor::OptionalEffectChoice { source_id, .. } => panic!(
            "a second optional offer was raised for an exchange that never happened \
             (source {source_id:?}) — the \"If you do\" gate read true"
        ),
        WaitingFor::RetargetChoice { .. } => {
            panic!("a retarget offer was raised for an exchange that never happened")
        }
        _ => {}
    }

    // CO-WITNESS: nothing was exchanged.
    assert!(
        commit.state().transient_continuous_effects.is_empty(),
        "CR 701.12a: no part of the exchange occurs"
    );
}

/// V2 — V1's PAIRED POSITIVE REACH GUARD. The same board WITHOUT the destroy
/// still reaches the retarget offer, so V1's two negatives cannot pass because
/// the fixture never built a targeted spell or never reached the gate at all.
#[test]
fn an_accepted_chimera_exchange_that_happened_still_offers_the_retarget() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let p0_bear = scenario.add_creature(P0, "P0 Bear", 2, 2).id();
    let p1_bear = scenario.add_creature(P1, "P1 Bear", 2, 2).id();
    let pump = scenario
        .add_spell_to_hand_from_oracle(P1, "Giant Growth", true, PUMP_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut commit = runner.cast(pump).target_objects(&[p1_bear]).commit();

    let mut reached = None;
    for _ in 0..40 {
        match &commit.state().waiting_for {
            WaitingFor::RetargetChoice {
                player,
                current_targets,
                legal_new_targets,
                ..
            } => {
                reached = Some((*player, current_targets.clone(), legal_new_targets.clone()));
                break;
            }
            WaitingFor::OptionalEffectChoice { .. } => {
                commit
                    .act(GameAction::DecideOptionalEffect { accept: true })
                    .expect("accepting must succeed");
            }
            WaitingFor::Priority { .. } => {
                assert!(
                    !commit.state().stack.is_empty(),
                    "the stack emptied before the retarget offer was raised"
                );
                commit
                    .act(GameAction::PassPriority)
                    .expect("PassPriority should succeed while draining");
            }
            other => panic!("unexpected state while draining to the retarget offer: {other:?}"),
        }
    }
    let (chooser, current, pool) = reached.expect("REACH GUARD: the retarget offer must be raised");

    // REACH GUARD: the exchange really happened.
    assert_eq!(
        commit.state().objects.get(&chimera).unwrap().controller,
        P1,
        "REACH GUARD: the Chimera must have swapped to P1"
    );
    assert_eq!(
        commit.state().transient_continuous_effects.len(),
        2,
        "CR 613.1b: the exchange installs one ChangeController effect per subject"
    );

    assert_eq!(chooser, P0, "CR 115.7: the spell's new controller chooses");
    assert_eq!(
        current,
        vec![TargetRef::Object(p1_bear)],
        "the offer is made against the spell's existing target"
    );
    assert!(
        pool.contains(&TargetRef::Object(p0_bear)),
        "an unrestricted \"target creature\" pool must offer P0's own creature too \
         (pool was {pool:?})"
    );
}

/// BLAST-RADIUS PIN (review round 2) — Gilded Drake's disposition when its
/// sole declared target becomes illegal while staying on the battlefield.
///
/// `validate_targets_in_chain`'s `ExchangeControl` arm re-validates against
/// each declared filter, where the generic branch it replaced checked only
/// `state.battlefield.contains`. For Gilded Drake ("exchange control of this
/// creature and up to one target creature an opponent controls. If you don't
/// or can't make an exchange, sacrifice this creature.") that flips the
/// outcome when the target stops being a creature in response:
///
///   * BEFORE — the target survived re-validation, so the ability resolved
///     and the exchange RAN against an illegal target. Plainly wrong.
///   * AFTER  — the target is illegal, it is this ability's only instance of
///     the word "target", so per CR 608.2b the ability doesn't resolve. This
///     is the correct DEFAULT, and it is what this row pins.
///
/// KNOWN GAP, deliberately not fixed here: Gilded Drake's printed "This
/// ability still resolves if its target becomes illegal" is an explicit CR
/// 608.2b exception that the parser does not model at all — the clause is
/// dropped, and `optional_targeting` is `false` despite "up to one target".
/// With it modelled, the ability would resolve, the exchange would not
/// happen, and the Drake would be sacrificed. Representing that exception is
/// a parser + AST change well outside this run; this row exists so the
/// current disposition is a recorded decision rather than an unnoticed side
/// effect, and so it fails loudly when the exception is implemented.
#[test]
fn gilded_drake_sole_target_that_stops_matching_its_filter_stops_the_ability() {
    use engine::game::ability_utils::validate_targets_in_chain;
    use engine::game::zones::create_object;
    use engine::types::ability::{
        ControllerRef, Effect, QuantityExpr, ResolvedAbility, TargetFilter, TargetRef, TypedFilter,
    };
    use engine::types::card_type::CoreType;
    use engine::types::game_state::GameState;
    use engine::types::identifiers::CardId;

    let mut state = GameState::new_two_player(42);
    let drake = create_object(
        &mut state,
        CardId(1),
        P0,
        "Gilded Drake".to_string(),
        Zone::Battlefield,
    );
    let victim = create_object(
        &mut state,
        CardId(2),
        P1,
        "Victim".to_string(),
        Zone::Battlefield,
    );

    let mut ability = ResolvedAbility::new(
        Effect::ExchangeControl {
            target_a: TargetFilter::SelfRef,
            target_b: TargetFilter::Typed(
                TypedFilter::creature().controller(ControllerRef::Opponent),
            ),
        },
        vec![TargetRef::Object(victim)],
        drake,
        P0,
    );
    ability.sub_ability = Some(Box::new(ResolvedAbility::new(
        Effect::Sacrifice {
            target: TargetFilter::SelfRef,
            count: QuantityExpr::Fixed { value: 1 },
            min_count: 0,
        },
        vec![],
        drake,
        P0,
    )));

    // REACH GUARD: while the victim IS a creature the target is kept, so this
    // row is exercising the re-validation seam and not an unrelated drop.
    state
        .objects
        .get_mut(&victim)
        .unwrap()
        .card_types
        .core_types = vec![CoreType::Creature];
    assert_eq!(
        validate_targets_in_chain(&state, &ability).targets,
        vec![TargetRef::Object(victim)],
        "REACH GUARD: a legal creature target must survive re-validation"
    );

    // The victim stays on the battlefield but stops being a creature — the
    // exact case the old battlefield-presence-only check let through.
    state
        .objects
        .get_mut(&victim)
        .unwrap()
        .card_types
        .core_types = vec![CoreType::Artifact];

    let validated = validate_targets_in_chain(&state, &ability);
    assert!(
        validated.targets.is_empty(),
        "CR 608.2b: a target that no longer matches its filter is illegal, so this ability's \
         only target is illegal"
    );
    // ...and carry the claim the test's name makes all the way to the
    // disposition rather than stopping one inference short of it. CR 608.2b:
    // all targets illegal ⇒ the ability doesn't resolve, so the "otherwise
    // sacrifice this creature" rider never runs. Asserting it here also fails
    // loudly if `check_fizzle`'s contract changes underneath this row.
    assert!(
        engine::game::targeting::check_fizzle(&[TargetRef::Object(victim)], &validated.targets),
        "CR 608.2b: with its only target illegal the ability does not resolve"
    );
}

/// REGRESSION (final review) — an `ExchangeControl` node must never bind an
/// object that was not one of its own two claimed targets.
///
/// `validate_targets_in_chain`'s `ExchangeControl` arm prunes illegal targets,
/// which shifts survivors toward index 0. `exchange_control::resolve` consumes
/// `ability.targets` positionally with no per-slot recheck, so ANY entry left
/// in the list is bindable into one of the two exchange slots. An earlier
/// revision of this arm appended unclaimed propagated entries after the
/// survivors (mirroring the `Attach` arm); with `[A_illegal, B_legal,
/// C_propagated]` that produced `[B, C]` and the resolver exchanged control of
/// B and C — a pair the spell never targeted together.
///
/// Correct outcome: only `B` survives, the second `resolve_slot` runs dry, and
/// CR 701.12a's all-or-nothing rule makes the whole thing a no-op.
///
/// REVERT-FAILING: re-adding `kept.extend(target_iter.cloned())` to that arm
/// makes this row exchange B and C and fail on the emptiness assertion.
#[test]
fn exchange_control_ignores_unclaimed_propagated_targets() {
    use engine::game::ability_utils::validate_targets_in_chain;
    use engine::game::effects::exchange_control;
    use engine::game::zones::create_object;
    use engine::types::ability::{Effect, ResolvedAbility, TargetFilter, TargetRef, TypedFilter};
    use engine::types::card_type::CoreType;
    use engine::types::game_state::GameState;
    use engine::types::identifiers::CardId;

    let mut state = GameState::new_two_player(42);
    let source = create_object(
        &mut state,
        CardId(1),
        P0,
        "Source".into(),
        Zone::Battlefield,
    );
    let illegal = create_object(
        &mut state,
        CardId(2),
        P0,
        "Illegal A".into(),
        Zone::Battlefield,
    );
    let legal = create_object(
        &mut state,
        CardId(3),
        P1,
        "Legal B".into(),
        Zone::Battlefield,
    );
    let propagated = create_object(
        &mut state,
        CardId(4),
        P0,
        "Propagated C".into(),
        Zone::Battlefield,
    );

    // A is on the battlefield but is NOT a creature, so it fails the declared
    // filter at CR 608.2b re-validation. B and C both satisfy it.
    state
        .objects
        .get_mut(&illegal)
        .unwrap()
        .card_types
        .core_types = vec![CoreType::Artifact];
    for id in [legal, propagated] {
        state.objects.get_mut(&id).unwrap().card_types.core_types = vec![CoreType::Creature];
    }

    let ability = ResolvedAbility::new(
        Effect::ExchangeControl {
            target_a: TargetFilter::Typed(TypedFilter::creature()),
            target_b: TargetFilter::Typed(TypedFilter::creature()),
        },
        vec![
            TargetRef::Object(illegal),
            TargetRef::Object(legal),
            TargetRef::Object(propagated),
        ],
        source,
        P0,
    );

    let validated = validate_targets_in_chain(&state, &ability);
    // REACH GUARD: the illegal target really was dropped, and the unclaimed
    // third entry really was not carried forward — without this the row could
    // pass for the wrong reason (e.g. nothing was pruned at all).
    assert_eq!(
        validated.targets,
        vec![TargetRef::Object(legal)],
        "only the surviving claimed target is kept; the unclaimed third entry is not appended"
    );

    let mut events = Vec::new();
    exchange_control::resolve(&mut state, &validated, &mut events).unwrap();
    assert!(
        state.transient_continuous_effects.is_empty(),
        "CR 701.12a: with only one subject bindable the exchange can't complete, so no part of \
         it occurs — and in particular C, which was never a target of this effect, is untouched"
    );
}

/// V18 SIBLING (final review) — Perplexing Chimera's SECOND clause. After the
/// exchange, "you may choose new targets for the spell" must enumerate the
/// replacement pool against the spell's NEW controller.
///
/// The card's printed ruling is explicit: "The change of control happens before
/// new targets are chosen, so any targeting restrictions such as 'target
/// opponent' or 'target creature you control' are now made in reference to you,
/// not the spell's original controller."
///
/// This was wrong until the `pool_controller` binding in
/// `change_targets::legal_new_targets_for_entry`: the exchange installs a
/// layer-2 `ChangeController` on the OBJECT, while `ResolvedAbility.controller`
/// stays the caster until `stack::resolve_top` re-stamps it — which happens
/// after the retarget window has already closed. The pool was therefore built
/// for P1 while the chooser was P0.
///
/// MEASURED before the fix: `legal_new_targets == [chimera, p1_creature]` — the
/// creatures P1 controls, with P0's own creature absent, so P0 could not make
/// the one choice the ruling entitles them to.
///
/// The other Chimera rows cannot catch this: `perplexing_chimera_steals_the_
/// spell_end_to_end` deliberately uses vanilla Grizzly Bears so the retarget
/// offer is a guaranteed no-op, and `chimera_retarget_subject_binds_to_the_
/// triggering_spell` uses a filter with no `ControllerRef`. A controller-
/// relative filter is required to distinguish the two controllers at all.
#[test]
fn chimera_retarget_pool_is_built_for_the_new_controller() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    let p0_creature = scenario.add_creature(P0, "P0 Bear", 2, 2).id();
    let p1_creature = scenario.add_creature(P1, "P1 Bear", 2, 2).id();
    let guile = scenario
        .add_spell_to_hand_from_oracle(
            P1,
            "Ranger's Guile",
            true,
            "Target creature you control gets +1/+1 until end of turn.",
        )
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P1;
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    let mut commit = runner
        .cast(guile)
        .target_objects(&[p1_creature])
        .accept_optional()
        .commit();

    // Drain to the retarget prompt, accepting the Chimera trigger on the way.
    let mut reached = None;
    for _ in 0..40 {
        match &commit.state().waiting_for {
            WaitingFor::RetargetChoice {
                player,
                legal_new_targets,
                ..
            } => {
                reached = Some((*player, legal_new_targets.clone()));
                break;
            }
            WaitingFor::OptionalEffectChoice { .. } => {
                commit
                    .act(GameAction::DecideOptionalEffect { accept: true })
                    .expect("accepting the Chimera trigger must succeed");
            }
            WaitingFor::Priority { .. } => {
                assert!(
                    !commit.state().stack.is_empty(),
                    "the stack emptied before the retarget prompt was raised"
                );
                commit
                    .act(GameAction::PassPriority)
                    .expect("PassPriority should succeed while draining");
            }
            other => panic!("unexpected state while draining to the retarget prompt: {other:?}"),
        }
    }
    let (chooser, pool) = reached.expect("REACH GUARD: the retarget prompt must be raised");

    // REACH GUARD: the exchange really happened, so this row is measuring the
    // post-steal pool and not a pre-steal one.
    assert_eq!(
        commit.state().objects.get(&chimera).unwrap().controller,
        P1,
        "REACH GUARD: the Chimera must have swapped to P1"
    );
    assert_eq!(chooser, P0, "the new controller chooses the new targets");

    assert!(
        pool.contains(&TargetRef::Object(p0_creature)),
        "\"target creature you control\" must now mean P0's creatures — P0's own creature \
         must be offered (pool was {pool:?})"
    );
    assert!(
        !pool.contains(&TargetRef::Object(p1_creature)),
        "P1's creature must NOT be offered — the restriction is read against P0 now \
         (pool was {pool:?})"
    );
    assert!(
        !pool.contains(&TargetRef::Object(chimera)),
        "the Chimera is P1's after the swap, so it must not be in P0's pool \
         (pool was {pool:?})"
    );
}

// ---------------------------------------------------------------------------
// V5 — Gilded Drake's "if you don't or can't make an exchange" rider
// ---------------------------------------------------------------------------

/// Stage Gilded Drake's ETB trigger onto the stack with `bear` chosen as its
/// declared target, and hand priority to P1 so a response can be cast.
///
/// Shared by V5 and its positive control so the two rows differ in exactly one
/// thing — whether the response is cast — and nothing else.
fn stage_gilded_drake_trigger(
    runner: &mut engine::game::scenario::GameRunner,
    drake: engine::types::identifiers::ObjectId,
) -> CastCommit<'_> {
    let mut commit = runner.cast(drake).commit();
    let mut staged = false;
    for _ in 0..40 {
        let on_battlefield =
            commit.state().objects.get(&drake).map(|obj| obj.zone) == Some(Zone::Battlefield);
        if on_battlefield && commit.state().stack.len() == 1 {
            staged = true;
            break;
        }
        match commit.state().waiting_for {
            WaitingFor::Priority { .. } => {
                assert!(
                    !commit.state().stack.is_empty(),
                    "the stack emptied before the Drake's ETB trigger could be staged"
                );
                commit
                    .act(GameAction::PassPriority)
                    .expect("PassPriority should succeed while staging the trigger");
            }
            ref other => panic!("unexpected waiting state while staging the trigger: {other:?}"),
        }
    }
    // REACH GUARD: the Drake really is on the battlefield and its ETB trigger
    // really is on the stack, unresolved — the only window in which a response
    // can change who controls the Drake before the exchange resolves.
    //
    // NOTE the trigger raises no `TriggerTargetSelection` here: "up to one
    // target creature an opponent controls" has exactly one legal choice on
    // this board, so the engine binds it without prompting. The two rows below
    // assert on the bound target's disposition instead.
    assert!(
        staged,
        "REACH GUARD: the Drake must be on the battlefield with its ETB trigger on the stack"
    );
    {
        let state = commit.state_mut();
        state.priority_player = P1;
        state.waiting_for = WaitingFor::Priority { player: P1 };
    }
    commit
}

fn sacrifice_resolutions(
    outcome: &engine::game::scenario::CastOutcome,
    source: engine::types::identifiers::ObjectId,
) -> usize {
    use engine::types::ability::EffectKind;
    use engine::types::events::GameEvent;
    outcome
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event,
                GameEvent::EffectResolved {
                    kind: EffectKind::Sacrifice,
                    source_id,
                    ..
                } if *source_id == source
            )
        })
        .count()
}

/// V5 — CR 701.12b + CR 608.2c: "If you don't or can't make an exchange,
/// sacrifice this creature."
///
/// P0 casts Gilded Drake targeting P1's bear. **In response to the ETB
/// trigger**, P1 casts "Gain control of target creature until end of turn."
/// on the Drake itself. By the time the trigger resolves, P1 controls BOTH
/// subjects, so CR 701.12b makes the exchange do nothing — and the printed
/// rider must fire.
///
/// The declared target survives CR 608.2b re-validation because the ability's
/// controller is still P0: **CR 603.3a** — "a triggered ability is controlled
/// by the player who controlled its source at the time it triggered" — so a
/// control change of the SOURCE does not re-seat it, and "target creature an
/// opponent controls" is still read against P0. Slot A is `SelfRef`, whose
/// currency check (CR 400.7) is zone/incarnation-based, not controller-based,
/// so P1 gaining control of the Drake does not unbind it either.
///
/// **THE REVERT-FAILING ASSERTION** is the `EffectResolved { Sacrifice }` in
/// the event trail. It cannot be a board delta: the sacrifice itself is a
/// legitimate no-op here (CR 701.21a — the Drake's controller is P1, not the
/// ability's controller P0, so `sacrifice::resolve`'s controller guard skips
/// it), which makes the board BYTE-IDENTICAL pre- and post-fix. Pre-fix
/// `mandatory_parent_effect_performed` fell into `_ => true`, the
/// `Not(IfYouDo)` gate read false, and the sub never ran at all.
///
/// NEGATIVE CO-ASSERTION — no `PermanentSacrificed` at all. Once this sub
/// actually runs, the walker propagates the parent's declared target (the
/// BEAR) into the `Sacrifice { target: SelfRef }` sub, and only the CR 701.21a
/// controller guard stops the Drake's rider from eating P1's bear. If that
/// guard is ever weakened this row fails loudly instead of silently
/// sacrificing the wrong permanent.
#[test]
fn gilded_drake_sacrifice_rider_fires_when_the_exchange_does_nothing() {
    use engine::types::ability::ContinuousModification;
    use engine::types::events::GameEvent;

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let drake = scenario
        .add_creature_to_hand_from_oracle(P0, "Gilded Drake", 3, 3, GILDED_DRAKE_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();
    let steal = scenario
        .add_spell_to_hand_from_oracle(P1, "Seize the Drake", true, GAIN_CONTROL_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P0;
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
    }

    let mut commit = stage_gilded_drake_trigger(&mut runner, drake);
    let outcome = commit
        .cast(steal)
        .target_objects(&[drake])
        .decline_optional()
        .resolve();

    // REACH GUARD: the response actually landed, so the CR 701.12b collision
    // this row depends on really was staged.
    assert_eq!(
        outcome.state().objects.get(&drake).unwrap().controller,
        P1,
        "REACH GUARD: P1 must control the Drake when the exchange resolves"
    );
    assert_eq!(
        outcome.state().objects.get(&bear).unwrap().controller,
        P1,
        "REACH GUARD: the bear stays P1's, so both subjects share a controller \
         (CR 701.12b) and the exchange does nothing"
    );

    // THE DISCRIMINATOR.
    assert_eq!(
        sacrifice_resolutions(&outcome, drake),
        1,
        "CR 608.2c: the \"if you don't or can't make an exchange\" rider must RESOLVE \
         exactly once (events were {:?})",
        outcome.events()
    );

    // NEGATIVE CO-ASSERTION.
    assert!(
        !outcome
            .events()
            .iter()
            .any(|event| matches!(event, GameEvent::PermanentSacrificed { .. })),
        "CR 701.21a: nothing may actually be sacrificed — the Drake is P1's, and the bear \
         was never this rider's subject (events were {:?})",
        outcome.events()
    );

    // The exchange installed no Layer-2 control effect of its own; the only
    // transient effect present is the gain-control spell's.
    let drake_sourced: Vec<_> = outcome
        .state()
        .transient_continuous_effects
        .iter()
        .filter(|effect| {
            effect.source_id == drake
                && effect
                    .modifications
                    .contains(&ContinuousModification::ChangeController)
        })
        .collect();
    assert!(
        drake_sourced.is_empty(),
        "CR 701.12a/b: a no-op exchange installs no control effect of its own"
    );
}

/// V5's PAIRED POSITIVE CONTROL — the same scenario with P1 declining to
/// respond. The exchange genuinely happens, so the `Not(IfYouDo)` rider must
/// NOT fire. Without this row, a fix that over-suppressed (or a fixture that
/// never reached the trigger at all) would pass V5 silently.
#[test]
fn gilded_drake_sacrifice_rider_stays_silent_when_the_exchange_happens() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let drake = scenario
        .add_creature_to_hand_from_oracle(P0, "Gilded Drake", 3, 3, GILDED_DRAKE_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P0;
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
    }

    let commit = stage_gilded_drake_trigger(&mut runner, drake);
    let outcome = commit.resolve();

    // CR 701.12b: different controllers, so the exchange really happens.
    assert_eq!(
        outcome.state().objects.get(&drake).unwrap().controller,
        P1,
        "the Drake goes to the opponent"
    );
    assert_eq!(
        outcome.state().objects.get(&bear).unwrap().controller,
        P0,
        "and their creature comes back"
    );
    assert_eq!(
        sacrifice_resolutions(&outcome, drake),
        0,
        "CR 608.2c: an exchange that HAPPENED must not fire the \"if you don't or can't\" \
         rider (events were {:?})",
        outcome.events()
    );
}

// ---------------------------------------------------------------------------
// V8 — blast radius of the new `ControllerChanged` event
// ---------------------------------------------------------------------------

/// Verbatim from `client/public/card-data.json`. The middle clause parses to a
/// `TriggerMode::ChangesController` trigger with `valid_card: SelfRef` — one of
/// only four printed producers of that mode, and the reason this row exists.
const KHARN_TEXT: &str = "Berzerker — Khârn the Betrayer attacks or blocks each combat if \
    able.\nSigil of Corruption — When you lose control of Khârn the Betrayer, draw two \
    cards.\nThe Betrayer — If damage would be dealt to Khârn the Betrayer, prevent that damage \
    and an opponent of your choice gains control of it.";

/// Verbatim from `client/public/card-data.json`.
const SWITCHEROO_TEXT: &str = "Exchange control of two target creatures.";

/// V8 POSITIVE — CR 603.2 + CR 613.1b: now that `exchange_control::resolve`
/// publishes `ControllerChanged`, a "When you lose control of ~" trigger fires
/// on an exchange, exactly once — and, per PR #8332 round 1 (U3), for the
/// correct player.
///
/// Production entry chain: `exchange_control::resolve` → `collect_pending_triggers`
/// → `trigger_index.rs`'s `ControllerChanged{..} => TriggerEventKey::ChangesController`
/// (the gate that makes the matcher reachable at all) → `match_changes_controller`
/// → `collect_matching_triggers_inner`'s CR 603.10d + CR 603.3a controller
/// derivation (`triggers.rs`).
///
/// REVERT-FAILING (two independent legs): without the `ControllerChanged`
/// emission the event never exists, no `ChangesController` key is ever
/// pushed, and nobody draws (0/0). Without U3's controller derivation, the
/// trigger still fires exactly once but for the WRONG player — the gainer
/// (P1) instead of the loser (P0) — so a reversed-recipient assertion is
/// needed to catch that leg; a summed total cannot.
#[test]
fn exchanging_control_fires_a_lose_control_trigger_exactly_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.with_library_top(P0, &["Draw A", "Draw B", "Draw C"]);
    scenario.with_library_top(P1, &["Filler D", "Filler E", "Filler F"]);
    let kharn = scenario
        .add_creature_from_oracle(P0, "Khârn the Betrayer", 4, 4, KHARN_TEXT)
        .id();
    let bear = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let switcheroo = scenario
        .add_spell_to_hand_from_oracle(P0, "Switcheroo", false, SWITCHEROO_TEXT)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.active_player = P0;
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
    }
    let outcome = runner
        .cast(switcheroo)
        .target_objects(&[kharn, bear])
        .resolve();

    // REACH GUARD: the exchange really happened (CR 701.12b needed two
    // different controllers, which this board supplies).
    assert_eq!(
        outcome.state().objects.get(&kharn).unwrap().controller,
        P1,
        "REACH GUARD: Khârn must have changed hands"
    );
    assert_eq!(
        outcome.state().objects.get(&bear).unwrap().controller,
        P0,
        "REACH GUARD: and the bear must have come the other way"
    );

    // THE DISCRIMINATOR: exactly one lose-control trigger resolved, controlled
    // by the player who LOST control of Khârn (P0, CR 603.10d + CR 603.3a) —
    // not the gainer (P1), and not a double-fire (which would read 4/0 or 2/2
    // depending on attribution).
    outcome.assert_hand_drawn(P0, 2);
    outcome.assert_hand_drawn(P1, 0);
}

/// V8 NEGATIVE — the Portent trap. A `ChangesController` trigger is scoped to
/// its OWN tracked object by `valid_card_matches`, so a bystander carrying the
/// same trigger must NOT fire when two unrelated objects exchange control.
///
/// This row also covers the STACK HALF: Perplexing Chimera's exchange publishes
/// a `ControllerChanged` whose `object_id` is a SPELL (CR 109.4 — objects on the
/// stack have a controller). That event is a legitimate verdict signal and must
/// stay inert to triggers; no printed `ChangesController` trigger has
/// `valid_card: None`, so none can match it.
#[test]
fn an_unrelated_lose_control_trigger_does_not_fire_on_someone_elses_exchange() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.with_library_top(P0, &["Draw A", "Draw B", "Draw C"]);
    scenario.with_library_top(P1, &["Filler D", "Filler E", "Filler F"]);
    let chimera = scenario
        .add_creature_from_oracle(P0, "Perplexing Chimera", 3, 3, PERPLEXING_CHIMERA_TEXT)
        .id();
    // The BYSTANDER: it carries the ChangesController trigger, it is on the
    // battlefield throughout, and its controller never changes.
    let bystander = scenario
        .add_creature_from_oracle(P0, "Khârn the Betrayer", 4, 4, KHARN_TEXT)
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

    // REACH GUARD: the exchange really happened — the Chimera swapped and the
    // spell resolved for its new controller. Without this the row could pass
    // because nothing exchanged at all.
    assert_eq!(
        outcome.state().objects.get(&chimera).unwrap().controller,
        P1,
        "REACH GUARD: the Chimera must have swapped to P1"
    );
    assert_eq!(
        outcome
            .state()
            .objects
            .get(&grizzly_bears)
            .unwrap()
            .controller,
        P0,
        "REACH GUARD: CR 400.7a — the exchanged spell's permanent enters under P0's control"
    );
    assert_eq!(
        outcome.state().objects.get(&bystander).unwrap().controller,
        P0,
        "REACH GUARD: the bystander's controller never changed"
    );

    assert_eq!(
        outcome.hand_drawn(P0),
        0,
        "the bystander's \"When you lose control of ~\" must not fire for an exchange it \
         was not part of, on the battlefield half OR the stack half"
    );
    assert_eq!(outcome.hand_drawn(P1), 0);
}

// ---------------------------------------------------------------------------
// V6c — Arteeoh's reflexive "When you do", end to end
// ---------------------------------------------------------------------------

/// Verbatim from `client/public/card-data.json`.
const ARTEEOH_TEXT: &str = "Flying, deathtouch\nWhenever Arteeoh deals combat damage to a \
    player, you may exchange control of two other target artifacts. When you do, create a token \
    that's a copy of target artifact you don't control, except it's a 1/1 green Squirrel \
    creature token in addition to its other colors and types.";

/// Stage Arteeoh's trigger with its two declared exchange slots pre-wired, then
/// accept the "you may" through the real action pipeline.
///
/// The declared targets are pre-wired rather than submitted through
/// `GameAction::SelectTargets` because a two-declared-slot `ExchangeControl`
/// CANNOT be targeted that way today: `collect_target_slots` and the per-slot
/// spec builder both surface two slots, but `assign_targets_recursive` has no
/// `Effect::ExchangeControl` arm, so `assign_targets_in_chain` consumes one of
/// the two and rejects the submission with `InvalidAction("Unused selected
/// targets")` — measured for the same-controller AND the cross-controller pick,
/// so it is not fixture-specific. That is a pre-existing targeting-assignment
/// defect in `ability_utils.rs`, filed separately and NOT this change's to fix;
/// the CAST path is unaffected, which is why the Switcheroo / Sudden
/// Substitution rows above are green.
///
/// **DO NOT "upgrade" this row to `advance_to_combat` / `declare_attackers` /
/// `combat_damage` staging.** The combat half works (the trigger fires and
/// surfaces both slots), but the `SelectTargets` that must follow it cannot be
/// satisfied, so the row would be permanently red.
///
/// Everything downstream of the accept — the reflexive trigger, its target
/// prompt, and the token — flows through the production `WaitingFor` /
/// `GameAction` path, which is what this row measures.
fn accept_arteeoh_exchange(
    runner: &mut engine::game::scenario::GameRunner,
    arteeoh: engine::types::identifiers::ObjectId,
    slot_a: engine::types::identifiers::ObjectId,
    slot_b: engine::types::identifiers::ObjectId,
) {
    use engine::game::ability_utils::build_resolved_from_def_with_targets;
    use engine::game::effects::resolve_ability_chain;
    use engine::parser::oracle::parse_oracle_text;

    let parsed = parse_oracle_text(ARTEEOH_TEXT, "Arteeoh, Dread Scavenger", &[], &[], &[]);
    let def = *parsed
        .triggers
        .first()
        .expect("Arteeoh has a combat-damage trigger")
        .execute
        .clone()
        .expect("that trigger has an execute");

    let resolved = build_resolved_from_def_with_targets(
        &def,
        arteeoh,
        P0,
        vec![TargetRef::Object(slot_a), TargetRef::Object(slot_b)],
    );
    let mut events = Vec::new();
    resolve_ability_chain(runner.state_mut(), &resolved, &mut events, 0)
        .expect("the chain must resolve up to its optional prompt");

    // REACH GUARD: the chain really parked on Arteeoh's own "you may" offer.
    match runner.state().waiting_for {
        WaitingFor::OptionalEffectChoice { source_id, .. } => assert_eq!(
            source_id, arteeoh,
            "REACH GUARD: the offer must be Arteeoh's own"
        ),
        ref other => panic!("expected Arteeoh's OptionalEffectChoice, got {other:?}"),
    }

    assert!(
        runner
            .act(GameAction::DecideOptionalEffect { accept: true })
            .is_ok(),
        "accepting the exchange must be accepted by the reducer"
    );
}

fn squirrel_tokens(runner: &engine::game::scenario::GameRunner) -> Vec<String> {
    runner
        .state()
        .battlefield
        .iter()
        .filter(|id| runner.state().objects[id].is_token)
        .map(|id| runner.state().objects[id].name.clone())
        .collect()
}

/// V6c — CR 701.12b + CR 603.12: Arteeoh's reflexive "When you do, create a
/// token …" must NOT fire when the accepted exchange exchanged nothing.
///
/// Both declared artifacts are P0's, so CR 701.12b makes the exchange a no-op
/// even though the controller accepted the offer. Suppression happens at
/// `resolve_ability_chain`'s `if !condition_met` early exit, which is strictly
/// BEFORE `try_materialize_reflexive_trigger` — a suppressed `WhenYouDo` sub
/// can never materialise a reflexive trigger at all.
///
/// This consumer's path is DISJOINT from the `IfYouDo` one: `evaluate_condition`'s
/// `WhenYouDo` arm reads `ability.optional && !performed`, and the accept has
/// already lowered `optional`, so that arm returns true regardless.
/// `when_you_do_mandatory_parent_did_nothing` is the only thing that can
/// suppress it, and all four of its conjuncts must hold — two of which this
/// change supplies (the resolver-verdict block lowers the latched flag; the new
/// `mandatory_parent_effect_performed` arm answers no).
///
/// REVERT-FAILING (both measured PRESENT pre-fix): the reflexive
/// `TriggerTargetSelection` carrying a `CopyTokenOf` slot, and the token itself.
#[test]
fn arteeoh_reflexive_token_does_not_fire_when_the_exchange_did_nothing() {
    use engine::types::ability::EffectKind;

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let arteeoh = scenario
        .add_creature_from_oracle(P0, "Arteeoh, Dread Scavenger", 3, 3, ARTEEOH_TEXT)
        .id();
    // BOTH exchange subjects under P0. Legal: both slots are
    // `Typed(Artifact, Another)` with no controller restriction.
    let a1 = scenario.add_artifact_from_oracle(P0, "Bauble A", "").id();
    let a2 = scenario.add_artifact_from_oracle(P0, "Bauble B", "").id();
    // The reflexive body's own target — "target artifact you don't control".
    let foreign = scenario
        .add_artifact_from_oracle(P1, "Foreign Relic", "")
        .id();

    let mut runner = scenario.build();
    accept_arteeoh_exchange(&mut runner, arteeoh, a1, a2);

    // SECOND REACH GUARD: the exchange genuinely did nothing (CR 701.12b). A
    // row where the exchange SUCCEEDED would prove nothing about the gate.
    assert_eq!(runner.state().objects[&a1].controller, P0);
    assert_eq!(runner.state().objects[&a2].controller, P0);

    // THE DISCRIMINATOR (1): no reflexive trigger was materialised.
    if let WaitingFor::TriggerTargetSelection { target_slots, .. } = &runner.state().waiting_for {
        assert!(
            !target_slots
                .iter()
                .any(|slot| slot.effect_kind == EffectKind::CopyTokenOf),
            "the reflexive \"When you do\" must not raise its target prompt for an exchange \
             that exchanged nothing (slots were {target_slots:?})"
        );
    }

    // THE DISCRIMINATOR (2): and no token exists, at the prompt boundary or
    // after draining whatever else is pending.
    runner.advance_until_stack_empty();
    assert!(
        squirrel_tokens(&runner).is_empty(),
        "no token may be created for an exchange that exchanged nothing (tokens were {:?})",
        squirrel_tokens(&runner)
    );
    // The reflexive body's would-be target is untouched and still P1's.
    assert_eq!(runner.state().objects[&foreign].controller, P1);
}

/// V6c's PAIRED POSITIVE REACH GUARD (mandatory — it is what makes the two
/// negatives above non-vacuous). The same staging with a CROSS-controller pair,
/// so CR 701.12b does not no-op: the reflexive trigger must still be raised and
/// the token must still be created.
#[test]
fn arteeoh_reflexive_token_still_fires_when_the_exchange_happens() {
    use engine::types::ability::EffectKind;

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let arteeoh = scenario
        .add_creature_from_oracle(P0, "Arteeoh, Dread Scavenger", 3, 3, ARTEEOH_TEXT)
        .id();
    let a1 = scenario.add_artifact_from_oracle(P0, "Bauble A", "").id();
    let foreign = scenario
        .add_artifact_from_oracle(P1, "Foreign Relic", "")
        .id();

    let mut runner = scenario.build();
    accept_arteeoh_exchange(&mut runner, arteeoh, a1, foreign);

    // CR 701.12b: different controllers, so the exchange really happens.
    assert_eq!(
        runner.state().objects[&a1].controller,
        P1,
        "REACH GUARD: a real exchange must move control"
    );

    // The reflexive trigger IS materialised, with its own `CopyTokenOf` slot.
    let slot_reached = match &runner.state().waiting_for {
        WaitingFor::TriggerTargetSelection { target_slots, .. } => target_slots
            .iter()
            .any(|slot| slot.effect_kind == EffectKind::CopyTokenOf),
        _ => false,
    };
    assert!(
        slot_reached,
        "REACH GUARD: a completed exchange must raise the reflexive \"When you do\" target \
         prompt (state was {:?})",
        runner.state().waiting_for
    );

    // "target artifact you don't control" is read AFTER the exchange, so the
    // artifact P0 no longer controls is `a1` — the one it just handed over.
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(a1)],
        })
        .expect("the reflexive body's slot accepts the artifact P0 no longer controls");
    runner.advance_until_stack_empty();

    assert_eq!(
        squirrel_tokens(&runner),
        vec!["Bauble A".to_string()],
        "a completed exchange must still create the copy token"
    );
    // The artifact P0 gained stays P0's — this row is not measuring a revert.
    assert_eq!(runner.state().objects[&foreign].controller, P0);
}
