//! Instruction-local return results frozen into a next-upkeep delayed trigger.

use std::sync::Arc;

use engine::game::scenario::{GameScenario, P0, P1};
use engine::parser::oracle::parse_oracle_text;
use engine::types::ability::{
    AbilityCondition, AbilityDefinition, AbilityKind, ChoiceType, DelayedTriggerCondition, Effect,
    PtValue, QuantityExpr, ReplacementDefinition, SubAbilityLink, TargetFilter, TargetRef,
    TargetSelectionMode,
};
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::PersistedGameState;
use engine::types::game_state::WaitingFor;
use engine::types::identifiers::ObjectId;
use engine::types::keywords::Keyword;
use engine::types::mana::{ManaColor, ManaCost, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::replacements::ReplacementEvent;
use engine::types::resolution::ResolutionStateWire;
use engine::types::zones::{EtbTapState, Zone};

const EAGLES: &str = "Kicker {2}{W}{W} (You may pay an additional {2}{W}{W} as you cast this spell.)\nChoose target creature you own. If this spell was kicked, instead choose any number of target creatures you own. Return each chosen creature to your hand. At the beginning of the next upkeep, create a 4/4 white Bird Soldier creature token with flying for each creature returned to your hand this way.";
const TWINCAST: &str =
    "Copy target instant or sorcery spell. You may choose new targets for the copy.";
const INTERLEAVED_RETURNS: &str = "Choose target creature you own. Return each chosen creature to your hand. Choose target artifact you own. Return each chosen artifact to your hand. At the beginning of the next upkeep, create a 4/4 white Bird Soldier creature token with flying for each creature returned to your hand this way.";
const DISTINCT_RETURN_READERS: &str = "Choose target creature you own. Return each chosen creature to your hand. Return target land you control to your hand. Choose target artifact you own. Return each chosen artifact to your hand. At the beginning of the next upkeep, create a 1/1 white Soldier creature token for each creature returned to your hand this way. At the beginning of the next upkeep, create a 1/1 white Soldier creature token for each artifact returned to your hand this way.";
const CONDITIONAL_RETURN_WITNESS: &str = "Choose target creature you own. Return each chosen creature to your hand. At the beginning of the next upkeep, create a 4/4 white Bird Soldier creature token with flying for each creature returned to your hand this way.";

fn bird_count(runner: &engine::game::scenario::GameRunner) -> usize {
    runner
        .state()
        .battlefield
        .iter()
        .filter_map(|id| runner.state().objects.get(id))
        .filter(|object| object.is_token && object.name.contains("Bird Soldier"))
        .count()
}

#[test]
fn eagles_preserves_printed_delayed_token_and_binds_exact_return() {
    let parsed = parse_oracle_text(
        EAGLES,
        "The Eagles Are Coming!",
        &[],
        &["Instant".into()],
        &[],
    );
    let mut node = &parsed.abilities[0];
    let mut producer = None;
    let mut delayed = None;
    loop {
        if node.declares_return_result.is_some() {
            producer = Some(node);
        }
        if node.reads_return_result.is_some() {
            delayed = Some(node);
        }
        match node.sub_ability.as_deref() {
            Some(next) => node = next,
            None => break,
        }
    }
    let producer = producer.expect("selected return declares its result");
    let delayed = delayed.expect("delayed clause reads the result");
    assert_eq!(
        delayed.reads_return_result.as_ref().map(|(id, _)| *id),
        producer.declares_return_result
    );
    let Effect::CreateDelayedTrigger {
        condition, effect, ..
    } = &*delayed.effect
    else {
        panic!("delayed wrapper must survive parsing: {delayed:#?}");
    };
    assert!(matches!(
        condition,
        DelayedTriggerCondition::AtNextPhase {
            phase: Phase::Upkeep
        }
    ));
    let Effect::Token {
        name,
        power,
        toughness,
        types,
        colors,
        keywords,
        count,
        ..
    } = &*effect.effect
    else {
        panic!("printed token body must survive parsing: {effect:#?}");
    };
    assert!(name.contains("Bird Soldier"));
    assert_eq!(power, &PtValue::Fixed(4));
    assert_eq!(toughness, &PtValue::Fixed(4));
    assert!(types.iter().any(|kind| kind == "Bird"));
    assert!(types.iter().any(|kind| kind == "Soldier"));
    assert_eq!(colors, &[ManaColor::White]);
    assert!(keywords.contains(&Keyword::Flying));
    assert!(!matches!(count, QuantityExpr::Fixed { value: 1 }));
}

#[test]
fn delayed_return_reader_rejects_wrong_noun_destination_and_missing_producer() {
    for oracle in [
        EAGLES.replace("for each creature returned", "for each artifact returned"),
        EAGLES.replace("returned to your hand this way", "returned to your graveyard this way"),
        "At the beginning of the next upkeep, create a 4/4 white Bird Soldier creature token with flying for each creature returned to your hand this way.".to_string(),
    ] {
        let parsed = parse_oracle_text(&oracle, "Delayed Return Witness", &[], &["Instant".into()], &[]);
        let mut cursor = &parsed.abilities[0];
        while let Some(next) = cursor.sub_ability.as_deref() {
            cursor = next;
        }
        assert!(matches!(&*cursor.effect, Effect::Unimplemented { .. }), "{oracle}: {cursor:#?}");
        assert!(cursor.reads_return_result.is_none());
    }
}

#[test]
fn leading_conditional_chosen_return_remains_strict_unsupported() {
    let oracle = CONDITIONAL_RETURN_WITNESS.replacen(
        "Return each chosen creature to your hand.",
        "If you control a Bird, return each chosen creature to your hand.",
        1,
    );
    let parsed = parse_oracle_text(
        &oracle,
        "Leading Conditional Return Witness",
        &[],
        &["Instant".into()],
        &[],
    );
    let mut node = &parsed.abilities[0];
    let mut strict = false;
    loop {
        strict |= matches!(&*node.effect, Effect::Unimplemented { .. });
        match node.sub_ability.as_deref() {
            Some(next) => node = next,
            None => break,
        }
    }
    assert!(strict, "conditional return must not silently claim support");
}

#[test]
fn eagles_one_return_creates_one_bird_at_the_next_upkeep() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let selected = scenario.add_creature(P0, "Selected Bear", 2, 2).id();
    let other = scenario.add_creature(P0, "Other Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    let outcome = runner.cast(spell).target_object(selected).resolve();
    outcome.assert_zone(&[selected], Zone::Hand);
    outcome.assert_zone(&[other], Zone::Battlefield);
    assert_eq!(outcome.state().delayed_triggers.len(), 1);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
    assert_eq!(bird_count(&runner), 0);
    let mut external_move_events = Vec::new();
    engine::game::zones::move_to_zone(
        runner.state_mut(),
        selected,
        Zone::Graveyard,
        &mut external_move_events,
    );
    assert_eq!(runner.state().objects[&selected].zone, Zone::Graveyard);
    runner.advance_to_phase(Phase::Upkeep);
    assert_eq!(runner.state().phase, Phase::Upkeep);
    runner.advance_until_stack_empty();
    assert_eq!(bird_count(&runner), 1);
    assert!(runner.state().delayed_triggers.is_empty());
}

#[test]
fn all_invalid_eagles_targets_fizzle_without_installing_a_delayed_reader() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let first = scenario.add_creature(P0, "First Bear", 2, 2).id();
    let second = scenario.add_creature(P0, "Second Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let mut committed = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[first, second])
        .commit();
    let mut external_move_events = Vec::new();
    engine::game::zones::move_to_zone(
        committed.state_mut(),
        first,
        Zone::Graveyard,
        &mut external_move_events,
    );
    engine::game::zones::move_to_zone(
        committed.state_mut(),
        second,
        Zone::Graveyard,
        &mut external_move_events,
    );
    let outcome = committed.resolve();
    assert!(outcome.events().iter().any(|event| matches!(
        event, GameEvent::StackResolved { object_id } if *object_id == spell
    )));
    outcome.assert_zone(&[first, second], Zone::Graveyard);
    assert!(outcome.state().delayed_triggers.is_empty());
    assert!(outcome.state().return_result_frames.is_empty());
}

#[test]
fn kicked_zero_and_two_returns_freeze_independent_token_counts() {
    for chosen_count in [0, 2] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::End);
        let first = scenario.add_creature(P0, "First Bear", 2, 2).id();
        let second = scenario.add_creature(P0, "Second Bear", 2, 2).id();
        let unchosen = scenario.add_creature(P0, "Unchosen Bear", 2, 2).id();
        let spell = scenario
            .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
            .with_mana_cost(ManaCost::zero())
            .id();
        scenario.with_mana_pool(
            P0,
            (0..4)
                .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
                .collect(),
        );
        let mut runner = scenario.build();
        let cast = runner.cast(spell).accept_optional();
        let outcome = if chosen_count == 0 {
            cast.resolve()
        } else {
            cast.target_objects(&[first, second]).resolve()
        };
        if chosen_count == 0 {
            outcome.assert_zone(&[first, second, unchosen], Zone::Battlefield);
        } else {
            outcome.assert_zone(&[first, second], Zone::Hand);
            outcome.assert_zone(&[unchosen], Zone::Battlefield);
        }
        assert_eq!(outcome.state().delayed_triggers.len(), 1);
        assert!(matches!(
            &outcome.state().delayed_triggers[0].ability.effect,
            Effect::Token { count: QuantityExpr::Fixed { value }, .. } if *value == chosen_count
        ));
        runner.advance_to_phase(Phase::Upkeep);
        assert_eq!(runner.state().phase, Phase::Upkeep);
        runner.advance_until_stack_empty();
        assert_eq!(bird_count(&runner), chosen_count as usize);
        assert!(runner.state().delayed_triggers.is_empty());
    }
}

