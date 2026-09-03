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

    assert!(
        validate_targets_in_chain(&state, &ability)
            .targets
            .is_empty(),
        "CR 608.2b: a target that no longer matches its filter is illegal, so this ability's \
         only target is illegal and it does not resolve"
    );
}
