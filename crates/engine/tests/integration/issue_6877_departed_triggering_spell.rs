//! Regression for issue #6877: a spell-cast "copy that spell" / self-cast
//! "copy this spell" trigger must copy the spell as it last existed on the
//! stack once an earlier trigger already moved it (bounced, countered) —
//! never a different stack object, and never nothing.
//!
//! https://github.com/phase-rs/phase/issues/6877

use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::{
    CastPaymentMode, GameState, StackEntryKind, SyntheticTriggerProvenance, WaitingFor,
};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use crate::support::shared_card_db;

// Krark's Partner line is omitted; it plays no part in a two-player game.
const KRARK: &str = "Whenever you cast an instant or sorcery spell, flip a coin. \
    If you lose the flip, return that spell to its owner's hand. \
    If you win the flip, copy that spell, and you may choose new targets for the copy.";

const DRAW_SPELL: &str = "Draw a card.";
const BOLT_SPELL: &str = "Lightning Bolt deals 3 damage to any target.";
const BRAIN_FREEZE: &str = "Target player mills three cards.\n\
    Storm (When you cast this spell, copy it for each spell cast before it this turn. \
    You may choose new targets for the copies.)";
const HESITATION: &str =
    "When a player casts a spell, sacrifice this enchantment and counter that spell.";
const SWARM_INTELLIGENCE: &str = "Whenever you cast an instant or sorcery spell, you may copy \
    that spell. You may choose new targets for the copy.";
const SAGE_OF_THE_SKIES: &str = "When you cast this spell, if you've cast another spell this \
    turn, copy this spell. (The copy becomes a token.)\nFlying, lifelink";
const UNSUBSTANTIATE: &str = "Return target spell or creature to its owner's hand.";

fn floating_mana(n: usize, color: ManaType) -> Vec<ManaUnit> {
    (0..n)
        .map(|_| ManaUnit::new(color, ObjectId(0), false, vec![]))
        .collect()
}

fn library_len(state: &GameState, player: PlayerId) -> usize {
    state
        .players
        .iter()
        .find(|p| p.id == player)
        .map(|p| p.library.len())
        .unwrap_or(0)
}

fn spells_cast_count(state: &GameState, player: PlayerId) -> usize {
    state
        .spells_cast_this_turn_by_player
        .get(&player)
        .map_or(0, |records| records.len())
}

fn spells_cast_count_named(state: &GameState, player: PlayerId, name: &str) -> usize {
    state
        .spells_cast_this_turn_by_player
        .get(&player)
        .map_or(0, |records| {
            records.iter().filter(|r| r.name == name).count()
        })
}

/// Drive `act()` through cast setup (targeting, mana payment, modal face
/// choice) until priority or an ordering prompt is reached.
fn commit_cast(runner: &mut GameRunner, spell: ObjectId, target: Option<TargetRef>) {
    let card_id = runner.state().objects[&spell].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: spell,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Auto,
        })
        .expect("cast must be accepted");
    for _ in 0..32 {
        if matches!(
            runner.state().waiting_for,
            WaitingFor::Priority { .. } | WaitingFor::OrderTriggers { .. }
        ) {
            return;
        }
        match runner.state().waiting_for.clone() {
            WaitingFor::TargetSelection { .. } => {
                runner
                    .act(GameAction::ChooseTarget {
                        target: target.clone(),
                    })
                    .expect("declared target must be accepted");
            }
            WaitingFor::ManaPayment { .. } => {
                runner
                    .act(GameAction::PassPriority)
                    .expect("pool-funded remainder must pay");
            }
            WaitingFor::ModalFaceChoice { .. } => {
                runner
                    .act(GameAction::ChooseModalFace { back_face: true })
                    .expect("modal face choice must be accepted");
            }
            other => panic!("unexpected waiting_for while committing cast: {other:?}"),
        }
    }
    panic!("cast did not reach Priority or OrderTriggers");
}

fn reseed(runner: &mut GameRunner, seed: u64) {
    runner.state_mut().rng = ChaCha20Rng::seed_from_u64(seed);
}

fn saw_coin(events: &[GameEvent], won: bool) -> bool {
    events
        .iter()
        .any(|event| matches!(event, GameEvent::CoinFlipped { won: w, .. } if *w == won))
}

fn saw_spell_copied(events: &[GameEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, GameEvent::SpellCopied { .. }))
}