#[test]
fn skipped_return_publishes_empty_before_independent_delayed_reader() {
    // The parser does not claim a leading conditional-return grammar here.
    // Add a typed condition to its successfully parsed return instruction so
    // the cast pipeline exercises a skipped producer followed by an independent
    // delayed reader. The alternative case guards one-result publication when
    // an else branch executes the same producer instead.
    for (cast_zone, alternative_returns, expected_count) in [
        (Zone::Hand, false, 1),
        (Zone::Exile, false, 0),
        (Zone::Exile, true, 1),
    ] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::End);
        scenario.with_library_top(P0, &["P0 Draw A", "P0 Draw B"]);
        scenario.with_library_top(P1, &["P1 Draw A", "P1 Draw B"]);
        let selected = scenario.add_creature(P0, "Selected Bear", 2, 2).id();
        let spell = scenario
            .add_spell_to_hand_from_oracle(
                P0,
                "Conditional Return Witness",
                true,
                CONDITIONAL_RETURN_WITNESS,
            )
            .with_mana_cost(ManaCost::zero())
            .id();
        let mut runner = scenario.build();
        let object = runner.state_mut().objects.get_mut(&spell).unwrap();
        let definitions = Arc::make_mut(&mut object.abilities);
        let mut producer = definitions
            .iter_mut()
            .find(|definition| matches!(definition.kind, AbilityKind::Spell))
            .expect("printed spell ability");
        while producer.declares_return_result.is_none() {
            producer = producer
                .sub_ability
                .as_deref_mut()
                .expect("parsed chain must contain a return producer");
        }
        assert!(matches!(&*producer.effect, Effect::BounceAll { .. }));
        assert!(
            producer.else_ability.is_none(),
            "return producer already has an alternative: {:?}",
            producer.else_ability
        );
        assert_eq!(
            producer.sub_ability.as_ref().map(|sub| sub.sub_link),
            Some(SubAbilityLink::SequentialSibling),
            "the delayed reader must remain independent of the return gate"
        );
        assert!(
            producer.sub_ability.as_ref().unwrap().condition.is_none(),
            "delayed reader unexpectedly has a gate: {:?}",
            producer.sub_ability.as_ref().unwrap().condition
        );
        if alternative_returns {
            producer.else_ability = Some(Box::new(producer.clone()));
        }
        producer.condition = Some(AbilityCondition::WasCast {
            zone: Some(cast_zone),
        });
        object.base_abilities = object.abilities.clone();
        let outcome = runner.cast(spell).target_object(selected).resolve();
        outcome.assert_zone(
            &[selected],
            if expected_count == 0 {
                Zone::Battlefield
            } else {
                Zone::Hand
            },
        );
        assert_eq!(
            outcome.state().delayed_triggers.len(),
            1,
            "cast zone {cast_zone:?}, alternative {alternative_returns}"
        );
        assert!(matches!(
            &outcome.state().delayed_triggers[0].ability.effect,
            Effect::Token {
                count: QuantityExpr::Fixed { value },
                ..
            } if *value == expected_count
        ));
        runner.advance_to_phase(Phase::Upkeep);
        runner.advance_until_stack_empty();
        assert_eq!(bird_count(&runner), expected_count as usize);
        assert!(runner.state().delayed_triggers.is_empty());
        runner.advance_to_phase(Phase::Draw);
        runner.advance_to_phase(Phase::Upkeep);
        runner.advance_until_stack_empty();
        assert_eq!(bird_count(&runner), expected_count as usize);
    }
}

