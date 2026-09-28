//! `ability_utils::flatten_declared_targets_in_chain` on pending stack entries
//! (CR 115.1, CR 115.10a).

use engine::game::ability_utils::{flatten_declared_targets_in_chain, flatten_targets_in_chain};
use engine::game::combat::AttackTarget;
use engine::game::effects::attach::attach_to;
use engine::game::game_object::AttachTarget;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::types::ability::{Effect, TargetFilter, TargetRef};
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::{
    GameState, RetargetScope, ShardChoice, StackEntryKind, WaitingFor,
};
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
const VAMPIRE_SOVEREIGN: &str =
    "Flying\nWhen this creature enters, target opponent loses 3 life and you gain 3 life.";
const SWOOPING_PTERANODON: &str = "Flying, haste\nWhenever this creature or another Dinosaur you control with flying enters, gain control of target creature an opponent controls until end of turn. Untap that creature. It gains flying and haste until end of turn. At the beginning of the next end step, target land deals 3 damage to that creature.";
const FATAL_FISSURE: &str = "Choose target creature. When that creature dies this turn, you earthbend 4. (Target land you control becomes a 0/0 creature with haste that's still a land. Put four +1/+1 counters on it. When it dies or is exiled, return it to the battlefield tapped.)";
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

fn crimes(state: &GameState, player: PlayerId) -> u32 {
    state
        .players
        .iter()
        .find(|p| p.id == player)
        .expect("player")
        .crimes_committed_this_turn
}

#[test]
fn a_batched_attack_trigger_commits_no_crime() {
    let (mut runner, attacker, strategist) = sabotage_board();
    let entry = trigger_of(&mut runner, strategist);
    let (held, _) = both(runner.state(), entry);
    assert_eq!(
        held,
        vec![TargetRef::Object(attacker)],
        "reach guard: the trigger holds the attacker"
    );
    assert_eq!(
        runner.state().objects[&attacker].controller,
        P0,
        "reach guard: the attacker is P1's opponent's"
    );
    assert_eq!(crimes(runner.state(), P1), 0, "CR 700.13");
}

#[test]
fn an_enters_trigger_that_targets_an_opponent_commits_a_crime() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let sovereign = scenario
        .add_creature_to_hand_from_oracle(P0, "Vampire Sovereign", 3, 4, VAMPIRE_SOVEREIGN)
        .from_oracle_text_with_keywords(&["Flying"], VAMPIRE_SOVEREIGN)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(sovereign).commit();
    let entry = trigger_of(&mut runner, sovereign);
    let (_, declared) = both(runner.state(), entry);
    assert_eq!(
        declared,
        vec![TargetRef::Player(P1)],
        "reach guard: the trigger targets the opponent"
    );
    assert_eq!(crimes(runner.state(), P0), 1, "CR 700.13");
}

/// P0's Swooping Pteranodon has taken P1's 2/2, and P0 controls `lands`
/// Mountains. Passes to the end step until the Pteranodon's delayed trigger is
/// on the stack, choosing the first Mountain if asked. Returns the Mountains,
/// the trigger, whether a target prompt opened, and the target of every
/// `GameEvent::BecomesTarget` emitted from the end of the enters trigger on.
fn pteranodon_end_step(
    lands: usize,
) -> (GameRunner, Vec<ObjectId>, ObjectId, bool, Vec<TargetRef>) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PostCombatMain);
    let bear = scenario.add_creature(P1, "Bear", 2, 2).id();
    let pteranodon = scenario
        .add_creature_to_hand_from_oracle(P0, "Swooping Pteranodon", 3, 3, SWOOPING_PTERANODON)
        .from_oracle_text_with_keywords(&["Flying", "Haste"], SWOOPING_PTERANODON)
        .with_subtypes(vec!["Dinosaur"])
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mountains: Vec<ObjectId> = (0..lands)
        .map(|_| scenario.add_basic_land(P0, ManaColor::Red))
        .collect();
    let mut runner = scenario.build();
    runner.cast(pteranodon).commit();
    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&bear].controller,
        P0,
        "reach guard: the enters trigger resolved"
    );
    let mut prompted = false;
    let mut targeted = Vec::new();
    for _ in 0..40 {
        let state = runner.state();
        if state.phase == Phase::End
            && matches!(state.waiting_for, WaitingFor::Priority { .. })
            && !state.stack.is_empty()
        {
            break;
        }
        let action = if matches!(state.waiting_for, WaitingFor::TriggerTargetSelection { .. }) {
            prompted = true;
            GameAction::ChooseTarget {
                target: Some(TargetRef::Object(mountains[0])),
            }
        } else {
            GameAction::PassPriority
        };
        let result = runner.act(action).expect("advance to the end step");
        targeted.extend(result.events.into_iter().filter_map(|event| match event {
            GameEvent::BecomesTarget { target, .. } => Some(target),
            _ => None,
        }));
    }
    let entry = trigger_of(&mut runner, pteranodon);
    (runner, mountains, entry, prompted, targeted)
}

