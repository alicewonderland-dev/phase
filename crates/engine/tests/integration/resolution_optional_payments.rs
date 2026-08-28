use engine::game::effects::resolve_ability_chain;
use engine::game::engine::apply;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::zones::move_to_zone;
use engine::types::ability::{
    AbilityCondition, AbilityCost, AbilityDefinition, AbilityKind, CardSelectionMode,
    DiscardSelfScope, Effect, QuantityExpr, ReplacementDefinition, ReplacementMode,
    ResolvedAbility, TargetFilter,
};
use engine::types::actions::{GameAction, ResolutionOptionalPaymentChoice};
use engine::types::game_state::{
    AutoMayChoice, LoopDetectSample, MayTriggerAutoChoiceKey, MayTriggerOrigin, WaitingFor,
};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaCost, ManaCostShard, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::replacements::ReplacementEvent;
use engine::types::zones::Zone;

fn discard() -> AbilityCost {
    AbilityCost::Discard {
        count: QuantityExpr::Fixed { value: 1 },
        filter: None,
        selection: CardSelectionMode::Chosen,
        self_scope: DiscardSelfScope::FromHand,
    }
}

fn optional_payment(source: ObjectId, costs: Vec<AbilityCost>) -> ResolvedAbility {
    let mut root = ResolvedAbility::new(
        Effect::PayCost {
            cost: AbilityCost::OneOf { costs },
            scale: None,
            payer: TargetFilter::Controller,
        },
        vec![],
        source,
        P0,
    );
    root.optional = true;
    let mut tail = ResolvedAbility::new(
        Effect::GainLife {
            amount: QuantityExpr::Fixed { value: 3 },
            player: TargetFilter::Controller,
        },
        vec![],
        source,
        P0,
    );
    tail.condition = Some(AbilityCondition::effect_performed());
    root.sub_ability = Some(Box::new(tail));
    root
}

fn runner_with_hand(card_count: usize) -> (GameRunner, ObjectId, Vec<ObjectId>) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let source = scenario.add_creature(P0, "Payment Source", 1, 1).id();
    let cards = (0..card_count)
        .map(|index| scenario.add_card_to_hand(P0, &format!("Payment Card {index}")))
        .collect();
    (scenario.build(), source, cards)
}

fn optional_graveyard_exile_replacement() -> ReplacementDefinition {
    ReplacementDefinition::new(ReplacementEvent::Moved)
        .destination_zone(Zone::Graveyard)
        .mode(ReplacementMode::Optional { decline: None })
        .execute(AbilityDefinition::new(
            AbilityKind::Spell,
            Effect::ChangeZone {
                origin: None,
                destination: Zone::Exile,
                target: TargetFilter::SelfRef,
                owner_library: false,
                enter_transformed: false,
                enters_under: None,
                enter_tapped: engine::types::zones::EtbTapState::Unspecified,
                enters_attacking: false,
                up_to: false,
                enter_with_counters: vec![],
                conditional_enter_with_counters: vec![],
                enters_modified_if: None,
                face_down_profile: None,
            },
        ))
}

#[test]
fn saved_decline_skips_resolution_optional_payment_prompt_and_payoff() {
    let (mut runner, source, cards) = runner_with_hand(1);
    let origin = MayTriggerOrigin::Printed { trigger_index: 0 };
    runner.state_mut().set_may_trigger_auto_choice(
        MayTriggerAutoChoiceKey {
            player: P0,
            source_id: source,
            origin: origin.clone(),
        },
        AutoMayChoice::Decline,
    );
    let mut ability = optional_payment(source, vec![discard()]);
    ability.set_may_trigger_origin_recursive(origin);

    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();

    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { .. }
    ));
    assert_eq!(runner.state().objects[&cards[0]].zone, Zone::Hand);
    assert_eq!(runner.state().players[P0.0 as usize].life, 20);
}

#[test]
fn saved_accept_still_opens_resolution_optional_payment_branch_prompt() {
    let (mut runner, source, _) = runner_with_hand(1);
    let origin = MayTriggerOrigin::Printed { trigger_index: 0 };
    runner.state_mut().set_may_trigger_auto_choice(
        MayTriggerAutoChoiceKey {
            player: P0,
            source_id: source,
            origin: origin.clone(),
        },
        AutoMayChoice::Accept,
    );
    let mut ability = optional_payment(source, vec![discard()]);
    ability.set_may_trigger_origin_recursive(origin);

    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();

    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ResolutionOptionalPaymentChoice { player: P0, .. }
    ));
    assert_eq!(runner.state().players[P0.0 as usize].life, 20);
}