fn saw_spell_countered(events: &[GameEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, GameEvent::SpellCountered { .. }))
}

/// Drain every `WaitingFor::OrderTriggers` prompt. `prefer_first`, if given,
/// is put last in submission order for its group — CR 603.3b: index 0 of
/// `order` selects the bottom of this controller's stack slot, so its last
/// entry resolves first (LIFO).
fn order_triggers(runner: &mut GameRunner, prefer_first: Option<ObjectId>) {
    let mut guard = 0;
    while let WaitingFor::OrderTriggers { triggers, .. } = runner.state().waiting_for.clone() {
        guard += 1;
        assert!(guard <= 16, "order_triggers: too many APNAP groups");
        let order: Vec<usize> = match prefer_first {
            Some(source_id) => {
                let mut order: Vec<usize> = (0..triggers.len())
                    .filter(|&i| triggers[i].source_id != source_id)
                    .collect();
                order.extend((0..triggers.len()).filter(|&i| triggers[i].source_id == source_id));
                order
            }
            None => (0..triggers.len()).collect(),
        };
        runner
            .act(GameAction::OrderTriggers { order })
            .expect("OrderTriggers must succeed");
    }
}

/// Options for [`drive`].
struct Drive {
    /// Put this source's trigger(s) on top of their APNAP group (resolve
    /// first) whenever an `OrderTriggers` prompt names them.
    prefer_first: Option<ObjectId>,
    /// Auto-submit `KeepAllCopyTargets` at a `CopyRetarget` pause instead of
    /// stopping the drive there.
    keep_retargets: bool,
    /// Auto-answer `OptionalEffectChoice` / `OptionalCostChoice` with this
    /// accept/decline decision instead of stopping the drive there.
    accept_optional: Option<bool>,
}

impl Drive {
    fn new() -> Self {
        Self {
            prefer_first: None,
            keep_retargets: false,
            accept_optional: None,
        }
    }

    fn prefer(mut self, source_id: ObjectId) -> Self {
        self.prefer_first = Some(source_id);
        self
    }

    fn keep_retargets(mut self) -> Self {
        self.keep_retargets = true;
        self
    }

    fn accept(mut self) -> Self {
        self.accept_optional = Some(true);
        self
    }

    fn decline(mut self) -> Self {
        self.accept_optional = Some(false);
        self
    }
}