#[test]
fn an_end_step_trigger_makes_only_its_one_legal_target_a_target() {
    let (runner, mountains, entry, prompted, targeted) = pteranodon_end_step(1);
    assert!(
        !prompted,
        "reach guard: the only land was chosen without a prompt"
    );
    let (held, _) = both(runner.state(), entry);
    assert!(
        held.iter().any(|t| *t != TargetRef::Object(mountains[0])),
        "reach guard: the trigger also holds an object it does not target"
    );
    assert_eq!(
        targeted,
        vec![TargetRef::Object(mountains[0])],
        "CR 115.10a"
    );
}

#[test]
fn an_end_step_trigger_makes_only_its_chosen_target_a_target() {
    let (runner, mountains, entry, prompted, targeted) = pteranodon_end_step(2);
    assert!(prompted, "reach guard: the land was chosen at a prompt");
    let (held, _) = both(runner.state(), entry);
    assert!(
        held.iter().any(|t| *t != TargetRef::Object(mountains[0])),
        "reach guard: the trigger also holds an object it does not target"
    );
    assert_eq!(
        targeted,
        vec![TargetRef::Object(mountains[0])],
        "CR 115.10a"
    );
}

/// A Fatal Fissure board once its delayed trigger is on the stack.
struct FissureTrigger {
    runner: GameRunner,
    bear: ObjectId,
    swamps: Vec<ObjectId>,
    entry: ObjectId,
    /// The query's answer for the trigger's entry at its target prompt, if
    /// one opened.
    at_prompt: Option<Vec<TargetRef>>,
    /// `GameEvent::BecomesTarget` targets and `GameEvent::CrimeCommitted`
    /// players emitted by the actions after Murder is cast.
    targeted: Vec<TargetRef>,
    criminals: Vec<PlayerId>,
}

/// P0, who controls `lands` Swamps, casts Fatal Fissure at P1's 2/2 and it
/// resolves. P0 then casts Murder at the 2/2 and passes until the delayed
/// trigger is on the stack, choosing the first Swamp if asked.
fn fissure_trigger(lands: usize) -> FissureTrigger {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Bear", 2, 2).id();
    let fissure = scenario
        .add_spell_to_hand_from_oracle(P0, "Fatal Fissure", true, FATAL_FISSURE)
        .from_oracle_text_with_keywords(&["Earthbend"], FATAL_FISSURE)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let murder = scenario
        .add_spell_to_hand_from_oracle(P0, "Murder", true, MURDER)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let swamps: Vec<ObjectId> = (0..lands)
        .map(|_| scenario.add_basic_land(P0, ManaColor::Black))
        .collect();
    let mut runner = scenario.build();
    runner.cast(fissure).target_objects(&[bear]).commit();
    runner.advance_until_stack_empty();
    runner.cast(murder).target_objects(&[bear]).commit();
    let mut at_prompt = None;
    let mut targeted = Vec::new();
    let mut criminals = Vec::new();
    for _ in 0..40 {
        let state = runner.state();
        let pending = state.stack.iter().rev().find(|e| {
            e.source_id == fissure && matches!(e.kind, StackEntryKind::TriggeredAbility { .. })
        });
        let action = match (&state.waiting_for, pending) {
            (WaitingFor::Priority { .. }, Some(_)) => break,
            (WaitingFor::TriggerTargetSelection { .. }, Some(entry)) => {
                at_prompt = Some(both(state, entry.id).1);
                GameAction::ChooseTarget {
                    target: Some(TargetRef::Object(swamps[0])),
                }
            }
            _ => GameAction::PassPriority,
        };
        let result = runner.act(action).expect("advance to the delayed trigger");
        for event in result.events {
            match event {
                GameEvent::BecomesTarget { target, .. } => targeted.push(target),
                GameEvent::CrimeCommitted { player_id } => criminals.push(player_id),
                _ => {}
            }
        }
    }
    let entry = trigger_of(&mut runner, fissure);
    assert_eq!(
        runner.state().objects[&bear].zone,
        Zone::Graveyard,
        "reach guard: the creature the delayed trigger waited on died"
    );
    FissureTrigger {
        runner,
        bear,
        swamps,
        entry,
        at_prompt,
        targeted,
        criminals,
    }
}