#[test]
fn declined_optional_return_publishes_empty_before_independent_delayed_reader() {
    // The parsed chain supplies the result binding and independent reader;
    // marking only its return producer optional exercises the runtime choice
    // without claiming support for an additional Oracle grammar.
    for (accept_return, alternative_returns, expected_count) in
        [(false, false, 0), (true, false, 1), (false, true, 1)]
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::End);
        scenario.with_library_top(P0, &["P0 Draw A", "P0 Draw B"]);
        scenario.with_library_top(P1, &["P1 Draw A", "P1 Draw B"]);
        let selected = scenario.add_creature(P0, "Selected Bear", 2, 2).id();
        let spell = scenario
            .add_spell_to_hand_from_oracle(
                P0,
                "Optional Return Witness",
                true,
                CONDITIONAL_RETURN_WITNESS,
            )
            .with_mana_cost(ManaCost::zero())
            .id();
        let mut runner = scenario.build();
        let object = runner.state_mut().objects.get_mut(&spell).unwrap();
        let definitions = Arc::make_mut(&mut object.abilities);
        let mut producer = definitions
            .iter_mut()
            .find(|definition| matches!(definition.kind, AbilityKind::Spell))
            .expect("printed spell ability");
        while producer.declares_return_result.is_none() {
            producer = producer
                .sub_ability
                .as_deref_mut()
                .expect("parsed chain must contain a return producer");
        }
        assert!(matches!(&*producer.effect, Effect::BounceAll { .. }));
        assert_eq!(
            producer.sub_ability.as_ref().map(|sub| sub.sub_link),
            Some(SubAbilityLink::SequentialSibling),
            "the delayed reader must remain independent of the optional return"
        );
        if alternative_returns {
            let mut alternative = producer.clone();
            alternative.optional = false;
            alternative.else_ability = None;
            producer.else_ability = Some(Box::new(alternative));
        }
        producer.optional = true;
        object.base_abilities = object.abilities.clone();

        let cast = runner.cast(spell).target_object(selected);
        let outcome = if accept_return {
            cast.accept_optional().resolve()
        } else {
            cast.decline_optional().resolve()
        };
        outcome.assert_zone(
            &[selected],
            if expected_count == 0 {
                Zone::Battlefield
            } else {
                Zone::Hand
            },
        );
        assert_eq!(
            outcome.state().delayed_triggers.len(),
            1,
            "accept return {accept_return}, alternative {alternative_returns}"
        );
        assert!(matches!(
            &outcome.state().delayed_triggers[0].ability.effect,
            Effect::Token {
                count: QuantityExpr::Fixed { value },
                ..
            } if *value == expected_count
        ));
        runner.advance_to_phase(Phase::Upkeep);
        runner.advance_until_stack_empty();
        assert_eq!(bird_count(&runner), expected_count as usize);
        assert!(runner.state().delayed_triggers.is_empty());
    }
}

