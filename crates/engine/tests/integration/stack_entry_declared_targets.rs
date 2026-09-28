//! `ability_utils::flatten_declared_targets_in_chain` on pending stack entries
//! (CR 115.1, CR 115.10a).

use engine::game::ability_utils::{flatten_declared_targets_in_chain, flatten_targets_in_chain};
use engine::game::combat::AttackTarget;
use engine::game::effects::attach::attach_to;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::types::ability::{Effect, TargetFilter, TargetRef};
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::{GameState, StackEntryKind, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::keywords::Keyword;
use engine::types::mana::{ManaColor, ManaCost};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

const SABOTAGE_STRATEGIST: &str = "Flying, vigilance\nWhenever one or more creatures attack you, those creatures get -1/-0 until end of turn.\nExhaust — {5}{U}{U}: Put three +1/+1 counters on this creature. (Activate each exhaust ability only once.)";
const FRONTIER_WARMONGER: &str = "Whenever one or more creatures attack one of your opponents or a planeswalker they control, those creatures gain menace until end of turn.";
const ENDLESS_COCKROACHES: &str = "When this creature dies, return it to its owner's hand.";
const CONTEMPT: &str = "Enchant creature\nWhen enchanted creature attacks, return it and this Aura to their owners' hands at end of combat.";
const PACIFISM: &str = "Enchant creature\nEnchanted creature can't attack or block.";
const RIPTIDE_GEARHULK: &str = "Double strike\nProwess (Whenever you cast a noncreature spell, this creature gets +1/+1 until end of turn.)\nWhen this creature enters, for each opponent, put up to one target nonland permanent that player controls into its owner's library third from the top.";
const SHOCK: &str = "Shock deals 2 damage to any target.";
const MURDER: &str = "Destroy target creature.";
const TAIL_SWIPE: &str = "Choose target creature you control and target creature you don't control. If you cast this spell during your main phase, the creature you control gets +1/+1 until end of turn. Then those creatures fight each other. (Each deals damage equal to its power to the other.)";
const PSIONIC_ENTITY: &str =
    "{T}: This creature deals 2 damage to any target and 3 damage to itself.";
const AEGIS_ANGEL: &str = "Flying (This creature can't be blocked except by creatures with flying or reach.)\nWhen this creature enters, another target permanent gains indestructible for as long as you control this creature. (Effects that say \"destroy\" don't destroy it. A creature with indestructible can't be destroyed by damage.)";

/// `(flatten_targets_in_chain, flatten_declared_targets_in_chain)` of the
/// stack entry with `id`.
fn both(state: &GameState, id: ObjectId) -> (Vec<TargetRef>, Vec<TargetRef>) {
    let ability = state
        .stack
        .iter()
        .find(|e| e.id == id)
        .and_then(|e| e.ability())
        .expect("entry with an ability");
    (
        flatten_targets_in_chain(ability),
        flatten_declared_targets_in_chain(ability),
    )
}

/// Passes priority until a triggered ability of `source` is on the stack, and
/// returns the newest one.
fn trigger_of(runner: &mut GameRunner, source: ObjectId) -> ObjectId {
    for _ in 0..24 {
        if let Some(entry) = runner.state().stack.iter().rev().find(|e| {
            e.source_id == source && matches!(e.kind, StackEntryKind::TriggeredAbility { .. })
        }) {
            return entry.id;
        }
        runner.act(GameAction::PassPriority).expect("pass priority");
    }
    panic!("no trigger of {source:?} reached the stack");
}

/// The built scenario with P1 active and holding priority.
fn p1_priority(scenario: GameScenario) -> GameRunner {
    let mut runner = scenario.build();
    let state = runner.state_mut();
    state.active_player = P1;
    state.priority_player = P1;
    state.waiting_for = WaitingFor::Priority { player: P1 };
    runner
}

fn give_keyword(runner: &mut GameRunner, id: ObjectId, keyword: Keyword) {
    let object = runner.state_mut().objects.get_mut(&id).expect("object");
    object.keywords.push(keyword.clone());
    object.base_keywords.push(keyword);
    runner.state_mut().layers_dirty.mark_full();
}

/// P0's 2/2 attacks P1, who controls Sabotage Strategist.
fn sabotage_board() -> (GameRunner, ObjectId, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Bear", 2, 2).id();
    let strategist = {
        let mut b = scenario.add_creature(P1, "Sabotage Strategist", 2, 2);
        b.from_oracle_text_with_keywords(&["Exhaust", "Flying", "Vigilance"], SABOTAGE_STRATEGIST);
        b.id()
    };
    let mut runner = scenario.build();
    runner.advance_to_combat();
    runner
        .declare_attackers(&[(attacker, AttackTarget::Player(P1))])
        .expect("declare attackers");
    (runner, attacker, strategist)
}

#[test]
fn a_batched_attack_trigger_targets_none_of_the_attackers_it_holds() {
    let (mut runner, attacker, strategist) = sabotage_board();
    let entry = trigger_of(&mut runner, strategist);
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(attacker)],
        "reach guard: the trigger holds the attacker"
    );
    assert_eq!(declared, Vec::new());

    give_keyword(&mut runner, attacker, Keyword::Shroud);
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&attacker].power,
        Some(1),
        "CR 115.10a + CR 702.18a: shroud does not stop an ability that does not target"
    );
}