/// Pass priority (draining ordering/optional/retarget prompts per `opts`)
/// until the stack shrinks below its length at call time, or an unhandled
/// prompt is reached. Returns the events collected along the way.
fn drive(runner: &mut GameRunner, opts: &Drive) -> Vec<GameEvent> {
    order_triggers(runner, opts.prefer_first);
    let initial_stack_len = runner.state().stack.len();
    let mut events = Vec::new();
    for _ in 0..32 {
        order_triggers(runner, opts.prefer_first);
        // Prompt handling is checked BEFORE the stack-shrink stop condition:
        // a resolving entry is popped from the stack before its effect chain
        // runs (`resolve_top`), so a mid-resolution pause (CopyRetarget,
        // OptionalEffectChoice) can already show a shrunk stack while still
        // needing an answer here.
        let waiting = runner.state().waiting_for.clone();
        match waiting {
            WaitingFor::CopyRetarget { .. } if opts.keep_retargets => {
                match runner.act(GameAction::KeepAllCopyTargets) {
                    Ok(result) => {
                        events.extend(result.events);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            WaitingFor::CopyRetarget { .. } => break,
            WaitingFor::OptionalEffectChoice { .. } | WaitingFor::OptionalCostChoice { .. }
                if opts.accept_optional.is_some() =>
            {
                let accept = opts.accept_optional.unwrap();
                let action = if matches!(waiting, WaitingFor::OptionalEffectChoice { .. }) {
                    GameAction::DecideOptionalEffect { accept }
                } else {
                    GameAction::DecideOptionalCost { pay: accept }
                };
                match runner.act(action) {
                    Ok(result) => {
                        events.extend(result.events);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            WaitingFor::OptionalEffectChoice { .. } | WaitingFor::OptionalCostChoice { .. } => {
                break
            }
            _ => {}
        }
        if runner.state().stack.len() < initial_stack_len {
            break;
        }
        match runner.act(GameAction::PassPriority) {
            Ok(result) => events.extend(result.events),
            Err(_) => break,
        }
    }
    events
}

fn setup_krarks_and_spell(
    krark_count: usize,
    seed: u64,
    spell_name: &str,
    spell_oracle: &str,
) -> (GameScenario, ObjectId) {
    let mut scenario = GameScenario::new_n_player(2, seed);
    scenario.at_phase(Phase::PreCombatMain);
    // CR 704.5j: non-legendary creatures so the legend rule does not collapse
    // the group.
    for i in 0..krark_count {
        scenario.add_creature_from_oracle(P0, &format!("Krark {i}"), 2, 2, KRARK);
    }
    for i in 0..8 {
        scenario.add_spell_to_library_top(P0, &format!("Library {i}"), true);
    }
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, spell_name, true, spell_oracle)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    (scenario, spell)
}

fn krark_id(runner: &GameRunner, name: &str) -> ObjectId {
    runner
        .state()
        .battlefield
        .iter()
        .copied()
        .find(|id| runner.state().objects[id].name == name)
        .unwrap_or_else(|| panic!("{name} must be on the battlefield"))
}

/// CONTROL: winning while the cast spell is still on the stack copies it —
/// establishes that the harness can observe a copy at all.
#[test]
fn winning_trigger_copies_spell_still_on_stack() {
    let (scenario, spell) = setup_krarks_and_spell(2, 42, "Draw Spell", DRAW_SPELL);
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);

    let lib_before = library_len(runner.state(), P0);
    reseed(&mut runner, 0);
    let win_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_coin(&win_events, true),
        "seed 0 must be a WIN for the first trigger: {win_events:?}"
    );
    assert!(
        saw_spell_copied(&win_events) && library_len(runner.state(), P0) < lib_before,
        "win while on stack must copy and the copy must resolve; events={win_events:?}"
    );
}

/// CR 608.2h: two Krark triggers, lose then win — the leftover winning
/// trigger still copies the spell as it last existed on the stack, even
/// though the earlier lose already bounced it to hand.
#[test]
fn winning_trigger_copies_spell_bounced_by_earlier_trigger() {
    let (scenario, spell) = setup_krarks_and_spell(2, 42, "Draw Spell", DRAW_SPELL);
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);
    order_triggers(&mut runner, None);
    assert_eq!(
        runner.state().stack.len(),
        3,
        "spell plus two Krark triggers must be on the stack: {:?}",
        runner.state().stack
    );

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_coin(&lose_events, false),
        "seed 1 must be a LOSE for the first trigger: {lose_events:?}"
    );
    assert_eq!(
        runner.state().objects[&spell].zone,
        Zone::Hand,
        "lose must bounce the original to hand"
    );

    let lib_before = library_len(runner.state(), P0);
    reseed(&mut runner, 0);
    let win_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_coin(&win_events, true),
        "seed 0 must be a WIN for the second trigger: {win_events:?}"
    );
    assert!(
        saw_spell_copied(&win_events) && library_len(runner.state(), P0) < lib_before,
        "win after bounce must create a copy that resolves (SpellCopied and a \
         resolved draw); events={win_events:?}"
    );
}

/// CR 707.10c: targeted instant, lose then win — the leftover win still
/// offers `CopyRetarget`, and the copy's default target is the original's.
#[test]
fn winning_trigger_offers_new_targets_for_copy_of_bounced_spell() {
    let (scenario, spell) = setup_krarks_and_spell(2, 42, "Lightning Bolt", BOLT_SPELL);
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, Some(TargetRef::Player(P1)));
    order_triggers(&mut runner, None);

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_coin(&lose_events, false),
        "seed 1 must be a LOSE for the first trigger: {lose_events:?}"
    );
    assert_eq!(runner.state().objects[&spell].zone, Zone::Hand);

    reseed(&mut runner, 0);
    let win_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_coin(&win_events, true),
        "seed 0 must be a WIN for the second trigger: {win_events:?}"
    );
    match &runner.state().waiting_for {
        WaitingFor::CopyRetarget { target_slots, .. } => {
            assert_eq!(
                target_slots[0].current,
                Some(TargetRef::Player(P1)),
                "the departed spell's own target must be the copy's default target"
            );
        }
        other => panic!("targeted leftover copy expected to halt on CopyRetarget, got {other:?}"),
    }
}