#[test]
fn a_delayed_trigger_with_one_legal_land_targets_only_the_land() {
    let fissure = fissure_trigger(1);
    let swamp = TargetRef::Object(fissure.swamps[0]);
    assert_eq!(
        fissure.at_prompt, None,
        "reach guard: the only land was chosen without a prompt"
    );
    let (held, declared) = both(fissure.runner.state(), fissure.entry);
    assert!(
        held.contains(&TargetRef::Object(fissure.bear)),
        "reach guard: the trigger also holds the creature it waited on"
    );
    assert_eq!(declared, vec![swamp.clone()]);
    assert_eq!(fissure.targeted, vec![swamp], "CR 115.10a");
    assert_eq!(fissure.criminals, Vec::new(), "CR 700.13");
}

#[test]
fn a_delayed_trigger_with_a_chosen_land_targets_only_the_land() {
    let fissure = fissure_trigger(2);
    assert!(
        fissure.at_prompt.is_some(),
        "reach guard: the land was chosen at a prompt"
    );
    assert!(
        both(fissure.runner.state(), fissure.entry)
            .0
            .contains(&TargetRef::Object(fissure.bear)),
        "reach guard: the trigger also holds the creature it waited on"
    );
    assert_eq!(fissure.at_prompt, Some(Vec::new()));
    assert_eq!(
        both(fissure.runner.state(), fissure.entry).1,
        vec![TargetRef::Object(fissure.swamps[0])]
    );
    assert_eq!(
        fissure.targeted,
        vec![TargetRef::Object(fissure.swamps[0])],
        "CR 115.10a"
    );
    assert_eq!(fissure.criminals, Vec::new(), "CR 700.13");
}

const GIFT_OF_IMMORTALITY: &str = "Enchant creature\nWhen enchanted creature dies, return that card to the battlefield under its owner's control. Return this card to the battlefield attached to that creature at the beginning of the next end step.";
const BASALT_GOLEM: &str = "This creature can't be blocked by artifact creatures.\nWhenever this creature becomes blocked by a creature, that creature's controller sacrifices it at end of combat. If the player does, they create a 0/2 colorless Wall artifact creature token with defender.";
const KIKI_JIKI: &str = "Haste\n{T}: Create a token that's a copy of target nonlegendary creature you control, except it has haste. Sacrifice it at the beginning of the next end step.";
const REDIRECT: &str = "You may choose new targets for target spell.";
const SPELLSKITE: &str = "{U/P}: Change a target of target spell or ability to this creature. ({U/P} can be paid with either {U} or 2 life.)";
const TWINCAST: &str =
    "Copy target instant or sorcery spell. You may choose new targets for the copy.";
const ZADA: &str = "Whenever you cast an instant or sorcery spell that targets only Zada, copy that spell for each other creature you control that the spell could target. Each copy targets a different one of those creatures.";
const GIANT_GROWTH: &str = "Target creature gets +3/+3 until end of turn.";

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