#[test]
fn parsed_trigger_reaches_optional_payment_through_cast_and_apply() {
    const ORACLE: &str =
        "When this creature enters, you may discard a card or pay {2}. If you do, you gain 3 life.";
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let creature = scenario
        .add_creature_to_hand_from_oracle(P0, "Production Payment", 1, 1, ORACLE)
        .id();
    scenario.add_card_to_hand(P0, "Payment Card");
    let mut runner = scenario.build();
    {
        let mut committed = runner.cast(creature).commit();
        for _ in 0..20 {
            if matches!(
                committed.state().waiting_for,
                WaitingFor::ResolutionOptionalPaymentChoice { .. }
            ) {
                break;
            }
            committed
                .act(GameAction::PassPriority)
                .expect("production cast/trigger pipeline advances");
        }
        assert!(matches!(
            committed.state().waiting_for,
            WaitingFor::ResolutionOptionalPaymentChoice { .. }
        ));
    }
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .expect("parsed trigger payment action applies");
    assert_eq!(runner.state().players[P0.0 as usize].life, 23);
}

#[test]
fn resolution_optional_discard_replacement_still_opens_if_you_do() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let source = scenario.add_creature(P0, "Payment Source", 1, 1).id();
    let card = scenario.add_card_to_hand(P0, "Payment Card");
    scenario
        .add_creature(P1, "Graveyard Warden", 1, 1)
        .with_replacement_definition(optional_graveyard_exile_replacement());
    let mut runner = scenario.build();
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(source, vec![discard()]);
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .unwrap();
    let WaitingFor::ReplacementChoice { candidates, .. } = &runner.state().waiting_for else {
        panic!("replacement-modified cost must pause on ReplacementChoice");
    };
    let accept = candidates
        .iter()
        .position(|candidate| candidate.description == "Accept")
        .unwrap();
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseReplacement { index: accept },
    )
    .unwrap();
    assert_eq!(runner.state().objects[&card].zone, Zone::Exile);
    assert_eq!(runner.state().players[P0.0 as usize].life, life + 3);
}

#[test]
fn resolution_optional_oneof_surfaces_only_live_immediate_branches() {
    let (mut runner, source, _) = runner_with_hand(1);
    let ability = optional_payment(
        source,
        vec![
            discard(),
            AbilityCost::OneOf {
                costs: vec![discard()],
            },
            AbilityCost::Exile {
                count: 1,
                zone: Some(Zone::Hand),
                filter: None,
            },
        ],
    );
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();

    let WaitingFor::ResolutionOptionalPaymentChoice {
        player,
        source_id,
        costs,
    } = &runner.state().waiting_for
    else {
        panic!("expected direct resolution payment choice");
    };
    assert_eq!((*player, *source_id), (P0, source));
    assert_eq!(
        costs.iter().map(|option| option.index).collect::<Vec<_>>(),
        vec![0, 2],
        "filtered branches retain their original server indices"
    );
    let json = serde_json::to_string(runner.state()).expect("choice state serializes");
    let _: engine::types::game_state::GameState =
        serde_json::from_str(&json).expect("choice state round-trips");
    let pay = GameAction::ChooseResolutionOptionalPaymentBranch {
        choice: ResolutionOptionalPaymentChoice::Pay { index: 2 },
    };
    assert_eq!(
        serde_json::to_value(&pay).unwrap(),
        serde_json::json!({
            "type": "ChooseResolutionOptionalPaymentBranch",
            "data": { "choice": { "type": "Pay", "data": { "index": 2 } } }
        })
    );
    assert_eq!(
        serde_json::from_value::<GameAction>(serde_json::to_value(&pay).unwrap()).unwrap(),
        pay
    );

    let before = serde_json::to_string(runner.state()).unwrap();
    assert!(apply(
        runner.state_mut(),
        P1,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), before);
    assert!(apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 1 },
        },
    )
    .is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), before);
    assert!(apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: usize::MAX },
        },
    )
    .is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), before);
}