#[test]
fn optional_return_prompt_restores_exact_result_occurrence() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let selected = scenario.add_creature(P0, "Selected Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(
            P0,
            "Optional Return Save Witness",
            true,
            CONDITIONAL_RETURN_WITNESS,
        )
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    let object = runner.state_mut().objects.get_mut(&spell).unwrap();
    let definitions = Arc::make_mut(&mut object.abilities);
    let mut producer = definitions
        .iter_mut()
        .find(|definition| matches!(definition.kind, AbilityKind::Spell))
        .expect("printed spell ability");
    while producer.declares_return_result.is_none() {
        producer = producer
            .sub_ability
            .as_deref_mut()
            .expect("parsed chain must contain a return producer");
    }
    producer.optional = true;
    object.base_abilities = object.abilities.clone();

    runner.cast(spell).target_object(selected).commit();
    runner.resolve_top();
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::OptionalEffectChoice { .. }
    ));
    let saved = serde_json::to_value(ResolutionStateWire::from_game_state(runner.state().clone()))
        .expect("parked optional return serializes");
    let occurrence = saved["resolution_frames"]["frames"]
        .as_array()
        .expect("typed frames")
        .iter()
        .find(|frame| frame["type"] == "OptionalEffect")
        .and_then(|frame| frame["data"]["return_result_occurrence"].as_u64())
        .expect("optional frame captures its named-result occurrence");
    assert!(saved["return_result_frames"][occurrence.to_string()].is_object());

    for (index, (mut corrupt, expected)) in [
        (saved.clone(), "optional effect has no occurrence stamp"),
        (saved.clone(), "optional effect names missing occurrence"),
    ]
    .into_iter()
    .enumerate()
    {
        let frame = corrupt["resolution_frames"]["frames"]
            .as_array_mut()
            .expect("typed frames")
            .iter_mut()
            .find(|frame| frame["type"] == "OptionalEffect")
            .expect("parked optional frame");
        if index == 0 {
            frame["data"]
                .as_object_mut()
                .expect("optional frame data")
                .remove("return_result_occurrence");
        } else {
            frame["data"]["return_result_occurrence"] = serde_json::json!(999_u64);
        }
        let error = serde_json::from_value::<PersistedGameState>(corrupt)
            .expect_err("malformed optional occurrence must fail at restore");
        assert!(error.to_string().contains(expected), "{error}");
    }

    let restored: ResolutionStateWire =
        serde_json::from_value(saved).expect("valid parked optional return restores");
    *runner.state_mut() = restored.into_game_state();
    runner
        .act(GameAction::DecideOptionalEffect { accept: false })
        .expect("declining after reload resumes the original result frame");
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().objects[&selected].zone, Zone::Battlefield);
    assert!(matches!(
        &runner.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 0 },
            ..
        }
    ));
}

#[test]
fn token_creature_counts_from_its_prior_object_record_after_leaving() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let card = scenario.add_creature(P0, "Card Bear", 2, 2).id();
    let token = scenario.add_creature(P0, "Token Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    runner.state_mut().objects.get_mut(&token).unwrap().is_token = true;
    let outcome = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[card, token])
        .resolve();
    assert!(outcome.events().iter().any(|event| matches!(
        event,
        GameEvent::ZoneChanged { object_id, to: Zone::Hand, .. } if *object_id == token
    )));
    outcome.assert_zone(&[card], Zone::Hand);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 2 },
            ..
        }
    ));
}

fn redirect_own_hand_move_to(destination: Zone) -> ReplacementDefinition {
    ReplacementDefinition::new(ReplacementEvent::Moved)
        .destination_zone(Zone::Hand)
        .valid_card(TargetFilter::SelfRef)
        .execute(AbilityDefinition::new(
            AbilityKind::Spell,
            Effect::ChangeZone {
                destination,
                origin: None,
                target: TargetFilter::SelfRef,
                owner_library: false,
                enter_transformed: false,
                enters_under: None,
                enter_tapped: EtbTapState::Unspecified,
                enters_attacking: false,
                up_to: false,
                enter_with_counters: vec![],
                conditional_enter_with_counters: vec![],
                face_down_profile: None,
                enters_modified_if: None,
            },
        ))
}

#[test]
fn replacement_redirect_counts_only_final_hand_returns() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let redirected = scenario
        .add_creature(P0, "Redirected Bear", 2, 2)
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Exile))
        .id();
    let returned = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let opponent_owned = scenario
        .add_creature(P1, "Borrowed Bear", 2, 2)
        .controlled_by(P0)
        .id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let outcome = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[redirected, returned])
        .resolve();
    assert!(
        matches!(
            outcome.final_waiting_for(),
            engine::types::game_state::WaitingFor::Priority { .. }
        ),
        "return paused: {:?}",
        outcome.final_waiting_for()
    );
    outcome.assert_zone(&[redirected], Zone::Exile);
    outcome.assert_zone(&[returned], Zone::Hand);
    outcome.assert_zone(&[opponent_owned], Zone::Battlefield);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
}