#[test]
fn a_granting_attack_trigger_targets_none_of_the_attackers_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_creature(P0, "Bear", 2, 2).id();
    let warmonger = scenario
        .add_creature_from_oracle(P0, "Frontier Warmonger", 4, 4, FRONTIER_WARMONGER)
        .id();
    let mut runner = scenario.build();
    runner.advance_to_combat();
    runner
        .declare_attackers(&[(attacker, AttackTarget::Player(P1))])
        .expect("declare attackers");
    let entry = trigger_of(&mut runner, warmonger);
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(attacker)],
        "reach guard: the trigger holds the attacker"
    );
    assert!(
        matches!(
            runner
                .state()
                .stack
                .iter()
                .find(|e| e.id == entry)
                .and_then(|e| e.ability())
                .map(|a| &a.effect),
            Some(Effect::GenericEffect { target: None, .. })
        ),
        "reach guard: the effect has no target filter"
    );
    assert_eq!(declared, Vec::new());
}

#[test]
fn a_dies_trigger_targets_none_of_the_card_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let roach = scenario
        .add_creature_from_oracle(P0, "Endless Cockroaches", 1, 1, ENDLESS_COCKROACHES)
        .id();
    let murder = scenario
        .add_spell_to_hand_from_oracle(P1, "Murder", true, MURDER)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = p1_priority(scenario);
    runner.cast(murder).target_objects(&[roach]).commit();
    let entry = trigger_of(&mut runner, roach);
    assert_eq!(
        runner.state().objects[&roach].zone,
        Zone::Graveyard,
        "reach guard: the creature died"
    );
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(roach)],
        "reach guard: the trigger holds the card that died"
    );
    assert_eq!(declared, Vec::new());
}

#[test]
fn a_delayed_trigger_targets_none_of_the_creature_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P0, "Bear", 2, 2).id();
    let contempt = scenario
        .add_creature(P1, "Contempt", 0, 0)
        .as_enchantment()
        .with_subtypes(vec!["Aura"])
        .from_oracle_text_with_keywords(&["Enchant"], CONTEMPT)
        .id();
    let mut runner = scenario.build();
    attach_to(runner.state_mut(), contempt, bear);
    runner.advance_to_combat();
    runner
        .declare_attackers(&[(bear, AttackTarget::Player(P1))])
        .expect("declare attackers");
    let attack_trigger = trigger_of(&mut runner, contempt);
    let (_, declared_attack) = both(runner.state(), attack_trigger);
    assert_eq!(
        declared_attack,
        Vec::new(),
        "the attack trigger targets nothing"
    );
    let mut delayed = None;
    for _ in 0..24 {
        if let Some(entry) = runner.state().stack.iter().find(|e| {
            e.id != attack_trigger
                && e.source_id == contempt
                && matches!(e.kind, StackEntryKind::TriggeredAbility { .. })
                && e.ability().is_some_and(|a| {
                    matches!(
                        a.effect,
                        Effect::Bounce {
                            target: TargetFilter::TriggeringSource,
                            ..
                        }
                    )
                })
        }) {
            delayed = Some(entry.id);
            break;
        }
        if let WaitingFor::OrderTriggers { triggers, .. } = &runner.state().waiting_for {
            let order = (0..triggers.len()).collect();
            runner
                .act(GameAction::OrderTriggers { order })
                .expect("order triggers");
        } else {
            runner.act(GameAction::PassPriority).expect("pass priority");
        }
    }
    let delayed = delayed.expect("reach guard: the delayed return is on the stack");
    let (held, declared) = both(runner.state(), delayed);
    assert!(
        held.contains(&TargetRef::Object(bear)),
        "reach guard: the delayed trigger holds the creature"
    );
    assert_eq!(declared, Vec::new());

    give_keyword(&mut runner, bear, Keyword::Shroud);
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&bear].zone,
        Zone::Hand,
        "CR 115.10a + CR 702.18a: shroud does not stop an ability that does not target"
    );
}