/// CR 608.2h: a spell countered before a winning Krark trigger resolves still
/// gets copied. Hesitation is P1's (non-active), so under CR 603.3b APNAP its
/// trigger is placed on the stack after Krark's own and resolves first.
#[test]
fn winning_trigger_copies_spell_countered_before_it_resolved() {
    let (mut scenario, spell) = setup_krarks_and_spell(2, 0, "Draw Spell", DRAW_SPELL);
    scenario.add_enchantment_from_oracle(P1, "Hesitation", HESITATION);
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);
    order_triggers(&mut runner, None);
    assert_eq!(
        runner.state().stack.len(),
        4,
        "spell plus two Krark triggers plus Hesitation's must be on the stack: {:?}",
        runner.state().stack
    );

    let counter_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_spell_countered(&counter_events),
        "Hesitation must counter first under APNAP: {counter_events:?}"
    );

    let lib_before = library_len(runner.state(), P0);
    reseed(&mut runner, 0);
    let win_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_coin(&win_events, true),
        "seed 0 must be a WIN for a remaining trigger: {win_events:?}"
    );
    assert!(
        saw_spell_copied(&win_events) && library_len(runner.state(), P0) < lib_before,
        "win after the spell was countered must still create a copy that resolves; \
         events={win_events:?}"
    );
}

/// CR 608.2h + CR 608.2d: Swarm Intelligence's optional copy of a
/// spell-cast trigger still offers (and creates) a copy after Hesitation
/// counters the spell first — accept branch.
#[test]
fn optional_copy_of_countered_spell_is_created_when_accepted() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_enchantment_from_oracle(P0, "Swarm Intelligence", SWARM_INTELLIGENCE);
    scenario.add_enchantment_from_oracle(P1, "Hesitation", HESITATION);
    for i in 0..4 {
        scenario.add_spell_to_library_top(P0, &format!("Library {i}"), true);
    }
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Draw Spell", true, DRAW_SPELL)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);
    order_triggers(&mut runner, None);

    let counter_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_spell_countered(&counter_events),
        "Hesitation must counter first under APNAP: {counter_events:?}"
    );

    let accept_events = drive(&mut runner, &Drive::new().accept().keep_retargets());
    assert!(
        saw_spell_copied(&accept_events),
        "accepting the optional copy of a countered spell must still create it: \
         {accept_events:?}"
    );
}

/// Companion negative: declining the same optional copy creates none.
#[test]
fn optional_copy_of_countered_spell_is_not_created_when_declined() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_enchantment_from_oracle(P0, "Swarm Intelligence", SWARM_INTELLIGENCE);
    scenario.add_enchantment_from_oracle(P1, "Hesitation", HESITATION);
    for i in 0..4 {
        scenario.add_spell_to_library_top(P0, &format!("Library {i}"), true);
    }
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Draw Spell", true, DRAW_SPELL)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);
    order_triggers(&mut runner, None);

    let counter_events = drive(&mut runner, &Drive::new());
    assert!(
        saw_spell_countered(&counter_events),
        "Hesitation must counter first under APNAP: {counter_events:?}"
    );

    // `drive` stops as soon as ANY entry leaves the stack (here: Hesitation's
    // own trigger, which also removed the countered spell) — Swarm's own
    // trigger has not yet started resolving. One more `drive` call advances
    // through the fresh priority round it needs before it reaches its own
    // `OptionalEffectChoice` pause.
    let _ = drive(&mut runner, &Drive::new());
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::OptionalEffectChoice { .. }
        ),
        "reach guard: the optional copy choice must actually be offered, got {:?}",
        runner.state().waiting_for
    );

    let decline_events = drive(&mut runner, &Drive::new().decline());
    assert!(
        !saw_spell_copied(&decline_events),
        "declining must create no copy: {decline_events:?}"
    );
}