#[test]
fn competing_redirect_choice_survives_save_and_counts_settled_batch() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let redirected = scenario
        .add_creature(P0, "Redirected Bear", 2, 2)
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Exile))
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Graveyard))
        .id();
    let returned = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let outcome = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[redirected, returned])
        .resolve();
    assert!(matches!(
        outcome.final_waiting_for(),
        engine::types::game_state::WaitingFor::ReplacementChoice { .. }
    ));
    assert!(outcome.state().delayed_triggers.is_empty());
    assert!(!outcome.state().return_result_frames.is_empty());
    let saved = serde_json::to_value(ResolutionStateWire::from_game_state(runner.state().clone()))
        .expect("paused return batch serializes");
    let restored: ResolutionStateWire =
        serde_json::from_value(saved).expect("paused return batch restores");
    *runner.state_mut() = restored.into_game_state();
    runner
        .act(GameAction::ChooseReplacement { index: 0 })
        .expect("choose first redirect");
    runner.advance_until_stack_empty();
    assert!(matches!(
        runner.state().objects[&redirected].zone,
        Zone::Exile | Zone::Graveyard
    ));
    assert_eq!(runner.state().objects[&returned].zone, Zone::Hand);
    assert_eq!(runner.state().delayed_triggers.len(), 1);
    assert!(matches!(
        &runner.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
    assert!(runner.state().return_result_frames.is_empty());
}

#[test]
fn paused_return_result_restore_rejects_corrupt_authority_before_resume() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let redirected = scenario
        .add_creature(P0, "Redirected Bear", 2, 2)
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Exile))
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Graveyard))
        .id();
    let returned = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let outcome = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[redirected, returned])
        .resolve();
    assert!(matches!(
        outcome.final_waiting_for(),
        WaitingFor::ReplacementChoice { .. }
    ));
    assert!(!outcome.state().return_result_frames.is_empty());

    let saved = serde_json::to_value(ResolutionStateWire::from_game_state(runner.state().clone()))
        .expect("parked production batch serializes");
    let batch_completion = |wire: &serde_json::Value| {
        wire["resolution_frames"]["frames"]
            .as_array()
            .expect("typed resolution frames")
            .iter()
            .find_map(|frame| frame["data"]["completion"].get("RecordInstructionZoneResult"))
            .cloned()
            .expect("production return batch carries a named completion")
    };
    let completion = batch_completion(&saved);
    let occurrence = completion["occurrence_id"].as_u64().expect("occurrence id");
    let result = completion["result_id"].as_u64().expect("result id");
    assert!(completion["settled_records"].is_null());
    assert!(saved["return_result_frames"]
        .get(occurrence.to_string())
        .is_some());
    let continuation = saved["resolution_frames"]["frames"]
        .as_array()
        .expect("typed frames")
        .iter()
        .find(|frame| {
            frame["type"] == "AbilityContinuation"
                && frame["data"]["pending"]["return_result_occurrence"] == occurrence
        })
        .expect("the return reader is parked behind the replacement choice");
    assert!(
        continuation["data"]["pending"]["chain"]
            .to_string()
            .contains("reads_return_result"),
        "the parked continuation must carry a named reader"
    );

    let restored: ResolutionStateWire =
        serde_json::from_value(saved.clone()).expect("unsettled None remains valid");
    let mut saved_v2 = saved.clone();
    saved_v2["resolution_state_version"] = serde_json::json!(2);
    let _: ResolutionStateWire =
        serde_json::from_value(saved_v2).expect("v2 typed parked batch restores");
    let raw: PersistedGameState =
        serde_json::from_value(saved.clone()).expect("raw parked save restores");
    let trusted: PersistedGameState = serde_json::from_value(serde_json::json!({
        "state": saved.clone()
    }))
    .expect("trusted parked save restores");
    assert!(matches!(raw, PersistedGameState::Raw(_)));
    assert!(matches!(trusted, PersistedGameState::Trusted(_)));
    *runner.state_mut() = restored.into_game_state();
    runner
        .act(GameAction::ChooseReplacement { index: 0 })
        .expect("valid parked batch resumes");
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().delayed_triggers.len(), 1);
    assert!(matches!(
        &runner.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));

    let mut corruptions = Vec::new();
    let mut missing = saved.clone();
    missing["return_result_frames"]
        .as_object_mut()
        .expect("occurrence arena")
        .remove(&occurrence.to_string());
    corruptions.push((missing, "missing occurrence"));

    let mut zero_allocator = saved.clone();
    zero_allocator["next_return_result_occurrence_id"] = serde_json::json!(0);
    corruptions.push((zero_allocator, "allocator is zero"));

    let mut collided_allocator = saved.clone();
    collided_allocator["next_return_result_occurrence_id"] = serde_json::json!(occurrence);
    corruptions.push((collided_allocator, "outside the allocator"));

    let mut missing_active = saved.clone();
    missing_active["active_return_result_occurrence"] = serde_json::json!(999_u64);
    corruptions.push((missing_active, "active return-result occurrence"));

    let mut wrong_batch = saved.clone();
    for frame in wrong_batch["resolution_frames"]["frames"]
        .as_array_mut()
        .expect("typed frames")
    {
        if let Some(completion) = frame["data"]["completion"].get_mut("RecordInstructionZoneResult")
        {
            completion["occurrence_id"] = serde_json::json!(999_u64);
        }
    }
    corruptions.push((wrong_batch, "batch names missing occurrence"));

    let mut premature = saved.clone();
    for frame in premature["resolution_frames"]["frames"]
        .as_array_mut()
        .expect("typed frames")
    {
        if let Some(completion) = frame["data"]["completion"].get_mut("RecordInstructionZoneResult")
        {
            completion["settled_records"] = serde_json::json!([]);
        }
    }
    corruptions.push((premature, "prematurely settled"));

    let mut duplicate_publication = saved.clone();
    duplicate_publication["return_result_frames"][occurrence.to_string()]
        .as_object_mut()
        .expect("result frame")
        .insert(result.to_string(), serde_json::json!([]));
    corruptions.push((duplicate_publication, "already published"));

    let mut unstamped_reader = saved.clone();
    for frame in unstamped_reader["resolution_frames"]["frames"]
        .as_array_mut()
        .expect("typed frames")
    {
        if frame["type"] == "AbilityContinuation"
            && frame["data"]["pending"]["return_result_occurrence"] == occurrence
        {
            frame["data"]["pending"]
                .as_object_mut()
                .expect("pending continuation")
                .remove("return_result_occurrence");
        }
    }
    corruptions.push((unstamped_reader, "continuation has no occurrence stamp"));

    let mut wrong_continuation = saved.clone();
    for frame in wrong_continuation["resolution_frames"]["frames"]
        .as_array_mut()
        .expect("typed frames")
    {
        if frame["type"] == "AbilityContinuation"
            && frame["data"]["pending"]["return_result_occurrence"] == occurrence
        {
            frame["data"]["pending"]["return_result_occurrence"] = serde_json::json!(999_u64);
        }
    }
    corruptions.push((wrong_continuation, "continuation names missing occurrence"));

    let mut duplicate_batch = saved.clone();
    let frames = duplicate_batch["resolution_frames"]["frames"]
        .as_array_mut()
        .expect("typed frames");
    let publisher = frames
        .iter()
        .find(|frame| {
            frame["data"]["completion"]
                .get("RecordInstructionZoneResult")
                .is_some()
        })
        .expect("parked named publisher")
        .clone();
    frames.insert(0, publisher);
    corruptions.push((duplicate_batch, "duplicate parked batch publishers"));

    for (corrupt, expected) in corruptions {
        let mut v2 = corrupt.clone();
        v2["resolution_state_version"] = serde_json::json!(2);
        for envelope in [corrupt.clone(), serde_json::json!({ "state": corrupt }), v2] {
            let error = serde_json::from_value::<PersistedGameState>(envelope)
                .expect_err("malformed authority must fail at restore");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
        }
    }

    let mut direct = serde_json::to_value(runner.state()).expect("authoritative raw state");
    direct["next_return_result_occurrence_id"] = serde_json::json!(0);
    let error = serde_json::from_value::<engine::types::game_state::GameState>(direct)
        .expect_err("direct authoritative state rejects corrupt allocator");
    assert!(error.to_string().contains("allocator is zero"));
}