fn criminals(events: &[GameEvent]) -> Vec<PlayerId> {
    events
        .iter()
        .filter_map(|event| match event {
            GameEvent::CrimeCommitted { player_id } => Some(*player_id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_delayed_return_targets_none_of_the_creature_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bear = scenario.add_creature(P1, "Bear", 2, 2).id();
    let gift = scenario
        .add_enchantment_from_oracle(P0, "Gift of Immortality", GIFT_OF_IMMORTALITY)
        .with_subtypes(vec!["Aura"])
        .from_oracle_text_with_keywords(&["Enchant"], GIFT_OF_IMMORTALITY)
        .id();
    let murder = scenario
        .add_spell_to_hand_from_oracle(P1, "Murder", true, MURDER)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = p1_priority(scenario);
    attach_to(runner.state_mut(), gift, bear);
    runner.cast(murder).target_objects(&[bear]).commit();
    let mut events = pass_until(&mut runner, |state| {
        state.phase == Phase::End && pending_trigger_of(state, gift).is_some()
    });
    let entry = pending_trigger_of(runner.state(), gift).expect("the delayed return");
    let (held, declared) = both(runner.state(), entry);
    assert!(
        held.contains(&TargetRef::Object(bear)),
        "reach guard: the delayed return holds the creature"
    );
    assert_eq!(
        runner.state().objects[&bear].controller,
        P1,
        "reach guard: the creature is P0's opponent's"
    );
    assert_eq!(declared, Vec::new());
    events.extend(pass_until(&mut runner, |state| state.stack.is_empty()));
    assert_eq!(
        runner.state().objects[&gift].attached_to,
        Some(AttachTarget::Object(bear)),
        "reach guard: the return resolved"
    );
    assert_eq!(criminals(&events), Vec::new(), "CR 700.13");
}

#[test]
fn a_delayed_sacrifice_targets_none_of_the_blocker_it_holds() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let golem = scenario
        .add_creature_from_oracle(P0, "Basalt Golem", 2, 4, BASALT_GOLEM)
        .id();
    let blocker = scenario.add_creature(P1, "Blocker", 0, 5).id();
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&golem)
        .expect("golem")
        .summoning_sick = false;
    runner.advance_to_combat();
    runner
        .declare_attackers(&[(golem, AttackTarget::Player(P1))])
        .expect("declare attackers");
    let mut events = pass_until(&mut runner, |state| {
        matches!(state.waiting_for, WaitingFor::DeclareBlockers { .. })
    });
    events.extend(
        runner
            .declare_blockers(&[(blocker, golem)])
            .expect("declare blockers")
            .events,
    );
    events.extend(pass_until(&mut runner, |state| {
        state.phase == Phase::EndCombat && pending_trigger_of(state, golem).is_some()
    }));
    let entry = pending_trigger_of(runner.state(), golem).expect("the delayed sacrifice");
    let (held, declared) = both(runner.state(), entry);
    assert!(
        held.contains(&TargetRef::Object(blocker)),
        "reach guard: the delayed sacrifice holds the blocker"
    );
    assert_eq!(
        runner.state().objects[&blocker].controller,
        P1,
        "reach guard: the blocker is P0's opponent's"
    );
    assert_eq!(declared, Vec::new());
    assert_eq!(criminals(&events), Vec::new(), "CR 700.13");
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

#[test]
fn a_spell_given_new_targets_targets_the_new_ones() {
    let mut scenario = GameScenario::new();
    let (first, other, shock) = shock_on_the_stack(&mut scenario);
    let redirect = scenario
        .add_spell_to_hand_from_oracle(P1, "Redirect", true, REDIRECT)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(shock).target_objects(&[first]).commit();
    assert_eq!(
        declared_of(runner.state(), shock),
        vec![TargetRef::Object(first)],
        "reach guard: Shock was cast at the first creature"
    );
    runner.act(GameAction::PassPriority).expect("pass to P1");
    runner.cast(redirect).target_objects(&[shock]).commit();
    pass_until(&mut runner, |state| {
        matches!(
            state.waiting_for,
            WaitingFor::OptionalEffectChoice { .. } | WaitingFor::RetargetChoice { .. }
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
    runner
        .act(GameAction::RetargetSpell {
            new_targets: vec![TargetRef::Object(other)],
        })
        .expect("retarget");
    let (held, declared) = both(runner.state(), shock);
    assert_eq!(
        held,
        vec![TargetRef::Object(other)],
        "reach guard: the new target was written"
    );
    assert_eq!(declared, vec![TargetRef::Object(other)], "CR 115.7d");
}

#[test]
fn a_spell_given_new_targets_at_a_prompt_without_slot_addresses_targets_the_new_ones() {
    let mut scenario = GameScenario::new();
    let (first, other, shock) = shock_on_the_stack(&mut scenario);
    let mut runner = scenario.build();
    runner.cast(shock).target_objects(&[first]).commit();
    let index = runner
        .state()
        .stack
        .iter()
        .position(|e| e.id == shock)
        .expect("Shock is on the stack");
    // A retarget prompt saved before prompts carried slot addresses.
    runner.state_mut().waiting_for = WaitingFor::RetargetChoice {
        player: P1,
        stack_entry_index: index,
        scope: RetargetScope::All,
        current_targets: vec![TargetRef::Object(first)],
        slots: Vec::new(),
        slot_pools: Vec::new(),
        legal_new_targets: vec![TargetRef::Object(first), TargetRef::Object(other)],
    };
    runner
        .act(GameAction::RetargetSpell {
            new_targets: vec![TargetRef::Object(other)],
        })
        .expect("retarget");
    let (held, declared) = both(runner.state(), shock);
    assert_eq!(
        held,
        vec![TargetRef::Object(other)],
        "reach guard: the new target was written"
    );
    assert_eq!(declared, vec![TargetRef::Object(other)], "CR 115.7d");
}

#[test]
fn a_spell_whose_target_is_changed_to_a_creature_targets_that_creature() {
    let mut scenario = GameScenario::new();
    let (first, _, shock) = shock_on_the_stack(&mut scenario);
    let spellskite = scenario
        .add_creature_from_oracle(P1, "Spellskite", 0, 4, SPELLSKITE)
        .id();
    let mut runner = scenario.build();
    runner.cast(shock).target_objects(&[first]).commit();
    runner.act(GameAction::PassPriority).expect("pass to P1");
    runner
        .act(GameAction::ActivateAbility {
            source_id: spellskite,
            ability_index: 0,
        })
        .expect("activate");
    for _ in 0..8 {
        match &runner.state().waiting_for {
            WaitingFor::TargetSelection { .. } => {
                runner
                    .act(GameAction::ChooseTarget {
                        target: Some(TargetRef::Object(shock)),
                    })
                    .expect("target Shock");
            }
            WaitingFor::PhyrexianPayment { .. } => {
                runner
                    .act(GameAction::SubmitPhyrexianChoices {
                        choices: vec![ShardChoice::PayLife],
                    })
                    .expect("pay 2 life");
            }
            WaitingFor::Priority { .. } => break,
            other => panic!("unexpected prompt {other:?}"),
        }
    }
    pass_until(&mut runner, |state| state.stack.len() == 1);
    let (held, declared) = both(runner.state(), shock);
    assert_eq!(
        held,
        vec![TargetRef::Object(spellskite)],
        "reach guard: the new target was written"
    );
    assert_eq!(declared, vec![TargetRef::Object(spellskite)], "CR 115.7b");
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
fn a_copy_given_a_new_target_targets_it() {
    let (mut runner, first, shock, copy) = twincast_board();
    runner
        .act(GameAction::ChooseTarget {
            target: Some(TargetRef::Player(P1)),
        })
        .expect("choose the copy's new target");
    let (held, declared) = both(runner.state(), copy);
    assert_eq!(
        held,
        vec![TargetRef::Player(P1)],
        "reach guard: the new target was written"
    );
    assert_eq!(declared, vec![TargetRef::Player(P1)], "CR 707.10c");
    assert_eq!(
        declared_of(runner.state(), shock),
        vec![TargetRef::Object(first)]
    );
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

#[test]
fn each_copy_made_for_another_creature_targets_that_creature() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let zada = scenario
        .add_creature_from_oracle(P0, "Zada, Hedron Grinder", 3, 3, ZADA)
        .as_legendary()
        .id();
    let bear = scenario.add_creature(P0, "Bear", 2, 2).id();
    let growth = scenario
        .add_spell_to_hand_from_oracle(P0, "Giant Growth", true, GIANT_GROWTH)
        .with_mana_cost(ManaCost::generic(0))
        .id();
    let mut runner = scenario.build();
    runner.cast(growth).target_objects(&[zada]).commit();
    pass_until(&mut runner, |state| {
        state.stack.len() == 2
            && state
                .stack
                .iter()
                .all(|e| matches!(e.kind, StackEntryKind::Spell { .. }))
    });
    let copy = runner.state().stack.back().expect("the copy").id;
    assert_ne!(copy, growth, "reach guard: a copy was put on the stack");
    let (held, declared) = both(runner.state(), copy);
    assert_eq!(
        held,
        vec![TargetRef::Object(bear)],
        "reach guard: the copy's target was rewritten"
    );
    assert_eq!(declared, vec![TargetRef::Object(bear)], "CR 707.10d");
    assert_eq!(
        declared_of(runner.state(), growth),
        vec![TargetRef::Object(zada)]
    );
}