/// CR 400.7 + CR 601.2i: the same card cast twice in one step, leaving the
/// stack each time with a different target — the OLDER trigger's copy must
/// carry the OLDER cast's target, never the newer recast's, even though both
/// casts share the same storage object id.
#[test]
fn older_trigger_copies_its_own_cast_not_a_later_recast() {
    let (scenario, bolt) = setup_krarks_and_spell(2, 0, "Lightning Bolt", BOLT_SPELL);
    let mut runner = scenario.build();

    // First cast: Bolt -> P1.
    commit_cast(&mut runner, bolt, Some(TargetRef::Player(P1)));
    order_triggers(&mut runner, None);
    assert_eq!(runner.state().stack.len(), 3);

    // Resolve one trigger with a LOSE — bounces the first Bolt to hand,
    // leaving its sibling ("old K1") waiting on the stack.
    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new());
    assert!(saw_coin(&lose_events, false), "{lose_events:?}");
    assert_eq!(runner.state().objects[&bolt].zone, Zone::Hand);
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only the sibling ('old K1') trigger should remain: {:?}",
        runner.state().stack
    );

    // Recast the same Bolt object -> P0, while the older trigger still waits.
    commit_cast(&mut runner, bolt, Some(TargetRef::Player(P0)));
    order_triggers(&mut runner, None);
    assert_eq!(runner.state().stack.len(), 4);
    assert_eq!(
        spells_cast_count(runner.state(), P0),
        2,
        "reach guard: two SpellCast events for the one object id"
    );

    // New K2 loses -> bounces the second Bolt to hand.
    reseed(&mut runner, 1);
    let new_lose_events = drive(&mut runner, &Drive::new());
    assert!(saw_coin(&new_lose_events, false), "{new_lose_events:?}");
    assert_eq!(runner.state().objects[&bolt].zone, Zone::Hand);

    // New K1 wins -> copies the SECOND cast (keeps its P0 target); drive it
    // fully through so only the old trigger remains.
    reseed(&mut runner, 0);
    let new_win_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(saw_coin(&new_win_events, true), "{new_win_events:?}");
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only the old trigger should remain after the recast's pair resolves: {:?}",
        runner.state().stack
    );

    // Finally the OLDEST trigger wins. It must copy the FIRST cast's record
    // (target P1) — never the highest (most recent) departed record, which
    // would be the second cast's (target P0).
    reseed(&mut runner, 0);
    let old_win_events = drive(&mut runner, &Drive::new());
    assert!(saw_coin(&old_win_events, true), "{old_win_events:?}");
    match &runner.state().waiting_for {
        WaitingFor::CopyRetarget { target_slots, .. } => {
            assert_eq!(
                target_slots[0].current,
                Some(TargetRef::Player(P1)),
                "the OLDER trigger's pin must read the OLDER cast's departed record, \
                 not the highest (most recent) one"
            );
        }
        other => panic!("expected CopyRetarget for the old trigger's win, got {other:?}"),
    }
}