#[test]
fn replacement_post_effect_nested_root_preserves_outer_result() {
    let mut replacement = redirect_own_hand_move_to(Zone::Exile);
    replacement
        .execute
        .as_mut()
        .expect("redirect has an execute chain")
        .sub_ability = Some(Box::new(AbilityDefinition::new(
        AbilityKind::Spell,
        Effect::GainLife {
            amount: QuantityExpr::Fixed { value: 1 },
            player: TargetFilter::Controller,
        },
    )));
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let redirected = scenario
        .add_creature(P0, "Redirected Bear", 2, 2)
        .with_replacement_definition(replacement)
        .id();
    let returned = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let life_before = runner.state().players[P0.0 as usize].life;
    let outcome = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[redirected, returned])
        .resolve();
    outcome.assert_zone(&[redirected], Zone::Exile);
    outcome.assert_zone(&[returned], Zone::Hand);
    assert_eq!(outcome.state().players[P0.0 as usize].life, life_before + 1);
    assert_eq!(outcome.state().delayed_triggers.len(), 1);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
    assert!(outcome.state().return_result_frames.is_empty());
}

#[test]
fn nested_post_effect_repause_and_reload_preserve_outer_result() {
    let named_choice = || {
        AbilityDefinition::new(
            AbilityKind::Spell,
            Effect::Choose {
                choice_type: ChoiceType::Labeled {
                    options: vec!["first".to_string(), "second".to_string()],
                },
                persist: false,
                selection: TargetSelectionMode::Chosen,
            },
        )
    };
    let mut replacement = redirect_own_hand_move_to(Zone::Exile);
    let mut first_choice = named_choice();
    first_choice.sub_ability = Some(Box::new(named_choice()));
    replacement
        .execute
        .as_mut()
        .expect("redirect has an execute chain")
        .sub_ability = Some(Box::new(first_choice));

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let redirected = scenario
        .add_creature(P0, "Redirected Bear", 2, 2)
        .with_replacement_definition(replacement)
        .id();
    let returned = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let initial = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[redirected, returned])
        .resolve();
    assert!(matches!(
        initial.final_waiting_for(),
        WaitingFor::NamedChoice { .. }
    ));
    assert!(initial.state().delayed_triggers.is_empty());
    assert!(!initial.state().return_result_frames.is_empty());
    for (index, choice) in ["first", "second"].into_iter().enumerate() {
        let saved =
            serde_json::to_value(ResolutionStateWire::from_game_state(runner.state().clone()))
                .expect("nested post-effect pause serializes");
        let restored: ResolutionStateWire =
            serde_json::from_value(saved).expect("nested post-effect pause restores");
        *runner.state_mut() = restored.into_game_state();
        runner
            .act(GameAction::ChooseOption {
                choice: choice.to_string(),
            })
            .expect("answer nested post-effect choice");
        if index == 0 {
            assert!(
                matches!(runner.state().waiting_for, WaitingFor::NamedChoice { .. }),
                "after first post-effect choice: waiting={:?}, delayed={:?}, frames={:?}",
                runner.state().waiting_for,
                runner.state().delayed_triggers,
                runner.state().resolution_stack
            );
            assert!(runner.state().delayed_triggers.is_empty());
        }
    }
    runner.advance_until_stack_empty();
    assert_eq!(runner.state().objects[&redirected].zone, Zone::Exile);
    assert_eq!(runner.state().objects[&returned].zone, Zone::Hand);
    assert_eq!(runner.state().delayed_triggers.len(), 1);
    assert!(matches!(
        &runner.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
    assert!(runner.state().return_result_frames.is_empty());
}

#[test]
fn legacy_save_without_return_result_fields_decodes_with_fresh_identity() {
    let mut state_json =
        serde_json::to_value(GameScenario::new().build().state()).expect("fresh game serializes");
    let object = state_json.as_object_mut().expect("game state is an object");
    object.remove("return_result_frames");
    object.remove("active_return_result_occurrence");
    object.remove("next_return_result_occurrence_id");
    let restored: engine::types::game_state::GameState =
        serde_json::from_value(state_json).expect("old game state decodes");
    assert!(restored.return_result_frames.is_empty());
    assert!(restored.active_return_result_occurrence.is_none());
    assert_eq!(restored.next_return_result_occurrence_id, 1);
}

#[test]
fn two_casts_keep_distinct_frozen_results_until_the_same_next_upkeep() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let first = scenario.add_creature(P0, "First Bear", 2, 2).id();
    let second = scenario.add_creature(P0, "Second Bear", 2, 2).id();
    let third = scenario.add_creature(P0, "Third Bear", 2, 2).id();
    let spell_a = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    let spell_b = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    runner.cast(spell_a).target_object(first).resolve();
    runner
        .cast(spell_b)
        .accept_optional()
        .target_objects(&[second, third])
        .resolve();
    let counts: Vec<_> = runner
        .state()
        .delayed_triggers
        .iter()
        .map(|trigger| match &trigger.ability.effect {
            Effect::Token {
                count: QuantityExpr::Fixed { value },
                ..
            } => *value,
            other => panic!("expected a frozen token body, got {other:?}"),
        })
        .collect();
    assert_eq!(counts, [1, 2]);
    assert!(runner.state().return_result_frames.is_empty());
    runner.advance_to_phase(Phase::Upkeep);
    assert_eq!(runner.state().active_player, P1);
    runner.advance_until_stack_empty();
    assert_eq!(bird_count(&runner), 3);
    assert!(runner.state().delayed_triggers.is_empty());
}