#[test]
fn resolution_optional_payment_revalidates_stale_affordability() {
    let (mut runner, source, cards) = runner_with_hand(1);
    let ability = optional_payment(source, vec![discard()]);
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    move_to_zone(
        runner.state_mut(),
        cards[0],
        Zone::Graveyard,
        &mut Vec::new(),
    );
    let before = serde_json::to_string(runner.state()).unwrap();

    assert!(apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), before);
}

#[test]
fn selecting_root_branch_does_not_substitute_nested_oneof() {
    let (mut runner, source, cards) = runner_with_hand(2);
    let mut ability = optional_payment(source, vec![discard()]);
    ability.sub_ability = Some(Box::new(optional_payment(
        source,
        vec![
            discard(),
            AbilityCost::Exile {
                count: 1,
                zone: Some(Zone::Hand),
                filter: None,
            },
        ],
    )));
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .unwrap();
    if matches!(
        runner.state().waiting_for,
        WaitingFor::PayCost { .. } | WaitingFor::DiscardChoice { .. }
    ) {
        apply(
            runner.state_mut(),
            P0,
            GameAction::SelectCards {
                cards: vec![cards[0]],
            },
        )
        .unwrap();
    }

    let WaitingFor::ResolutionOptionalPaymentChoice { costs, .. } = &runner.state().waiting_for
    else {
        panic!(
            "nested optional OneOf must remain a distinct prompt, got {:?}",
            runner.state().waiting_for
        );
    };
    assert_eq!(
        costs.iter().map(|option| option.index).collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn resolution_optional_oneof_with_no_payable_branch_declines_without_prompt() {
    let (mut runner, source, _) = runner_with_hand(0);
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(
        source,
        vec![
            discard(),
            AbilityCost::Mana {
                cost: ManaCost::generic(99),
            },
        ],
    );
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    assert!(!matches!(
        runner.state().waiting_for,
        WaitingFor::ResolutionOptionalPaymentChoice { .. }
    ));
    assert_eq!(runner.state().players[P0.0 as usize].life, life);
}

#[test]
fn resolution_optional_oneof_uses_paycost_player_reference() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let source = scenario.add_creature(P0, "Payment Source", 1, 1).id();
    let payer_card = scenario.add_card_to_hand(P1, "Payer Card");
    let mut runner = scenario.build();
    let mut ability = optional_payment(source, vec![discard()]);
    let Effect::PayCost { payer, .. } = &mut ability.effect else {
        unreachable!();
    };
    *payer = TargetFilter::Opponent;
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::ResolutionOptionalPaymentChoice { player: P1, .. }
        ),
        "PayCost player reference must own the prompt, got {:?}",
        runner.state().waiting_for
    );
    let pay = GameAction::ChooseResolutionOptionalPaymentBranch {
        choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
    };
    assert!(apply(runner.state_mut(), P0, pay.clone()).is_err());
    apply(runner.state_mut(), P1, pay).unwrap();
    assert_eq!(runner.state().objects[&payer_card].zone, Zone::Graveyard);
}

#[test]
fn resolution_optional_oneof_decline_clears_loop_ring_and_cannot_replay() {
    let (mut runner, source, cards) = runner_with_hand(1);
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(source, vec![discard()]);
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    let sampled = runner.state().clone();
    runner
        .state_mut()
        .loop_detect_ring
        .push_back(std::sync::Arc::new(LoopDetectSample {
            normalized: sampled.clone(),
            live: sampled,
        }));
    let decline = GameAction::ChooseResolutionOptionalPaymentBranch {
        choice: ResolutionOptionalPaymentChoice::Decline,
    };
    apply(runner.state_mut(), P0, decline.clone()).expect("decline is legal");
    assert!(
        runner.state().loop_detect_ring.is_empty(),
        "the optional-payment window can precede a life payment, so its answer must clear the ring"
    );
    assert_eq!(runner.state().players[P0.0 as usize].life, life);
    assert!(runner.state().players[P0.0 as usize]
        .hand
        .contains(&cards[0]));
    let after = serde_json::to_string(runner.state()).unwrap();
    assert!(apply(runner.state_mut(), P0, decline).is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), after);
}