/// CR 702.40a + CR 113.7a + CR 400.7: the same card cast twice in one step
/// while its own first Storm trigger still waits — the OLDER Storm trigger's
/// copy must carry the OLDER cast's target, never the newer recast's, even
/// though both casts share the same storage object id.
#[test]
fn older_storm_trigger_copies_its_own_cast_not_a_later_recast() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Krark 0", 2, 2, KRARK);
    for i in 0..8 {
        scenario.add_spell_to_library_top(P0, &format!("Library P0 {i}"), true);
    }
    for i in 0..8 {
        scenario.add_spell_to_library_top(P1, &format!("Library P1 {i}"), true);
    }
    // A creature spell, not instant/sorcery, so Krark's own trigger does not
    // fire on it — only its cast counts toward Storm's copy count.
    let filler_spell = scenario
        .add_creature_to_hand(P0, "Filler Creature", 1, 1)
        .id();
    let brain_freeze = scenario
        .add_spell_to_hand(P0, "Brain Freeze", true)
        .from_oracle_text_with_keywords(&["Storm"], BRAIN_FREEZE)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();
    let krark = krark_id(&runner, "Krark 0");

    commit_cast(&mut runner, filler_spell, None);
    let _ = drive(&mut runner, &Drive::new());

    // First cast: Brain Freeze -> P1. Krark preferred to resolve first, so its
    // own (first) Storm trigger is left waiting below it.
    commit_cast(&mut runner, brain_freeze, Some(TargetRef::Player(P1)));
    order_triggers(&mut runner, Some(krark));
    assert_eq!(
        runner.state().stack.len(),
        3,
        "spell plus Krark's trigger plus its own (first) Storm trigger: {:?}",
        runner.state().stack
    );

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new().prefer(krark));
    assert!(saw_coin(&lose_events, false), "{lose_events:?}");
    assert_eq!(
        runner.state().objects[&brain_freeze].zone,
        Zone::Hand,
        "Krark's lose must bounce the first Brain Freeze cast to hand"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only the first ('old') Storm trigger should remain: {:?}",
        runner.state().stack
    );

    // Recast the same Brain Freeze object -> P0, while the older Storm
    // trigger still waits.
    commit_cast(&mut runner, brain_freeze, Some(TargetRef::Player(P0)));
    order_triggers(&mut runner, Some(krark));
    assert_eq!(
        runner.state().stack.len(),
        4,
        "recast plus Krark's new trigger plus a new Storm trigger, above the \
         older waiting Storm trigger: {:?}",
        runner.state().stack
    );
    assert_eq!(
        spells_cast_count_named(runner.state(), P0, "Brain Freeze"),
        2,
        "reach guard: two SpellCast events for the one object id"
    );

    reseed(&mut runner, 1);
    let new_lose_events = drive(&mut runner, &Drive::new().prefer(krark));
    assert!(saw_coin(&new_lose_events, false), "{new_lose_events:?}");
    assert_eq!(
        runner.state().objects[&brain_freeze].zone,
        Zone::Hand,
        "Krark's lose must bounce the second Brain Freeze cast to hand"
    );
    assert_eq!(
        runner.state().stack.len(),
        2,
        "the new Storm trigger and the older Storm trigger remain: {:?}",
        runner.state().stack
    );

    // Reach guard: both remaining entries are Storm-provenance triggers
    // sharing the one Brain Freeze object id, not e.g. leftover Krark
    // triggers or a TriggeringSource shape.
    let storm_entries = runner
        .state()
        .stack
        .iter()
        .filter(|entry| {
            entry.source_id == brain_freeze
                && matches!(
                    &entry.kind,
                    StackEntryKind::TriggeredAbility {
                        provenance: Some(SyntheticTriggerProvenance::Storm { .. }),
                        ..
                    }
                )
        })
        .count();
    assert_eq!(
        storm_entries,
        2,
        "reach guard: two Storm-provenance triggers must be waiting: {:?}",
        runner.state().stack
    );

    // The newer Storm trigger is on top and resolves first; keep its target.
    let new_storm_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_spell_copied(&new_storm_events),
        "the newer Storm trigger must copy its own (newer) cast: {new_storm_events:?}"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only the OLDER Storm trigger should remain: {:?}",
        runner.state().stack
    );

    // Finally the OLDER Storm trigger resolves. It must copy the FIRST
    // cast's record (target P1) — never the highest (most recent) departed
    // record, which would be the second cast's (target P0).
    let lib_before_p1 = library_len(runner.state(), P1);
    let _ = drive(&mut runner, &Drive::new());
    match &runner.state().waiting_for {
        WaitingFor::CopyRetarget { target_slots, .. } => {
            assert_eq!(
                target_slots[0].current,
                Some(TargetRef::Player(P1)),
                "the OLDER Storm trigger's pin must read the OLDER cast's \
                 departed record, not the highest (most recent) one"
            );
        }
        other => {
            panic!("expected CopyRetarget for the older Storm trigger's resolution, got {other:?}")
        }
    }
    runner
        .act(GameAction::KeepAllCopyTargets)
        .expect("KeepAllCopyTargets must succeed for the older Storm trigger's copy");
    let _ = drive(&mut runner, &Drive::new());
    assert_eq!(
        library_len(runner.state(), P1),
        lib_before_p1.saturating_sub(3),
        "the OLDER Storm copy must have resolved against the older cast's \
         target (P1), milling 3 as Brain Freeze's own effect does"
    );
}