#[test]
fn copied_kicked_spell_freezes_its_own_retargeted_return_count() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let first = scenario.add_creature(P0, "First Bear", 2, 2).id();
    let second = scenario.add_creature(P0, "Second Bear", 2, 2).id();
    let replacement = scenario.add_creature(P0, "Replacement Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    let twincast = scenario
        .add_spell_to_hand_from_oracle(P0, "Twincast", true, TWINCAST)
        .with_mana_cost(ManaCost::zero())
        .id();
    scenario.with_mana_pool(
        P0,
        (0..4)
            .map(|_| ManaUnit::new(ManaType::White, ObjectId(0), false, vec![]))
            .collect(),
    );
    let mut runner = scenario.build();
    let mut original = runner
        .cast(spell)
        .accept_optional()
        .target_objects(&[first, second])
        .commit();
    let copied = original
        .cast(twincast)
        .target_object(spell)
        .commit()
        .resolve();
    assert!(matches!(
        copied.final_waiting_for(),
        WaitingFor::CopyRetarget { .. }
    ));
    let copy_id = copied
        .events()
        .iter()
        .find_map(|event| match event {
            GameEvent::SpellCopied { object_id, .. } => Some(*object_id),
            _ => None,
        })
        .expect("Twincast produces a spell copy");
    assert!(!copied.events().iter().any(|event| matches!(
        event,
        GameEvent::SpellCast { object_id, .. } if *object_id == copy_id
    )));
    original
        .act(GameAction::ChooseTarget {
            target: Some(TargetRef::Object(replacement)),
        })
        .expect("retarget copied first member");
    original
        .act(GameAction::KeepAllCopyTargets)
        .expect("keep copied second member");
    let outcome = original.resolve();
    let mut counts: Vec<_> = outcome
        .state()
        .delayed_triggers
        .iter()
        .map(|trigger| match &trigger.ability.effect {
            Effect::Token {
                count: QuantityExpr::Fixed { value },
                ..
            } => *value,
            other => panic!("expected frozen token body, got {other:?}"),
        })
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, [1, 2]);
    assert_eq!(outcome.state().objects[&first].zone, Zone::Hand);
    assert_eq!(outcome.state().objects[&second].zone, Zone::Hand);
    assert_eq!(outcome.state().objects[&replacement].zone, Zone::Hand);
    assert!(outcome.state().return_result_frames.is_empty());
}