#[test]
fn resolution_optional_phyrexian_payment_clears_ring_before_observable_life_move() {
    let (mut runner, source, _) = runner_with_hand(0);
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(
        source,
        vec![AbilityCost::Mana {
            cost: ManaCost::Cost {
                shards: vec![ManaCostShard::PhyrexianBlue],
                generic: 0,
            },
        }],
    );
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    let sampled = runner.state().clone();
    runner
        .state_mut()
        .loop_detect_ring
        .push_back(std::sync::Arc::new(LoopDetectSample {
            normalized: sampled.clone(),
            live: sampled,
        }));

    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .expect("choose the Phyrexian payment branch");
    assert!(
        runner.state().loop_detect_ring.is_empty(),
        "the ring must be cleared before the selected branch can move life"
    );
    assert_eq!(
        runner.state().players[P0.0 as usize].life,
        life + 1,
        "auto-paying 2 life and the 3-life conditional payoff happen during the answer"
    );
}

#[test]
fn resolution_optional_oneof_routes_discard_through_existing_payment() {
    let (mut runner, source, cards) = runner_with_hand(1);
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(source, vec![discard()]);
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .expect("selecting the advertised branch starts canonical payment");
    // The existing executor auto-selects when exactly one legal card exists.
    assert!(runner.state().players[P0.0 as usize]
        .graveyard
        .contains(&cards[0]));
    assert_eq!(runner.state().players[P0.0 as usize].life, life + 3);
}

#[test]
fn resolution_optional_oneof_routes_exile_through_existing_payment() {
    let (mut runner, source, cards) = runner_with_hand(1);
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(
        source,
        vec![AbilityCost::Exile {
            count: 1,
            zone: Some(Zone::Hand),
            filter: None,
        }],
    );
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    apply(
        runner.state_mut(),
        P0,
        GameAction::ChooseResolutionOptionalPaymentBranch {
            choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
        },
    )
    .expect("canonical exile payment starts");
    if matches!(
        runner.state().waiting_for,
        WaitingFor::PayCost { .. } | WaitingFor::EffectZoneChoice { .. }
    ) {
        apply(
            runner.state_mut(),
            P0,
            GameAction::SelectCards {
                cards: vec![cards[0]],
            },
        )
        .expect("canonical exile selection completes");
    }
    assert_eq!(
        runner.state().objects[&cards[0]].zone,
        Zone::Exile,
        "waiting after branch/selection: {:?}",
        runner.state().waiting_for
    );
    assert_eq!(runner.state().players[P0.0 as usize].life, life + 3);
}

#[test]
fn resolution_optional_oneof_routes_mana_through_existing_payment() {
    let (mut runner, source, _) = runner_with_hand(0);
    runner.state_mut().players[P0.0 as usize]
        .mana_pool
        .add(ManaUnit::new(ManaType::Colorless, source, false, vec![]));
    let life = runner.state().players[P0.0 as usize].life;
    let ability = optional_payment(
        source,
        vec![AbilityCost::Mana {
            cost: ManaCost::generic(1),
        }],
    );
    resolve_ability_chain(runner.state_mut(), &ability, &mut Vec::new(), 0).unwrap();
    let pay = GameAction::ChooseResolutionOptionalPaymentBranch {
        choice: ResolutionOptionalPaymentChoice::Pay { index: 0 },
    };
    apply(runner.state_mut(), P0, pay.clone()).expect("canonical mana payment starts");
    if let WaitingFor::ManaPayment { .. } = runner.state().waiting_for {
        let pip_id = runner.state().players[P0.0 as usize].mana_pool.mana[0].pip_id;
        apply(runner.state_mut(), P0, GameAction::SpendPoolMana { pip_id })
            .expect("pool mana is a legal payment pin");
        apply(runner.state_mut(), P0, GameAction::PassPriority).expect("pinned payment finalizes");
    }
    assert!(runner.state().players[P0.0 as usize]
        .mana_pool
        .mana
        .is_empty());
    assert_eq!(runner.state().players[P0.0 as usize].life, life + 3);
    let after = serde_json::to_string(runner.state()).unwrap();
    assert!(apply(runner.state_mut(), P0, pay).is_err());
    assert_eq!(serde_json::to_string(runner.state()).unwrap(), after);
}