/// CR 712.8a: a spell cast as a modal back face, then departed (bounced by an
/// earlier trigger) — the copy must be of the back face, not the front.
#[test]
fn copy_of_bounced_modal_back_face_spell_is_the_back_face() {
    let Some(db) = shared_card_db() else {
        eprintln!("skipping: no card database available");
        return;
    };
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Krark 0", 2, 2, KRARK);
    scenario.add_creature_from_oracle(P0, "Krark 1", 2, 2, KRARK);
    let card = scenario.add_real_card(P0, "Flamescroll Celebrant", Zone::Hand, db);
    let mut white = floating_mana(4, ManaType::White);
    white.extend(floating_mana(4, ManaType::Colorless));
    scenario.with_mana_pool(P0, white);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    assert_eq!(
        runner.state().objects[&card].name,
        "Flamescroll Celebrant",
        "reach guard: the card in hand is named for its front face"
    );

    commit_cast(&mut runner, card, None);
    order_triggers(&mut runner, None);
    assert_eq!(runner.state().stack.len(), 3);

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new());
    assert!(saw_coin(&lose_events, false), "{lose_events:?}");
    assert_eq!(runner.state().objects[&card].zone, Zone::Hand);

    reseed(&mut runner, 0);
    // A plain `drive` would run the untargeted copy all the way through its
    // own resolution (Revel in Silence has no targets, so nothing pauses it),
    // and the spell-copy token ceases to exist once resolved — reading its
    // name from `state.objects` after that is a stale lookup. Stop capturing
    // as soon as `SpellCopied` itself appears, while the copy is still live.
    order_triggers(&mut runner, None);
    let mut win_events = Vec::new();
    let mut copy_name = None;
    for _ in 0..8 {
        match runner.act(GameAction::PassPriority) {
            Ok(result) => win_events.extend(result.events),
            Err(_) => break,
        }
        if let Some(id) = win_events.iter().find_map(|event| match event {
            GameEvent::SpellCopied { object_id, .. } => Some(*object_id),
            _ => None,
        }) {
            copy_name = runner.state().objects.get(&id).map(|obj| obj.name.clone());
            break;
        }
    }
    assert!(saw_coin(&win_events, true), "{win_events:?}");
    assert_eq!(
        copy_name.as_deref(),
        Some("Revel in Silence"),
        "the copy must be the back face the departed spell was cast as: {win_events:?}"
    );
}

/// CR 400.7 + CR 500.2: a departed-spell record is cleared once the step in
/// which the spell left ends — it is not readable in a later step.
#[test]
fn departed_spell_record_is_cleared_when_the_step_ends() {
    let (scenario, spell) = setup_krarks_and_spell(1, 42, "Draw Spell", DRAW_SPELL);
    let mut runner = scenario.build();
    commit_cast(&mut runner, spell, None);
    order_triggers(&mut runner, None);

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new());
    assert!(saw_coin(&lose_events, false), "{lose_events:?}");
    assert_eq!(runner.state().objects[&spell].zone, Zone::Hand);
    assert!(
        runner.state().stack.is_empty(),
        "reach guard: the stack must be empty before the step ends"
    );
    assert!(
        !runner.state().departed_stack_spells.is_empty(),
        "reach guard: a departed record must exist before the step ends"
    );

    let phase_before = runner.state().phase;
    runner.pass_both_players();
    assert_ne!(
        runner.state().phase,
        phase_before,
        "reach guard: passing with an empty stack must advance the step/phase"
    );
    assert!(
        runner.state().departed_stack_spells.is_empty(),
        "the departed record must not survive the step transition"
    );
}

/// CR 702.40a control: a spell with Storm stays on the stack -> its copies
/// are created.
#[test]
fn storm_copies_spell_still_on_stack() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature(P0, "Filler Creature", 1, 1);
    scenario.add_spell_to_library_top(P0, "Library P0", true);
    for i in 0..4 {
        scenario.add_spell_to_library_top(P1, &format!("Library P1 {i}"), true);
    }
    let filler_spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Filler Spell", true, DRAW_SPELL)
        .id();
    let brain_freeze = scenario
        .add_spell_to_hand(P0, "Brain Freeze", true)
        .from_oracle_text_with_keywords(&["Storm"], BRAIN_FREEZE)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();

    commit_cast(&mut runner, filler_spell, None);
    let _ = drive(&mut runner, &Drive::new());

    commit_cast(&mut runner, brain_freeze, Some(TargetRef::Player(P1)));
    order_triggers(&mut runner, None);
    assert_eq!(
        runner.state().stack.len(),
        2,
        "Brain Freeze plus its own Storm trigger: {:?}",
        runner.state().stack
    );

    let lib_before = library_len(runner.state(), P1);
    let storm_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_spell_copied(&storm_events),
        "Storm's copy must still fire while the spell is on the stack: {storm_events:?}"
    );
    assert_eq!(
        library_len(runner.state(), P1),
        lib_before.saturating_sub(3),
        "the Storm copy must have resolved (mill 3)"
    );
}