#[test]
fn owner_not_controller_governs_target_and_return_result() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let owned_by_caster = scenario
        .add_creature(P0, "Borrowed by Opponent", 2, 2)
        .controlled_by(P1)
        .id();
    let controlled_by_caster = scenario
        .add_creature(P1, "Borrowed by Caster", 2, 2)
        .controlled_by(P0)
        .id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    let outcome = runner.cast(spell).target_object(owned_by_caster).resolve();
    outcome.assert_zone(&[owned_by_caster], Zone::Hand);
    outcome.assert_zone(&[controlled_by_caster], Zone::Battlefield);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
}

#[test]
fn cast_during_upkeep_waits_for_the_following_upkeep_and_fires_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::Upkeep);
    scenario.with_library_top(P0, &["P0 Draw A", "P0 Draw B"]);
    scenario.with_library_top(P1, &["P1 Draw A", "P1 Draw B"]);
    let selected = scenario.add_creature(P0, "Selected Bear", 2, 2).id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "The Eagles Are Coming!", true, EAGLES)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    runner.cast(spell).target_object(selected).resolve();
    assert_eq!(bird_count(&runner), 0);
    assert_eq!(runner.state().delayed_triggers.len(), 1);
    runner.advance_to_phase(Phase::Draw);
    assert_eq!(runner.state().phase, Phase::Draw);
    runner.advance_to_phase(Phase::Upkeep);
    assert_eq!(runner.state().phase, Phase::Upkeep);
    assert_eq!(runner.state().active_player, P1);
    runner.advance_until_stack_empty();
    assert_eq!(bird_count(&runner), 1);
    assert!(runner.state().delayed_triggers.is_empty());
    runner.advance_to_phase(Phase::Draw);
    assert_eq!(runner.state().phase, Phase::Draw);
    runner.advance_to_phase(Phase::Upkeep);
    assert_eq!(runner.state().phase, Phase::Upkeep);
    runner.advance_until_stack_empty();
    assert_eq!(bird_count(&runner), 1);
}

#[test]
fn interleaved_artifact_return_does_not_feed_creature_result_reader() {
    let parsed = parse_oracle_text(
        INTERLEAVED_RETURNS,
        "Interleaved Return Witness",
        &[],
        &["Instant".into()],
        &[],
    );
    let mut cursor = &parsed.abilities[0];
    let mut creature_result = None;
    let mut artifact_result = None;
    let mut reader = None;
    loop {
        if let Effect::BounceAll { .. } = &*cursor.effect {
            if creature_result.is_none() {
                creature_result = cursor.declares_return_result;
            } else {
                artifact_result = cursor.declares_return_result;
            }
        }
        if cursor.reads_return_result.is_some() {
            reader = cursor.reads_return_result.as_ref().map(|(id, _)| *id);
        }
        match cursor.sub_ability.as_deref() {
            Some(next) => cursor = next,
            None => break,
        }
    }
    assert_eq!(
        reader, creature_result,
        "reader must name the creature return"
    );
    assert!(reader.is_some());
    assert!(
        artifact_result.is_none(),
        "unrelated artifact return is not a producer"
    );

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let creature = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let artifact = scenario
        .add_artifact_from_oracle(P0, "Returned Relic", "")
        .id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Interleaved Return Witness", true, INTERLEAVED_RETURNS)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    let outcome = runner
        .cast(spell)
        .target_objects(&[creature, artifact])
        .resolve();
    outcome.assert_zone(&[creature, artifact], Zone::Hand);
    assert!(matches!(
        &outcome.state().delayed_triggers[0].ability.effect,
        Effect::Token {
            count: QuantityExpr::Fixed { value: 1 },
            ..
        }
    ));
}

#[test]
fn distinct_same_verb_results_survive_an_interleaved_return() {
    let parsed = parse_oracle_text(
        DISTINCT_RETURN_READERS,
        "Distinct Return Readers",
        &[],
        &["Instant".into()],
        &[],
    );
    let mut node = &parsed.abilities[0];
    let mut producers = Vec::new();
    let mut readers = Vec::new();
    loop {
        if let Some(id) = node.declares_return_result {
            producers.push(id);
        }
        if let Some((id, _)) = &node.reads_return_result {
            readers.push(*id);
        }
        match node.sub_ability.as_deref() {
            Some(next) => node = next,
            None => break,
        }
    }
    assert_eq!(
        producers.len(),
        2,
        "two return instructions declare results"
    );
    assert_ne!(producers[0], producers[1]);
    assert_eq!(readers, producers);

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::End);
    let creature = scenario.add_creature(P0, "Returned Bear", 2, 2).id();
    let land = scenario.add_basic_land(P0, ManaColor::Green);
    let artifact = scenario
        .add_artifact_from_oracle(P0, "Redirected Relic", "")
        .with_replacement_definition(redirect_own_hand_move_to(Zone::Exile))
        .id();
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Distinct Return Readers", true, DISTINCT_RETURN_READERS)
        .with_mana_cost(ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    let outcome = runner
        .cast(spell)
        .target_objects(&[creature, land, artifact])
        .resolve();
    outcome.assert_zone(&[creature, land], Zone::Hand);
    outcome.assert_zone(&[artifact], Zone::Exile);
    assert_eq!(outcome.state().delayed_triggers.len(), 2);
    let mut counts: Vec<_> = outcome
        .state()
        .delayed_triggers
        .iter()
        .map(|trigger| match &trigger.ability.effect {
            Effect::Token {
                count: QuantityExpr::Fixed { value },
                ..
            } => *value,
            other => panic!("expected frozen token body, got {other:?}"),
        })
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, [0, 1]);
    assert!(outcome.state().return_result_frames.is_empty());
}