#[test]
fn shock_targets_the_creature_it_is_cast_at() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Bear", 2, 2).id();
    let shock = scenario
        .add_spell_to_hand_from_oracle(P0, "Shock", true, SHOCK)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    scenario.add_basic_land(P0, ManaColor::Red);
    let mut runner = scenario.build();
    runner.cast(shock).target_objects(&[bear]).commit();
    let entry = runner.state().stack.back().expect("Shock").id;
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(held, vec![TargetRef::Object(bear)]);
    assert_eq!(declared, vec![TargetRef::Object(bear)]);
}

#[test]
fn tail_swipe_targets_both_creatures_its_fight_inherits() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let own = scenario.add_creature(P0, "Own", 3, 3).id();
    let theirs = scenario.add_creature(P1, "Theirs", 4, 4).id();
    let card = scenario
        .add_spell_to_hand_from_oracle(P0, "Tail Swipe", true, TAIL_SWIPE)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    scenario.add_basic_land(P0, ManaColor::Green);
    let mut runner = scenario.build();
    runner.cast(card).target_objects(&[own, theirs]).commit();
    let entry = runner.state().stack.back().expect("Tail Swipe").id;
    let ability = runner
        .state()
        .stack
        .back()
        .and_then(|e| e.ability())
        .expect("ability");
    let mut node = Some(ability);
    let mut fight_targets = None;
    while let Some(n) = node {
        if matches!(n.effect, Effect::Fight { .. }) {
            fight_targets = Some(n.targets.clone());
        }
        node = n.sub_ability.as_deref();
    }
    assert_eq!(
        fight_targets,
        Some(Vec::new()),
        "reach guard: the fight holds no targets of its own"
    );
    let (_, declared) = both(runner.state(), entry);
    assert_eq!(
        declared,
        vec![TargetRef::Object(own), TargetRef::Object(theirs)]
    );
}

fn entity_board(aim_at_self: bool) -> (GameRunner, ObjectId, ObjectId, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P0, "Bear", 2, 2).id();
    let entity = scenario
        .add_creature_from_oracle(P1, "Psionic Entity", 2, 2, PSIONIC_ENTITY)
        .id();
    let mut runner = p1_priority(scenario);
    runner
        .state_mut()
        .objects
        .get_mut(&entity)
        .expect("entity")
        .summoning_sick = false;
    let aim = if aim_at_self { entity } else { bear };
    runner
        .act(GameAction::ActivateAbility {
            source_id: entity,
            ability_index: 0,
        })
        .expect("activate");
    runner
        .act(GameAction::ChooseTarget {
            target: Some(TargetRef::Object(aim)),
        })
        .expect("choose the target");
    let entry = runner.state().stack.back().expect("ability").id;
    (runner, entry, entity, bear)
}

#[test]
fn psionic_entity_targets_what_it_is_aimed_at_and_not_itself() {
    let (runner, entry, entity, bear) = entity_board(false);
    let ability = runner
        .state()
        .stack
        .back()
        .and_then(|e| e.ability())
        .expect("ability");
    let self_damage = ability
        .sub_ability
        .as_deref()
        .expect("reach guard: a second node");
    assert!(
        matches!(self_damage.effect, Effect::DealDamage { .. }),
        "reach guard: the second node deals damage"
    );
    let (_, declared) = both(runner.state(), entry);
    assert_eq!(declared, vec![TargetRef::Object(bear)]);
    assert!(!declared.contains(&TargetRef::Object(entity)));
}