/// CR 702.40a + CR 113.7a: a spell with Storm is returned to hand (by an
/// unrelated Krark trigger) before its own Storm trigger resolves — the
/// Storm copies are still created.
#[test]
fn storm_copies_spell_returned_to_hand_before_storm_resolves() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Krark 0", 2, 2, KRARK);
    for i in 0..4 {
        scenario.add_spell_to_library_top(P1, &format!("Library P1 {i}"), true);
    }
    // The filler ("a spell cast before it this turn") is itself a creature
    // spell, not an instant/sorcery — Krark's own trigger only cares about
    // Storm's later Brain Freeze cast, not this one.
    let filler_spell = scenario
        .add_creature_to_hand(P0, "Filler Creature", 1, 1)
        .id();
    let brain_freeze = scenario
        .add_spell_to_hand(P0, "Brain Freeze", true)
        .from_oracle_text_with_keywords(&["Storm"], BRAIN_FREEZE)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();

    commit_cast(&mut runner, filler_spell, None);
    let _ = drive(&mut runner, &Drive::new());

    let krark = krark_id(&runner, "Krark 0");
    commit_cast(&mut runner, brain_freeze, Some(TargetRef::Player(P1)));
    // Order with Krark preferred to the top now, before any prompt is
    // answered — a later `.prefer(krark)` on `drive` has no effect once the
    // `OrderTriggers` prompt has already been answered.
    order_triggers(&mut runner, Some(krark));
    assert_eq!(
        runner.state().stack.len(),
        3,
        "Brain Freeze plus Krark's trigger plus its own Storm trigger: {:?}",
        runner.state().stack
    );

    reseed(&mut runner, 1);
    let lose_events = drive(&mut runner, &Drive::new().prefer(krark));
    assert!(
        saw_coin(&lose_events, false),
        "Krark must resolve first and lose: {lose_events:?}"
    );
    assert_eq!(
        runner.state().objects[&brain_freeze].zone,
        Zone::Hand,
        "Krark's lose must bounce Brain Freeze before Storm resolves"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only Storm's trigger remains"
    );

    let lib_before = library_len(runner.state(), P1);
    let storm_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_spell_copied(&storm_events),
        "Storm's own trigger must still copy the departed spell: {storm_events:?}"
    );
    assert_eq!(
        library_len(runner.state(), P1),
        lib_before.saturating_sub(3),
        "the Storm copy must have resolved against the departed spell's target"
    );
}

/// CR 702.40a class: a self-cast copy trigger (Sage of the Skies, not Storm)
/// still copies its own spell after that spell is returned to hand before
/// the trigger resolves.
#[test]
fn self_cast_copy_trigger_copies_spell_returned_to_hand() {
    let mut scenario = GameScenario::new_n_player(2, 0);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_spell_to_library_top(P0, "Library P0", true);
    let filler_spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Filler Spell", true, DRAW_SPELL)
        .id();
    let sage = scenario
        .add_creature_to_hand_from_oracle(P0, "Sage of the Skies", 2, 1, SAGE_OF_THE_SKIES)
        .id();
    let unsubstantiate = scenario
        .add_spell_to_hand_from_oracle(P0, "Unsubstantiate", true, UNSUBSTANTIATE)
        .id();
    scenario.with_mana_pool(P0, floating_mana(10, ManaType::Colorless));
    let mut runner = scenario.build();

    commit_cast(&mut runner, filler_spell, None);
    let _ = drive(&mut runner, &Drive::new());

    commit_cast(&mut runner, sage, None);
    order_triggers(&mut runner, None);
    assert_eq!(
        runner.state().stack.len(),
        2,
        "Sage plus its own self-cast copy trigger: {:?}",
        runner.state().stack
    );

    // While Sage's trigger waits, bounce Sage itself off the stack.
    commit_cast(&mut runner, unsubstantiate, Some(TargetRef::Object(sage)));
    order_triggers(&mut runner, None);
    let bounce_events = drive(&mut runner, &Drive::new());
    assert_eq!(
        runner.state().objects[&sage].zone,
        Zone::Hand,
        "Unsubstantiate must bounce Sage before its own trigger resolves: {bounce_events:?}"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "only Sage's own trigger should remain: {:?}",
        runner.state().stack
    );

    let copy_events = drive(&mut runner, &Drive::new().keep_retargets());
    assert!(
        saw_spell_copied(&copy_events),
        "Sage's self-cast trigger must still copy the departed spell: {copy_events:?}"
    );
    let token_on_battlefield = runner.state().battlefield.iter().any(|id| {
        let obj = &runner.state().objects[id];
        obj.is_token && obj.name == "Sage of the Skies"
    });
    assert!(
        token_on_battlefield,
        "the copy of a permanent spell must resolve onto the battlefield as a token \
         (CR 608.3f): battlefield={:?}",
        runner.state().battlefield
    );
}