#[test]
fn psionic_entity_aimed_at_itself_targets_itself() {
    let (runner, entry, entity, _) = entity_board(true);
    let (_, declared) = both(runner.state(), entry);
    assert_eq!(declared, vec![TargetRef::Object(entity)]);
}

#[test]
fn an_enters_trigger_targets_the_permanent_it_declares() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P0, "Bear", 2, 2).id();
    let angel = scenario
        .add_creature_to_hand_from_oracle(P0, "Aegis Angel", 5, 5, AEGIS_ANGEL)
        .from_oracle_text_with_keywords(&["Flying"], AEGIS_ANGEL)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(angel).commit();
    let entry = trigger_of(&mut runner, angel);
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(bear)],
        "reach guard: the trigger holds its target"
    );
    assert_eq!(declared, vec![TargetRef::Object(bear)]);
}

#[test]
fn an_aura_spell_targets_the_creature_it_will_enchant() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Bear", 2, 2).id();
    let pacifism = scenario
        .add_spell_to_hand_from_oracle(P0, "Pacifism", false, PACIFISM)
        .as_enchantment()
        .with_subtypes(vec!["Aura"])
        .from_oracle_text_with_keywords(&["Enchant"], PACIFISM)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(pacifism).target_objects(&[bear]).commit();
    let entry = runner.state().stack.back().expect("Pacifism").id;
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(bear)],
        "reach guard: the spell holds its target"
    );
    assert_eq!(declared, vec![TargetRef::Object(bear)]);
}

#[test]
fn a_per_opponent_trigger_targets_only_the_permanents_it_is_aimed_at() {
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let p2 = PlayerId(2);
    let first = scenario.add_creature(P1, "First Bear", 2, 2).id();
    let second = scenario.add_creature(p2, "Second Bear", 2, 2).id();
    let gearhulk = scenario
        .add_creature_to_hand_from_oracle(P0, "Riptide Gearhulk", 4, 4, RIPTIDE_GEARHULK)
        .from_oracle_text_with_keywords(&["Double strike", "Prowess"], RIPTIDE_GEARHULK)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(gearhulk).commit();
    for _ in 0..12 {
        match &runner.state().waiting_for {
            WaitingFor::TriggerTargetSelection { .. } | WaitingFor::TargetSelection { .. } => {
                runner.choose_first_legal_target().expect("choose a target");
            }
            _ if runner.state().stack.iter().any(|e| {
                e.source_id == gearhulk && matches!(e.kind, StackEntryKind::TriggeredAbility { .. })
            }) =>
            {
                break;
            }
            _ => {
                runner.act(GameAction::PassPriority).expect("pass priority");
            }
        }
    }
    let entry = trigger_of(&mut runner, gearhulk);
    let ability = runner
        .state()
        .stack
        .iter()
        .find(|e| e.id == entry)
        .and_then(|e| e.ability())
        .expect("ability");
    let (_, declared) = both(runner.state(), entry);
    assert!(
        ability
            .targets
            .iter()
            .any(|t| matches!(t, TargetRef::Player(_))),
        "reach guard: the fanout node also holds the opponents it iterates"
    );
    assert_eq!(
        declared,
        vec![TargetRef::Object(first), TargetRef::Object(second)]
    );
}

const KIKI_JIKI: &str = "Haste\n{T}: Create a token that's a copy of target nonlegendary creature you control, except it has haste. Sacrifice it at the beginning of the next end step.";
const TWINCAST: &str =
    "Copy target instant or sorcery spell. You may choose new targets for the copy.";

/// Passes priority, ordering triggers as listed, until `done` holds. Returns
/// every event those actions emitted.
fn pass_until(runner: &mut GameRunner, done: impl Fn(&GameState) -> bool) -> Vec<GameEvent> {
    let mut events = Vec::new();
    for _ in 0..60 {
        if done(runner.state()) {
            return events;
        }
        let action = match &runner.state().waiting_for {
            WaitingFor::Priority { .. } => GameAction::PassPriority,
            WaitingFor::OrderTriggers { triggers, .. } => GameAction::OrderTriggers {
                order: (0..triggers.len()).collect(),
            },
            WaitingFor::DeclareAttackers { .. } => GameAction::DeclareAttackers {
                attacks: Vec::new(),
                bands: Vec::new(),
            },
            WaitingFor::DeclareBlockers { .. } => GameAction::DeclareBlockers {
                assignments: Vec::new(),
            },
            other => panic!("unexpected prompt {other:?}"),
        };
        events.extend(runner.act(action).expect("pass").events);
    }
    panic!("the awaited state was not reached");
}

/// The newest triggered stack entry of `source`, if any.
fn pending_trigger_of(state: &GameState, source: ObjectId) -> Option<ObjectId> {
    state
        .stack
        .iter()
        .rev()
        .find(|e| {
            e.source_id == source && matches!(e.kind, StackEntryKind::TriggeredAbility { .. })
        })
        .map(|e| e.id)
}

#[test]
fn an_end_step_sacrifice_targets_none_of_the_token_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let kiki = scenario
        .add_creature_from_oracle(P0, "Kiki-Jiki, Mirror Breaker", 2, 2, KIKI_JIKI)
        .from_oracle_text_with_keywords(&["Haste"], KIKI_JIKI)
        .id();
    let bear = scenario.add_creature(P0, "Bear", 2, 2).id();
    let mut runner = scenario.build();
    runner
        .act(GameAction::ActivateAbility {
            source_id: kiki,
            ability_index: 0,
        })
        .expect("activate");
    if matches!(
        runner.state().waiting_for,
        WaitingFor::TargetSelection { .. }
    ) {
        runner
            .act(GameAction::ChooseTarget {
                target: Some(TargetRef::Object(bear)),
            })
            .expect("choose the creature to copy");
    }
    runner.advance_until_stack_empty();
    let token = runner
        .state()
        .objects
        .values()
        .find(|o| o.zone == Zone::Battlefield && o.id != bear && o.name == "Bear")
        .map(|o| o.id)
        .expect("reach guard: the token copy");
    pass_until(&mut runner, |state| {
        state.phase == Phase::End && pending_trigger_of(state, kiki).is_some()
    });
    let entry = pending_trigger_of(runner.state(), kiki).expect("the delayed sacrifice");
    let (held, declared) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(token)],
        "reach guard: the delayed sacrifice holds the token"
    );
    assert_eq!(declared, Vec::new());
}

/// `other` is a second creature of P1's.
fn shock_on_the_stack(scenario: &mut GameScenario) -> (ObjectId, ObjectId, ObjectId) {
    scenario.at_phase(Phase::PreCombatMain);
    let first = scenario.add_creature(P1, "First Bear", 2, 2).id();
    let other = scenario.add_creature(P1, "Other Bear", 2, 2).id();
    let shock = scenario
        .add_spell_to_hand_from_oracle(P0, "Shock", true, SHOCK)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    (first, other, shock)
}

fn declared_of(state: &GameState, id: ObjectId) -> Vec<TargetRef> {
    both(state, id).1
}

/// P0 casts Shock at P1's first creature, then Twincast at the Shock, and
/// passes until the copy's retarget prompt opens.
fn twincast_board() -> (GameRunner, ObjectId, ObjectId, ObjectId) {
    let mut scenario = GameScenario::new();
    let (first, _, shock) = shock_on_the_stack(&mut scenario);
    let twincast = scenario
        .add_spell_to_hand_from_oracle(P0, "Twincast", true, TWINCAST)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(shock).target_objects(&[first]).commit();
    runner.cast(twincast).target_objects(&[shock]).commit();
    pass_until(&mut runner, |state| {
        matches!(
            state.waiting_for,
            WaitingFor::OptionalEffectChoice { .. } | WaitingFor::CopyRetarget { .. }
        )
    });
    if matches!(
        runner.state().waiting_for,
        WaitingFor::OptionalEffectChoice { .. }
    ) {
        runner
            .act(GameAction::DecideOptionalEffect { accept: true })
            .expect("choose new targets");
    }
    let WaitingFor::CopyRetarget { copy_id, .. } = runner.state().waiting_for.clone() else {
        panic!("expected the copy's retarget prompt");
    };
    (runner, first, shock, copy_id)
}

#[test]
fn a_copy_that_keeps_its_targets_targets_what_the_original_does() {
    let (mut runner, first, _, copy) = twincast_board();
    runner
        .act(GameAction::KeepAllCopyTargets)
        .expect("keep the copy's targets");
    assert_eq!(
        declared_of(runner.state(), copy),
        vec![TargetRef::Object(first)],
        "CR 707.10"
    );
}
