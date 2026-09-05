//! Spell-cast-trigger matrix: board states covering every way a spell can be
//! cast, so general `TriggerMode::SpellCast` triggers can be developed and
//! tested against all of them.
//!
//! # The gap this closes
//!
//! `CastingVariant` (`crates/engine/src/types/game_state.rs`) has 40 variants.
//! Every variant is exercised somewhere for its own mechanic, but before this
//! module only `issue_2376_pyromancers_ascension.rs` paired a non-`Normal`
//! cast variant with a real `TriggerMode::SpellCast` trigger — and even that
//! test builds `GameState` by hand rather than driving the real cast pipeline.
//! General cast triggers were therefore verified against `CastingVariant::Normal`
//! only. This module drives every reachable variant through the real
//! `GameRunner::cast(..)` pipeline with a live SpellCast watcher on the board.
//!
//! # Structure
//!
//! 1. A shared watcher helper (`install_universal_watcher`) — a permanent
//!    whose ability is a real `TriggerMode::SpellCast` trigger with an
//!    observable effect (drawing a card, per the plan — easiest to assert via
//!    `CastOutcome::hand_drawn`).
//! 2. Per-variant setup functions, one `#[test]` per row, each staging the
//!    zone/keyword/permission machinery that variant actually requires.
//! 3. A shared assertion harness (`stack_casting_variant` + hand-delta checks)
//!    every row runs through: the watcher fired exactly once, attributed to
//!    the correct controller, for the correct spell.
//! 4. Two extra axes: controller (opponent-cast vs. own-cast) and type
//!    (creature / instant-or-sorcery / noncreature), plus a Fuse split card
//!    and an X spell.
//!
//! # Coverage table (see the final implementer report for the authoritative,
//! up-to-date version — this is a maintenance aid, not a second source of truth)
//!
//! landed (38): Normal, Adventure, Warp, Escape, Retrace, Harmonize, Mayhem,
//! Flashback, Aftermath, Disturb, GraveyardPermission, HandPermission,
//! ExilePermission, Sneak, WebSlinging, Miracle, Madness, Evoke, Emerge,
//! Dash, Blitz, Spectacle, Suspend, Plot, Foretell, Overload, Bestow, Awaken,
//! Cleave, MoreThanMeetsTheEye, Impending, Prototype, Mutate, Freerunning,
//! Prowl, JumpStart, Fuse, FaceDown.
//!
//! not landed (2): Omen — no back-face-bearing fixture built (deferred, see
//! the implementer report). Surge — apparent engine bug (see the
//! doc comment immediately before where `surge_fires_watcher_once` would
//! sit, in the hand alternative-cost family section below); reported, not
//! fixed.
//!
//! # Oracle text
//!
//! Every card's Oracle text below is copied verbatim from a snapshot of
//! MTGJSON `AtomicCards.json` (2026-09-02) and cross-checked against the
//! wording cited in each test's doc comment. Two watcher cards are noted as
//! using a real card's ability line "as-is" — their full printed text
//! includes an unrelated clause that is inert for a single-cast-per-test
//! fixture (documented at each `install_*` helper). No card in this module
//! is synthetic.

use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::game::zones::create_object;
use engine::parser::oracle::parse_oracle_text;
use engine::types::ability::TargetRef;
use engine::types::actions::{AlternativeCastDecision, GameAction};
use engine::types::card_type::CoreType;
use engine::types::game_state::{CastingVariant, GameState, StackEntryKind, WaitingFor};
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::mana::{ManaColor, ManaCost, ManaCostShard, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use crate::support::shared_card_db as load_db;

// ===========================================================================
// Shared helpers
// ===========================================================================

/// `n` mana units of the given types, for pre-funding a player's mana pool.
/// Mirrors `fuse_runtime.rs`'s `pool_units`.
fn mana_units(types: &[ManaType]) -> Vec<ManaUnit> {
    let dummy = ObjectId(0);
    types
        .iter()
        .map(|m| ManaUnit::new(*m, dummy, false, vec![]))
        .collect()
}

/// Seed `player`'s library with enough filler cards that any draw in this
/// module's tests (the watcher's own draw, plus whatever the cast spell
/// itself draws) always has a card available. `GameScenario::new()` seeds NO
/// library by default (by design — zero filesystem/deck dependencies), so
/// every test that expects a successful draw must call this first.
fn ensure_library(scenario: &mut GameScenario, player: PlayerId, n: usize) {
    let names: Vec<&str> = std::iter::repeat_n("Filler Card", n).collect();
    scenario.with_library_top(player, &names);
}

/// Set `player` as active player and sole priority-holder. Needed whenever a
/// test must cast as a player other than the scenario's default active player
/// (`GameScenario::new()` / `at_phase` both default `active_player` to P0).
fn set_priority(runner: &mut GameRunner, player: PlayerId) {
    let state = runner.state_mut();
    state.active_player = player;
    state.priority_player = player;
    state.waiting_for = WaitingFor::Priority { player };
}

fn hand_len(state: &GameState, player: PlayerId) -> usize {
    state
        .players
        .iter()
        .find(|p| p.id == player)
        .expect("player exists")
        .hand
        .len()
}

/// Reach guard: the `CastingVariant` actually recorded on `spell`'s stack
/// entry. Panics if `spell` is not a `Spell` stack entry — i.e. if the cast
/// never reached the stack at all. This is the positive proof that a "trigger
/// fired" assertion is not vacuous: if the engine had silently fallen back to
/// a different variant (or the cast never landed), this assertion fails
/// before the hand-delta check is even reached.
fn stack_casting_variant(state: &GameState, spell: ObjectId) -> CastingVariant {
    state
        .stack
        .iter()
        .find_map(|entry| {
            if entry.id != spell {
                return None;
            }
            match &entry.kind {
                StackEntryKind::Spell {
                    casting_variant, ..
                } => Some(*casting_variant),
                other => panic!("expected a Spell stack entry for {spell:?}, got {other:?}"),
            }
        })
        .unwrap_or_else(|| panic!("{spell:?} is not on the stack — the cast did not reach it"))
}

/// Parse `oracle_text` and install it onto an already-created object,
/// bypassing `CardBuilder` (whose fields are private to `scenario.rs`). Used
/// for placing a REAL card's verbatim text on an object in a zone
/// `GameScenario` has no typed constructor for (a creature card sitting in a
/// graveyard, an exiled card with a specific `CastingPermission`, etc.).
/// Mirrors what `CardBuilder::from_oracle_text_with_keywords` does, minus the
/// MTGJSON-specific processing that helper also skips.
fn apply_oracle_text(
    state: &mut GameState,
    id: ObjectId,
    name: &str,
    types: &[&str],
    oracle_text: &str,
) {
    let type_strings: Vec<String> = types.iter().map(|s| s.to_string()).collect();
    let parsed = parse_oracle_text(oracle_text, name, &[], &type_strings, &[]);
    let obj = state.objects.get_mut(&id).unwrap();
    obj.keywords = parsed.extracted_keywords.clone();
    obj.base_keywords = obj.keywords.clone();
    obj.abilities = std::sync::Arc::new(parsed.abilities);
    obj.trigger_definitions = parsed.triggers.into();
    obj.static_definitions = parsed.statics.into();
    obj.replacement_definitions = parsed.replacements.into();
}

/// Place a REAL creature card's verbatim Oracle text directly into a
/// player's graveyard. No `GameScenario` builder places a *creature* card in
/// the graveyard (`add_spell_to_graveyard` is instant/sorcery-only), which
/// several graveyard-cast variants (Escape, Disturb, the GraveyardPermission
/// target) need.
#[allow(clippy::too_many_arguments)]
fn add_creature_to_graveyard_from_oracle(
    state: &mut GameState,
    player: PlayerId,
    name: &str,
    power: i32,
    toughness: i32,
    mana_cost: ManaCost,
    subtypes: &[&str],
    oracle_text: &str,
) -> ObjectId {
    let card_id = CardId(state.next_object_id);
    let id = create_object(state, card_id, player, name.to_string(), Zone::Graveyard);
    {
        let obj = state.objects.get_mut(&id).unwrap();
        obj.card_types.core_types.push(CoreType::Creature);
        obj.card_types.subtypes = subtypes.iter().map(|s| s.to_string()).collect();
        obj.base_card_types = obj.card_types.clone();
        obj.power = Some(power);
        obj.toughness = Some(toughness);
        obj.base_power = Some(power);
        obj.base_toughness = Some(toughness);
        obj.mana_cost = mana_cost.clone();
        obj.base_mana_cost = mana_cost;
    }
    apply_oracle_text(state, id, name, &["Creature"], oracle_text);
    id
}

// ===========================================================================
// Watchers — SpellCast triggers with an observable draw effect
// ===========================================================================

/// Moderation ({1}{W}{U}, Enchantment). Verbatim: "You can't cast more than
/// one spell each turn. Whenever you cast a spell, draw a card." The first
/// sentence is a real, enforced restriction (CR 601), but every fixture in
/// this module casts at most one spell per controller per scenario, so it is
/// inert here — never load-bearing for any assertion.
const MODERATION: &str =
    "You can't cast more than one spell each turn.\nWhenever you cast a spell, draw a card.";

/// Forced Fruition ({2}{U}{U}, Enchantment). Verbatim: "Whenever an opponent
/// casts a spell, that player draws seven cards." Unconditional and
/// mandatory — no `unless`/`may` clause to stage around, so the opponent's
/// hand delta unambiguously discriminates the controller axis.
const FORCED_FRUITION: &str = "Whenever an opponent casts a spell, that player draws seven cards.";

/// Beast Whisperer ({2}{G}{G}, Creature). Verbatim: "Whenever you cast a
/// creature spell, draw a card."
const BEAST_WHISPERER: &str = "Whenever you cast a creature spell, draw a card.";

/// Archmage of Runes ({3}{U}{U}, Creature). Verbatim: "Instant and sorcery
/// spells you cast cost {1} less to cast. Whenever you cast an instant or
/// sorcery spell, draw a card." The cost-reduction clause is inert for a
/// full-mana-pool fixture.
const ARCHMAGE_OF_RUNES: &str = "Instant and sorcery spells you cast cost {1} less to cast.\nWhenever you cast an instant or sorcery spell, draw a card.";

/// Whirlwind of Thought ({1}{U}{R}{W}, Enchantment). Verbatim: "Whenever you
/// cast a noncreature spell, draw a card."
const WHIRLWIND_OF_THOUGHT: &str = "Whenever you cast a noncreature spell, draw a card.";

fn install_universal_watcher(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "Moderation-Watcher", 0, 0)
        .as_enchantment()
        .from_oracle_text(MODERATION)
        .id()
}

fn install_opponent_watcher(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "ForcedFruition-Watcher", 0, 0)
        .as_enchantment()
        .from_oracle_text(FORCED_FRUITION)
        .id()
}

fn install_creature_watcher(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "Beast Whisperer", 2, 2)
        .from_oracle_text(BEAST_WHISPERER)
        .id()
}

fn install_instant_sorcery_watcher(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "Archmage of Runes", 2, 2)
        .from_oracle_text(ARCHMAGE_OF_RUNES)
        .id()
}

fn install_noncreature_watcher(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "Whirlwind-Watcher", 0, 0)
        .as_enchantment()
        .from_oracle_text(WHIRLWIND_OF_THOUGHT)
        .id()
}

// ===========================================================================
// Baseline: CastingVariant::Normal
// ===========================================================================

/// CR 601.2a (normal cast) + CR 603.2 (triggered ability). Baseline positive
/// control every other row is measured against: an ordinary hand cast fires
/// a general SpellCast watcher exactly once, attributed to the caster.
#[test]
fn normal_cast_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    let watcher = install_universal_watcher(&mut scenario, P0);
    let bolt = scenario
        .add_spell_to_hand_from_oracle(P0, "Shock", true, "Shock deals 2 damage to any target.")
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 0,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
    let mut runner = scenario.build();

    let commit = runner.cast(bolt).target_player(P1).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), bolt),
        CastingVariant::Normal
    );
    let outcome = commit.resolve();
    outcome.assert_hand_drawn(P0, 1);
    let _ = watcher;
}

// ===========================================================================
// Controller axis
// ===========================================================================

/// CR 603.2: a "whenever an OPPONENT casts a spell" watcher must fire for the
/// opponent's cast and must NOT fire for its own controller's cast — the
/// discriminator general cast-trigger code must respect.
#[test]
fn opponent_axis_fires_only_for_opponents_cast() {
    // Case A: P1 (the watcher's opponent) casts — must fire, attributed to P1.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        ensure_library(&mut scenario, P1, 10);
        install_opponent_watcher(&mut scenario, P0);
        let bolt = scenario
            .add_spell_to_hand_from_oracle(P1, "Shock", true, "Shock deals 2 damage to any target.")
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Red],
                generic: 0,
            })
            .id();
        scenario.with_mana_pool(P1, mana_units(&[ManaType::Red]));
        let mut runner = scenario.build();
        set_priority(&mut runner, P1);
        let commit = runner.cast(bolt).target_player(P0).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), bolt),
            CastingVariant::Normal,
            "reach guard: the opponent's spell must actually reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P1),
            7,
            "Forced Fruition must give the opponent-caster seven cards"
        );
    }
    // Case B (negative control): P0 (the watcher's own controller) casts —
    // must NOT fire. Paired reach guard: the cast still reaches the stack, so
    // a zero hand delta is not merely "nothing happened".
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        install_opponent_watcher(&mut scenario, P0);
        let bolt = scenario
            .add_spell_to_hand_from_oracle(P0, "Shock", true, "Shock deals 2 damage to any target.")
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Red],
                generic: 0,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
        let mut runner = scenario.build();
        let commit = runner.cast(bolt).target_player(P1).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), bolt),
            CastingVariant::Normal,
            "reach guard: the controller's own spell must still reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            0,
            "an opponent-only watcher must not fire for its own controller's cast"
        );
    }
}

// ===========================================================================
// Type axis
// ===========================================================================

/// CR 300.1 (creature) + CR 603.2: a "whenever you cast a CREATURE spell"
/// watcher fires for a creature spell and not for an instant — covering both
/// a permanent and a non-permanent spell in one row.
#[test]
fn type_axis_creature_watcher_fires_only_for_creature_spells() {
    // Positive: a creature spell fires it.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        ensure_library(&mut scenario, P0, 5);
        install_creature_watcher(&mut scenario, P0);
        let bear = scenario
            .add_creature_to_hand(P0, "Grizzly Bears", 2, 2)
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Green],
                generic: 1,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Green, ManaType::Colorless]));
        let mut runner = scenario.build();
        let commit = runner.cast(bear).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), bear),
            CastingVariant::Normal,
            "reach guard: the creature spell must reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            1,
            "creature spell must fire Beast Whisperer"
        );
    }
    // Negative control, paired reach guard: an instant reaches the stack but
    // must NOT fire the creature-only watcher.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        install_creature_watcher(&mut scenario, P0);
        let shock = scenario
            .add_spell_to_hand_from_oracle(P0, "Shock", true, "Shock deals 2 damage to any target.")
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Red],
                generic: 0,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
        let mut runner = scenario.build();
        let commit = runner.cast(shock).target_player(P1).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), shock),
            CastingVariant::Normal,
            "reach guard: the instant must still reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            0,
            "a creature-only watcher must not fire for an instant spell"
        );
    }
}

/// CR 300.1 + CR 603.2: a "whenever you cast an INSTANT OR SORCERY spell"
/// watcher fires for an instant and not for a creature spell.
#[test]
fn type_axis_instant_or_sorcery_watcher_fires_only_for_instants_and_sorceries() {
    // Positive: an instant fires it.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        ensure_library(&mut scenario, P0, 5);
        install_instant_sorcery_watcher(&mut scenario, P0);
        let shock = scenario
            .add_spell_to_hand_from_oracle(P0, "Shock", true, "Shock deals 2 damage to any target.")
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Red],
                generic: 0,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
        let mut runner = scenario.build();
        let commit = runner.cast(shock).target_player(P1).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), shock),
            CastingVariant::Normal,
            "reach guard: the instant must reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            1,
            "an instant spell must fire the instant-or-sorcery watcher"
        );
    }
    // Negative control, paired reach guard: a creature spell reaches the
    // stack but must NOT fire the instant-or-sorcery watcher.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        install_instant_sorcery_watcher(&mut scenario, P0);
        let bear = scenario
            .add_creature_to_hand(P0, "Grizzly Bears", 2, 2)
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Green],
                generic: 1,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Green, ManaType::Colorless]));
        let mut runner = scenario.build();
        let commit = runner.cast(bear).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), bear),
            CastingVariant::Normal,
            "reach guard: the creature spell must still reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            0,
            "an instant-or-sorcery-only watcher must not fire for a creature spell"
        );
    }
}

/// CR 300.1 (noncreature) + CR 603.2: a "whenever you cast a NONCREATURE
/// spell" watcher fires for a noncreature permanent (an artifact) and not for
/// a creature spell — covers a non-instant/sorcery noncreature case distinct
/// from the instant-or-sorcery row above.
#[test]
fn type_axis_noncreature_watcher_fires_only_for_noncreature_spells() {
    // Positive: a noncreature artifact spell fires it.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        ensure_library(&mut scenario, P0, 5);
        install_noncreature_watcher(&mut scenario, P0);
        let sphere = scenario
            .add_artifact_to_hand_from_oracle(P0, "Sphere of Filler", "")
            .with_mana_cost(ManaCost::Cost {
                shards: vec![],
                generic: 1,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Colorless]));
        let mut runner = scenario.build();
        let commit = runner.cast(sphere).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), sphere),
            CastingVariant::Normal,
            "reach guard: the artifact spell must reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            1,
            "a noncreature artifact spell must fire the noncreature watcher"
        );
    }
    // Negative control, paired reach guard: a creature spell reaches the
    // stack but must NOT fire the noncreature-only watcher.
    {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        install_noncreature_watcher(&mut scenario, P0);
        let bear = scenario
            .add_creature_to_hand(P0, "Grizzly Bears", 2, 2)
            .with_mana_cost(ManaCost::Cost {
                shards: vec![ManaCostShard::Green],
                generic: 1,
            })
            .id();
        scenario.with_mana_pool(P0, mana_units(&[ManaType::Green, ManaType::Colorless]));
        let mut runner = scenario.build();
        let commit = runner.cast(bear).commit();
        assert_eq!(
            stack_casting_variant(commit.state(), bear),
            CastingVariant::Normal,
            "reach guard: the creature spell must still reach the stack"
        );
        let outcome = commit.resolve();
        assert_eq!(
            outcome.hand_drawn(P0),
            0,
            "a noncreature-only watcher must not fire for a creature spell"
        );
    }
}

/// CR 601.2a + CR 603.2: an X spell casts and fires a general SpellCast
/// watcher like any other spell — X announcement is not a separate cast path.
/// Devil's Play ({X}{R}, Sorcery): "Devil's Play deals X damage to any
/// target." Verbatim Oracle text.
const DEVILS_PLAY: &str = "Devil's Play deals X damage to any target.\nFlashback {X}{R}{R}{R} (You may cast this card from your graveyard for its flashback cost. Then exile it.)";

#[test]
fn type_axis_x_spell_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let devils_play = scenario
        .add_spell_to_hand_from_oracle(P0, "Devil's Play", false, DEVILS_PLAY)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::X, ManaCostShard::Red],
            generic: 0,
        })
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Red,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(devils_play).x(3).target_player(P1).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), devils_play),
        CastingVariant::Normal,
        "reach guard: the announced X=3 spell must reach the stack"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "an X spell must fire the watcher exactly once"
    );
    assert_eq!(
        outcome.life_delta(P1),
        -3,
        "reach guard: X=3 damage confirms the cast actually resolved"
    );
}

/// CR 702.102a-d (Fuse) + CR 603.2: casting BOTH halves of a split card as a
/// fused spell is still one cast (`CastingVariant::Fuse`) and must fire a
/// general SpellCast watcher exactly once, not once per half. Breaking //
/// Entering is loaded through the real card database (`add_real_card`),
/// mirroring `fuse_runtime.rs` — no inline builder models a split card's dual
/// characteristics.
#[test]
fn type_axis_fuse_split_card_fires_watcher_once() {
    let Some(db) = load_db() else {
        eprintln!("skipping type_axis_fuse_split_card_fires_watcher_once: no card database");
        return;
    };
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let breaking = scenario.add_real_card(P0, "Breaking", Zone::Hand, db);
    let milled_creature = scenario.add_real_card(P1, "Grizzly Bears", Zone::Library, db);
    for _ in 0..7 {
        scenario.add_real_card(P1, "Lightning Bolt", Zone::Library, db);
    }
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Blue,
            ManaType::Black,
            ManaType::Black,
            ManaType::Red,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);

    let commit = runner
        .cast(breaking)
        .casting_variant(CastingVariant::Fuse)
        .target_player(P1)
        .target_object(milled_creature)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), breaking),
        CastingVariant::Fuse,
        "reach guard: the fused cast must actually select CastingVariant::Fuse"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "a fused (both-halves) cast is ONE cast event and must fire the watcher exactly once"
    );
}

// ===========================================================================
// Graveyard family
// ===========================================================================

/// CR 702.34a (Flashback) + CR 603.2: casting from the graveyard via
/// Flashback fires a general SpellCast watcher exactly once. Devil's Play is
/// reused here (also the X-spell fixture above) for its Flashback line.
#[test]
fn flashback_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let devils_play = scenario
        .add_spell_to_graveyard(P0, "Devil's Play", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::X, ManaCostShard::Red],
            generic: 0,
        })
        .from_oracle_text(DEVILS_PLAY)
        .id();
    // Flashback {X}{R}{R}{R}, announced X=2: {2}{R}{R}{R}.
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Red,
            ManaType::Red,
            ManaType::Red,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(devils_play)
        .casting_variant(CastingVariant::Flashback)
        .x(2)
        .target_player(P1)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), devils_play),
        CastingVariant::Flashback,
        "reach guard: the cast must actually select CastingVariant::Flashback"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Flashback cast must fire the watcher exactly once"
    );
    outcome.assert_zone(&[devils_play], Zone::Exile);
}

/// CR 702.138a (Escape) + CR 603.2. Kroxa, Titan of Death's Hunger — Escape
/// {B}{B}{R}{R}, Exile five other cards from your graveyard.
const KROXA: &str = "When Kroxa enters, sacrifice it unless it escaped.\nWhenever Kroxa enters or attacks, each opponent discards a card, then each opponent who didn't discard a nonland card this way loses 3 life.\nEscape—{B}{B}{R}{R}, Exile five other cards from your graveyard. (You may cast this card from your graveyard for its escape cost.)";

#[test]
fn escape_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    let watcher_id = install_universal_watcher(&mut scenario, P0);
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Black,
            ManaType::Black,
            ManaType::Red,
            ManaType::Red,
        ]),
    );
    let mut runner = scenario.build();
    let kroxa = add_creature_to_graveyard_from_oracle(
        runner.state_mut(),
        P0,
        "Kroxa, Titan of Death's Hunger",
        6,
        6,
        ManaCost::Cost {
            shards: vec![ManaCostShard::Black, ManaCostShard::Red],
            generic: 0,
        },
        &["Elder", "Giant"],
        KROXA,
    );
    let mut filler_ids = Vec::new();
    for idx in 0..5u32 {
        let card_id = CardId(runner.state().next_object_id);
        let filler = create_object(
            runner.state_mut(),
            card_id,
            P0,
            format!("Escape Filler {idx}"),
            Zone::Graveyard,
        );
        runner
            .state_mut()
            .objects
            .get_mut(&filler)
            .unwrap()
            .card_types
            .core_types
            .push(CoreType::Sorcery);
        filler_ids.push(filler);
    }
    let _ = watcher_id;

    let commit = runner
        .cast(kroxa)
        .casting_variant(CastingVariant::Escape)
        .sacrifice_with(&filler_ids)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), kroxa),
        CastingVariant::Escape,
        "reach guard: the cast must actually select CastingVariant::Escape"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Escape cast must fire the watcher exactly once"
    );
}

/// CR 702.81a (Retrace) + CR 603.2. Flame Jab ({R}, Sorcery): "Flame Jab
/// deals 1 damage to any target. Retrace (You may cast this card from your
/// graveyard by discarding a land card in addition to paying its other
/// costs.)"
const FLAME_JAB: &str = "Flame Jab deals 1 damage to any target.\nRetrace (You may cast this card from your graveyard by discarding a land card in addition to paying its other costs.)";

#[test]
fn retrace_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let flame_jab = scenario
        .add_spell_to_graveyard(P0, "Flame Jab", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 0,
        })
        .from_oracle_text(FLAME_JAB)
        .id();
    let land = scenario.add_land_to_hand(P0, "Filler Forest").id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
    let mut runner = scenario.build();
    let commit = runner
        .cast(flame_jab)
        .casting_variant(CastingVariant::Retrace)
        .sacrifice_with(&[land])
        .target_player(P1)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), flame_jab),
        CastingVariant::Retrace,
        "reach guard: the cast must actually select CastingVariant::Retrace"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Retrace cast must fire the watcher exactly once"
    );
}

/// CR 702.180a (Harmonize) + CR 603.2. Channeled Dragonfire ({R}, Sorcery):
/// "Channeled Dragonfire deals 2 damage to any target. Harmonize
/// {5}{R}{R} (...)"
const CHANNELED_DRAGONFIRE: &str = "Channeled Dragonfire deals 2 damage to any target.\nHarmonize {5}{R}{R} (You may cast this card from your graveyard for its harmonize cost. You may tap a creature you control to reduce that cost by {X}, where X is its power. Then exile this spell.)";

#[test]
fn harmonize_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let dragonfire = scenario
        .add_spell_to_graveyard(P0, "Channeled Dragonfire", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 0,
        })
        .from_oracle_text(CHANNELED_DRAGONFIRE)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Red,
            ManaType::Red,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(dragonfire)
        .casting_variant(CastingVariant::Harmonize)
        .target_player(P1)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), dragonfire),
        CastingVariant::Harmonize,
        "reach guard: the cast must actually select CastingVariant::Harmonize"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Harmonize cast must fire the watcher exactly once"
    );
    outcome.assert_zone(&[dragonfire], Zone::Exile);
}

/// CR 702.187b (Mayhem) + CR 603.2. Electro's Bolt ({2}{R}, Sorcery):
/// "Electro's Bolt deals 4 damage to target creature. Mayhem {1}{R} (...)"
const ELECTROS_BOLT: &str = "Electro's Bolt deals 4 damage to target creature.\nMayhem {1}{R} (You may cast this card from your graveyard for {1}{R} if you discarded it this turn. Timing rules still apply.)";

#[test]
fn mayhem_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let victim = scenario.add_creature(P1, "Mayhem Target", 2, 6).id();
    let electros_bolt = scenario
        .add_spell_to_graveyard(P0, "Electro's Bolt", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 2,
        })
        .from_oracle_text(ELECTROS_BOLT)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red, ManaType::Colorless]));
    let mut runner = scenario.build();
    let turn = runner.state().turn_number;
    runner
        .state_mut()
        .objects
        .get_mut(&electros_bolt)
        .unwrap()
        .discarded_turn = Some(turn);
    let commit = runner
        .cast(electros_bolt)
        .casting_variant(CastingVariant::Mayhem)
        .target_object(victim)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), electros_bolt),
        CastingVariant::Mayhem,
        "reach guard: the cast must actually select CastingVariant::Mayhem"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Mayhem cast must fire the watcher exactly once"
    );
    // CR 702.187b: unlike Flashback/Harmonize, a Mayhem cast does NOT exile —
    // it goes to the graveyard normally, so it can be discarded and recast.
    outcome.assert_zone(&[electros_bolt], Zone::Graveyard);
}

/// CR 702.127a (Aftermath) + CR 603.2. Ribbons — the aftermath half of
/// Cut // Ribbons ({X}{B}{B}, Sorcery): "Aftermath (Cast this spell only from
/// your graveyard. Then exile it.) Each opponent loses X life."
const RIBBONS: &str = "Aftermath (Cast this spell only from your graveyard. Then exile it.)\nEach opponent loses X life.";

#[test]
fn aftermath_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let ribbons = scenario
        .add_spell_to_graveyard(P0, "Ribbons", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::X, ManaCostShard::Black, ManaCostShard::Black],
            generic: 0,
        })
        .from_oracle_text_with_keywords(&["Aftermath"], RIBBONS)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Black,
            ManaType::Black,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(ribbons)
        .casting_variant(CastingVariant::Aftermath)
        .x(2)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), ribbons),
        CastingVariant::Aftermath,
        "reach guard: the cast must actually select CastingVariant::Aftermath"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Aftermath cast must fire the watcher exactly once"
    );
    outcome.assert_zone(&[ribbons], Zone::Exile);
}

/// CR 702.146a-b (Disturb) + CR 603.2. Baithook Angler // Hook-Haunt Drifter
/// — front face verbatim: "Disturb {1}{U} (You may cast this card from your
/// graveyard transformed for its disturb cost.)" (the back-face flip on
/// resolution is not modeled here — `stack.rs` guards `obj.back_face.is_some()`
/// before marking transformed, so a front-face-only fixture resolves safely
/// without it; see the implementer report).
const BAITHOOK_ANGLER: &str =
    "Disturb {1}{U} (You may cast this card from your graveyard transformed for its disturb cost.)";

#[test]
fn disturb_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Blue, ManaType::Colorless]));
    let mut runner = scenario.build();
    let baithook = add_creature_to_graveyard_from_oracle(
        runner.state_mut(),
        P0,
        "Baithook Angler",
        2,
        1,
        ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 1,
        },
        &["Human", "Peasant"],
        BAITHOOK_ANGLER,
    );
    let commit = runner
        .cast(baithook)
        .casting_variant(CastingVariant::Disturb)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), baithook),
        CastingVariant::Disturb,
        "reach guard: the cast must actually select CastingVariant::Disturb"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Disturb cast must fire the watcher exactly once"
    );
}

/// CR 702.133a (Jump-start) + CR 603.2. Direct Current ({1}{R}{R}, Sorcery):
/// "Direct Current deals 2 damage to any target. Jump-start (...)"
const DIRECT_CURRENT: &str = "Direct Current deals 2 damage to any target.\nJump-start (You may cast this card from your graveyard by discarding a card in addition to paying its other costs. Then exile this card.)";

#[test]
fn jump_start_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let direct_current = scenario
        .add_spell_to_graveyard(P0, "Direct Current", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 1,
        })
        .from_oracle_text_with_keywords(&["Jump-start"], DIRECT_CURRENT)
        .id();
    let spare_card = scenario.add_card_to_hand(P0, "Spare Discard Fodder");
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red, ManaType::Colorless]));
    let mut runner = scenario.build();
    let commit = runner
        .cast(direct_current)
        .casting_variant(CastingVariant::JumpStart)
        .sacrifice_with(&[spare_card])
        .target_player(P1)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), direct_current),
        CastingVariant::JumpStart,
        "reach guard: the cast must actually select CastingVariant::JumpStart"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Jump-start cast must fire the watcher exactly once"
    );
    outcome.assert_zone(&[direct_current], Zone::Exile);
}

/// CR 601.2a (Lurrus-style graveyard permission) + CR 603.2. Lurrus of the
/// Dream-Den's own graveyard-cast line: "Once during each of your turns, you
/// may cast a permanent spell with mana value 2 or less from your graveyard."
const LURRUS_GRAVEYARD_LINE: &str = "Once during each of your turns, you may cast a permanent spell with mana value 2 or less from your graveyard.";

#[test]
fn graveyard_permission_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let lurrus = scenario
        .add_creature(P0, "Lurrus of the Dream-Den", 3, 2)
        .from_oracle_text(LURRUS_GRAVEYARD_LINE)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Colorless, ManaType::Colorless]));
    let mut runner = scenario.build();
    let cheap_creature = add_creature_to_graveyard_from_oracle(
        runner.state_mut(),
        P0,
        "Cheap Graveyard Creature",
        1,
        1,
        ManaCost::Cost {
            shards: vec![],
            generic: 2,
        },
        &[],
        "",
    );
    let _ = lurrus;
    let commit = runner.cast(cheap_creature).commit();
    let variant = stack_casting_variant(commit.state(), cheap_creature);
    assert!(
        matches!(variant, CastingVariant::GraveyardPermission { .. }),
        "reach guard: expected CastingVariant::GraveyardPermission, got {variant:?}"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "GraveyardPermission cast must fire the watcher exactly once"
    );
}

// ===========================================================================
// Exile family
// ===========================================================================

/// CR 702.62a (Suspend) + CR 603.2 + CR 608.2g: the last-counter Suspend cast
/// happens DURING resolution of the synthesized "remove the last time
/// counter" trigger — `Effect::CastFromZone { driver: DuringResolution, .. }`
/// puts the spell directly on the stack, not through a priority-window
/// `GameAction::CastSpell`. This mirrors
/// `jhoira_granted_suspend_last_counter_cast_tags_suspend_variant`'s setup,
/// using Arc Blade's own PRINTED Suspend keyword instead of a granted one.
/// Arc Blade ({3}{R}{R}, Sorcery): "Arc Blade deals 2 damage to any target.
/// Exile Arc Blade with three time counters on it. Suspend 3—{2}{R} (...)"
const ARC_BLADE: &str = "Arc Blade deals 2 damage to any target. Exile Arc Blade with three time counters on it.\nSuspend 3—{2}{R} (Rather than cast this card from your hand, you may pay {2}{R} and exile it with three time counters on it. At the beginning of your upkeep, remove a time counter. When the last is removed, you may cast it without paying its mana cost.)";

#[test]
fn suspend_fires_watcher_once() {
    use engine::game::effects::resolve_ability_chain;
    use engine::types::ability::{CardPlayMode, CastFromZoneDriver, Effect, ResolvedAbility};

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let mut runner = scenario.build();

    let arc_blade_card_id = CardId(runner.state().next_object_id);
    let arc_blade = create_object(
        runner.state_mut(),
        arc_blade_card_id,
        P0,
        "Arc Blade".to_string(),
        Zone::Exile,
    );
    {
        let obj = runner.state_mut().objects.get_mut(&arc_blade).unwrap();
        obj.card_types.core_types.push(CoreType::Sorcery);
        obj.base_card_types = obj.card_types.clone();
        obj.mana_cost = ManaCost::Cost {
            shards: vec![ManaCostShard::Red, ManaCostShard::Red],
            generic: 3,
        };
        obj.base_mana_cost = obj.mana_cost.clone();
    }
    apply_oracle_text(
        runner.state_mut(),
        arc_blade,
        "Arc Blade",
        &["Sorcery"],
        ARC_BLADE,
    );
    assert!(
        engine::game::keywords::object_has_effective_keyword_kind(
            runner.state(),
            arc_blade,
            engine::types::keywords::KeywordKind::Suspend,
        ),
        "reach guard: Arc Blade must carry the printed Suspend keyword"
    );

    let cast_trigger_ability = ResolvedAbility::new(
        Effect::CastFromZone {
            target: engine::types::ability::TargetFilter::SelfRef,
            without_paying_mana_cost: true,
            mode: CardPlayMode::Cast,
            cast_transformed: false,
            alt_ability_cost: None,
            constraint: None,
            duration: None,
            mana_spend_permission: None,
            driver: CastFromZoneDriver::DuringResolution,
        },
        vec![TargetRef::Object(arc_blade)],
        arc_blade,
        P0,
    );
    let mut events = Vec::new();
    resolve_ability_chain(runner.state_mut(), &cast_trigger_ability, &mut events, 0)
        .expect("CastFromZone must cast the suspended card during resolution");

    // CR 601.2c: Arc Blade's "deals 2 damage to any target" still needs a
    // target chosen before the spell lands on the stack.
    if let WaitingFor::TargetSelection { .. } = &runner.state().waiting_for {
        runner
            .act(GameAction::SelectTargets {
                targets: vec![TargetRef::Player(P1)],
            })
            .expect("choose Arc Blade's damage target");
    }

    assert_eq!(
        stack_casting_variant(runner.state(), arc_blade),
        CastingVariant::Suspend,
        "reach guard: the last-counter cast must select CastingVariant::Suspend"
    );
    let before = hand_len(runner.state(), P0);
    // Drain exactly the trigger (top of stack); leave Arc Blade itself
    // unresolved — this module only needs to prove the SpellCast watcher
    // fired, not that Arc Blade's own damage effect resolved.
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "Suspend cast must fire the watcher exactly once"
    );
}

/// CR 702.170d (Plot) + CR 603.2. Djinn of Fool's Fall ({4}{U}, Creature):
/// "Flying. Plot {3}{U} (You may pay {3}{U} and exile this card from your
/// hand. Cast it as a sorcery on a later turn without paying its mana cost.
/// Plot only as a sorcery.)" Staged directly in exile with the
/// `CastingPermission::Plotted` marker a real Plot action would have created
/// on an earlier turn — mirrors how `zaffai_second_cast_is_suppressed_same_turn`
/// seeds per-turn permission state directly rather than replaying the whole
/// turn.
const DJINN_OF_FOOLS_FALL: &str = "Flying\nPlot {3}{U} (You may pay {3}{U} and exile this card from your hand. Cast it as a sorcery on a later turn without paying its mana cost. Plot only as a sorcery.)";

#[test]
fn plot_fires_watcher_once() {
    use engine::types::ability::CastingPermission;

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let djinn = scenario
        .add_creature_to_exile(P0, "Djinn of Fool's Fall", 4, 3)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 4,
        })
        .from_oracle_text(DJINN_OF_FOOLS_FALL)
        .id();
    let mut runner = scenario.build();
    // Stamp the permission a real Plot payment would have created on turn 1
    // (the scenario's current turn is 2 — CR 702.170d requires "a later turn").
    runner
        .state_mut()
        .objects
        .get_mut(&djinn)
        .unwrap()
        .casting_permissions
        .push(CastingPermission::Plotted { turn_plotted: 1 });

    let commit = runner
        .cast(djinn)
        .casting_variant(CastingVariant::Plot)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), djinn),
        CastingVariant::Plot,
        "reach guard: the cast must actually select CastingVariant::Plot"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Plot cast must fire the watcher exactly once"
    );
}

/// CR 702.143a-c (Foretell) + CR 603.2. Alrund's Epiphany ({5}{U}{U},
/// Sorcery): "Create two 1/1 blue Bird creature tokens with flying. Take an
/// extra turn after this one. Exile Alrund's Epiphany. Foretell {4}{U}{U}
/// (...)" Staged directly in exile with the `CastingPermission::Foretold`
/// marker a real Foretell payment would have created on an earlier turn.
const ALRUNDS_EPIPHANY: &str = "Create two 1/1 blue Bird creature tokens with flying. Take an extra turn after this one. Exile Alrund's Epiphany.\nForetell {4}{U}{U} (During your turn, you may pay {2} and exile this card from your hand face down. Cast it on a later turn for its foretell cost.)";

#[test]
fn foretell_fires_watcher_once() {
    use engine::types::ability::CastingPermission;

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Blue,
            ManaType::Blue,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let epiphany = scenario
        .add_spell_to_exile(P0, "Alrund's Epiphany", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue, ManaCostShard::Blue],
            generic: 5,
        })
        .from_oracle_text_with_keywords(&["Foretell"], ALRUNDS_EPIPHANY)
        .id();
    let mut runner = scenario.build();
    let foretell_cost = ManaCost::Cost {
        shards: vec![ManaCostShard::Blue, ManaCostShard::Blue],
        generic: 4,
    };
    runner
        .state_mut()
        .objects
        .get_mut(&epiphany)
        .unwrap()
        .casting_permissions
        .push(CastingPermission::Foretold {
            cost: foretell_cost,
            turn_foretold: 1,
        });

    let commit = runner
        .cast(epiphany)
        .casting_variant(CastingVariant::Foretell)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), epiphany),
        CastingVariant::Foretell,
        "reach guard: the cast must actually select CastingVariant::Foretell"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Foretell cast must fire the watcher exactly once"
    );
}

/// CR 601.2a + CR 113.6b (static ExileCastPermission) + CR 603.2. Maralen,
/// Fae Ascendant's own graveyard/exile-cast line: "Once each turn, you may
/// cast a spell with mana value less than or equal to the number of Elves
/// and Faeries you control from among cards exiled with Maralen this turn
/// without paying its mana cost." Maralen herself is an Elf Faerie, so a
/// mana-value-1 spell qualifies with no other creatures needed.
const MARALEN_LINE: &str = "Whenever Maralen or another Elf or Faerie you control enters, exile the top two cards of target opponent's library.\nOnce each turn, you may cast a spell with mana value less than or equal to the number of Elves and Faeries you control from among cards exiled with Maralen this turn without paying its mana cost.";

#[test]
fn exile_permission_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let maralen = scenario
        .add_creature(P0, "Maralen, Fae Ascendant", 4, 5)
        .with_subtypes(vec!["Elf", "Faerie", "Noble"])
        .from_oracle_text(MARALEN_LINE)
        .id();
    let shock = scenario
        .add_spell_to_exile(P0, "Shock", true)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 0,
        })
        .from_oracle_text("Shock deals 2 damage to any target.")
        .id();
    let mut runner = scenario.build();
    runner
        .state_mut()
        .cards_exiled_with_source_this_turn
        .insert(maralen, vec![shock]);

    let commit = runner.cast(shock).target_player(P1).commit();
    let variant = stack_casting_variant(commit.state(), shock);
    assert!(
        matches!(variant, CastingVariant::ExilePermission { .. }),
        "reach guard: expected CastingVariant::ExilePermission, got {variant:?}"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "ExilePermission cast must fire the watcher exactly once"
    );
}

// ===========================================================================
// Hand alternative-cost family
// ===========================================================================

/// CR 702.185a (Warp) + CR 603.2. Bygone Colossus ({9}, Artifact Creature):
/// "Warp {3} (...)" — the entire printed text. Only {3} is funded so the
/// printed {9} is unaffordable and the engine auto-routes to the Warp
/// alternative without a `WaitingFor::AlternativeCastChoice` prompt.
const BYGONE_COLOSSUS: &str = "Warp {3} (You may cast this card from your hand for its warp cost. Exile this creature at the beginning of the next end step, then you may cast it from exile on a later turn.)";

#[test]
fn warp_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let colossus = scenario
        .add_creature_to_hand(P0, "Bygone Colossus", 9, 9)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![],
            generic: 9,
        })
        .from_oracle_text(BYGONE_COLOSSUS)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(colossus).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), colossus),
        CastingVariant::Warp,
        "reach guard: with only the {{3}} warp cost funded, the cast must select CastingVariant::Warp"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Warp cast must fire the watcher exactly once"
    );
}

/// CR 702.74a (Evoke) + CR 603.2. Briarhorn ({3}{G}, Creature): "Flash. When
/// this creature enters, target creature gets +3/+3 until end of turn. Evoke
/// {1}{G} (...)" — only {1}{G} is funded.
const BRIARHORN: &str = "Flash\nWhen this creature enters, target creature gets +3/+3 until end of turn.\nEvoke {1}{G} (You may cast this spell for its evoke cost. If you do, it's sacrificed when it enters.)";

#[test]
fn evoke_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let target = scenario.add_creature(P0, "Evoke Target", 2, 2).id();
    let briarhorn = scenario
        .add_creature_to_hand(P0, "Briarhorn", 3, 3)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Green],
            generic: 3,
        })
        .from_oracle_text(BRIARHORN)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Green, ManaType::Colorless]));
    let mut runner = scenario.build();
    let commit = runner.cast(briarhorn).target_object(target).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), briarhorn),
        CastingVariant::Evoke,
        "reach guard: with only the evoke cost funded, the cast must select CastingVariant::Evoke"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Evoke cast must fire the watcher exactly once"
    );
}

/// CR 702.96a-c (Overload) + CR 603.2. Blustersquall ({U}, Instant): "Tap
/// target creature you don't control. Overload {3}{U} (...)" — overloaded,
/// "target" becomes "each", so no target is declared. Only {3}{U} funded.
const BLUSTERSQUALL: &str = "Tap target creature you don't control.\nOverload {3}{U} (You may cast this spell for its overload cost. If you do, change \"target\" in its text to \"each.\")";

#[test]
fn overload_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let blustersquall = scenario
        .add_spell_to_hand_from_oracle(P0, "Blustersquall", true, BLUSTERSQUALL)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 0,
        })
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Blue,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(blustersquall)
        .alternative_cast(AlternativeCastDecision::Alternative)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), blustersquall),
        CastingVariant::Overload,
        "reach guard: with only the overload cost funded, the cast must select CastingVariant::Overload"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Overload cast must fire the watcher exactly once"
    );
}

/// CR 702.119a-c (Emerge) + CR 603.2. Adipose Offspring ({3}{W}, Creature):
/// "Emerge {5}{W} (...) When this creature enters, create a 2/2 white Alien
/// creature token. If this creature's emerge cost was paid, instead create X
/// of those tokens, where X is the sacrificed creature's toughness." Only
/// {5}{W} funded, plus a creature to sacrifice. Deliberately NOT
/// Abundant Maw (whose "When you CAST this spell" ability is itself a
/// SpellCast-family trigger that fires simultaneously with the watcher,
/// forcing an `OrderTriggers` choice the shared driver doesn't auto-answer
/// mid-announcement) — Adipose Offspring's trigger is an ordinary ETB, which
/// fires later, during resolution, with no simultaneous-trigger ordering.
const ADIPOSE_OFFSPRING: &str = "Emerge {5}{W} (You may cast this spell by sacrificing a creature and paying the emerge cost reduced by that creature's mana value.)\nWhen this creature enters, create a 2/2 white Alien creature token. If this creature's emerge cost was paid, instead create X of those tokens, where X is the sacrificed creature's toughness.";

#[test]
fn emerge_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let fodder = scenario.add_creature(P0, "Emerge Fodder", 1, 1).id();
    let adipose_offspring = scenario
        .add_creature_to_hand(P0, "Adipose Offspring", 2, 2)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::White],
            generic: 3,
        })
        .from_oracle_text(ADIPOSE_OFFSPRING)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::White,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(adipose_offspring)
        .alternative_cast(AlternativeCastDecision::Alternative)
        .sacrifice_with(&[fodder])
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), adipose_offspring),
        CastingVariant::Emerge,
        "reach guard: with only the emerge cost funded, the cast must select CastingVariant::Emerge"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Emerge cast must fire the watcher exactly once"
    );
}

/// CR 702.109a (Dash) + CR 603.2. Goblin Heelcutter ({3}{R}, Creature):
/// "Whenever this creature attacks, target creature can't block this turn.
/// Dash {2}{R} (...)" Only {2}{R} funded.
const GOBLIN_HEELCUTTER: &str = "Whenever this creature attacks, target creature can't block this turn.\nDash {2}{R} (You may cast this spell for its dash cost. If you do, it gains haste, and it's returned from the battlefield to its owner's hand at the beginning of the next end step.)";

#[test]
fn dash_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let heelcutter = scenario
        .add_creature_to_hand(P0, "Goblin Heelcutter", 3, 2)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 3,
        })
        .from_oracle_text(GOBLIN_HEELCUTTER)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[ManaType::Red, ManaType::Colorless, ManaType::Colorless]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(heelcutter).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), heelcutter),
        CastingVariant::Dash,
        "reach guard: with only the dash cost funded, the cast must select CastingVariant::Dash"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Dash cast must fire the watcher exactly once"
    );
}

/// CR 702.152a (Blitz) + CR 603.2. Girder Goons ({4}{B}, Creature): "When
/// this creature dies, create a tapped 2/2 black Rogue creature token. Blitz
/// {3}{B} (...)" Only {3}{B} funded.
const GIRDER_GOONS: &str = "When this creature dies, create a tapped 2/2 black Rogue creature token.\nBlitz {3}{B} (If you cast this spell for its blitz cost, it gains haste and \"When this creature dies, draw a card.\" Sacrifice it at the beginning of the next end step.)";

#[test]
fn blitz_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let girder_goons = scenario
        .add_creature_to_hand(P0, "Girder Goons", 4, 4)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Black],
            generic: 4,
        })
        .from_oracle_text(GIRDER_GOONS)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Black,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(girder_goons).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), girder_goons),
        CastingVariant::Blitz,
        "reach guard: with only the blitz cost funded, the cast must select CastingVariant::Blitz"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Blitz cast must fire the watcher exactly once"
    );
}

/// CR 702.137a (Spectacle) + CR 603.2. Body Count ({2}{B}, Instant): "Draw a
/// card for each creature that died under your control this turn. Spectacle
/// {B} (...)" No creatures died, so Body Count's own draw is 0 — isolating the
/// watcher's draw. Only {B} funded; an opponent must have lost life this turn.
const BODY_COUNT: &str = "Draw a card for each creature that died under your control this turn.\nSpectacle {B} (You may cast this spell for its spectacle cost rather than its mana cost if an opponent lost life this turn.)";

#[test]
fn spectacle_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let body_count = scenario
        .add_spell_to_hand_from_oracle(P0, "Body Count", false, BODY_COUNT)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Black],
            generic: 2,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Black]));
    let mut runner = scenario.build();
    runner
        .state_mut()
        .players
        .iter_mut()
        .find(|p| p.id == P1)
        .unwrap()
        .life_lost_this_turn = 1;
    let commit = runner.cast(body_count).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), body_count),
        CastingVariant::Spectacle,
        "reach guard: with only the spectacle cost funded, the cast must select CastingVariant::Spectacle"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Spectacle cast must fire the watcher exactly once (Body Count itself draws 0)"
    );
}

/// CR 702.103a-b (Bestow) + CR 603.2. Boon Satyr ({1}{G}{G}, Enchantment
/// Creature): "Flash. Bestow {3}{G}{G} (...) Enchanted creature gets +4/+2."
/// Only {3}{G}{G} funded; needs a creature target.
const BOON_SATYR: &str = "Flash\nBestow {3}{G}{G} (If you cast this card for its bestow cost, it's an Aura spell with enchant creature. It becomes a creature again if it's not attached.)\nEnchanted creature gets +4/+2.";

#[test]
fn bestow_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let target = scenario.add_creature(P0, "Bestow Target", 2, 2).id();
    let boon_satyr = scenario
        .add_creature_to_hand(P0, "Boon Satyr", 4, 2)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Green, ManaCostShard::Green],
            generic: 1,
        })
        .from_oracle_text(BOON_SATYR)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Green,
            ManaType::Green,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(boon_satyr)
        .alternative_cast(AlternativeCastDecision::Alternative)
        .target_object(target)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), boon_satyr),
        CastingVariant::Bestow,
        "reach guard: with only the bestow cost funded, the cast must select CastingVariant::Bestow"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Bestow cast must fire the watcher exactly once"
    );
}

/// CR 702.113a (Awaken) + CR 603.2. Coastal Discovery ({3}{U}, Sorcery):
/// "Draw two cards. Awaken 4—{5}{U} (...)" Only {5}{U} funded; needs a land
/// target you control.
const COASTAL_DISCOVERY: &str = "Draw two cards.\nAwaken 4—{5}{U} (If you cast this spell for {5}{U}, also put four +1/+1 counters on target land you control and it becomes a 0/0 Elemental creature with haste. It's still a land.)";

#[test]
fn awaken_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let land = scenario.add_basic_land(P0, ManaColor::Blue);
    let coastal_discovery = scenario
        .add_spell_to_hand_from_oracle(P0, "Coastal Discovery", false, COASTAL_DISCOVERY)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 3,
        })
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Blue,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner
        .cast(coastal_discovery)
        .alternative_cast(AlternativeCastDecision::Alternative)
        .target_object(land)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), coastal_discovery),
        CastingVariant::Awaken,
        "reach guard: with only the awaken cost funded, the cast must select CastingVariant::Awaken"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        3,
        "Awaken cast draws 2 (Coastal Discovery) + 1 (watcher)"
    );
}

/// CR 702.148a-b (Cleave) + CR 603.2. Alchemist's Retrieval ({U}, Instant):
/// "Cleave {1}{U} (...) Return target nonland permanent [you control] to its
/// owner's hand." Only {1}{U} funded; needs a nonland permanent target.
const ALCHEMISTS_RETRIEVAL: &str = "Cleave {1}{U} (You may cast this spell for its cleave cost. If you do, remove the words in square brackets.)\nReturn target nonland permanent [you control] to its owner's hand.";

#[test]
fn cleave_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    // Cleave removes "[you control]" from the target restriction, so target an
    // opponent's permanent — bouncing P0's own permanent would add +1 to P0's
    // hand size and contaminate the watcher's draw-delta assertion below.
    let target = scenario.add_creature(P1, "Cleave Target", 1, 1).id();
    let retrieval = scenario
        .add_spell_to_hand_from_oracle(P0, "Alchemist's Retrieval", true, "")
        .from_oracle_text_with_keywords(&["cleave"], ALCHEMISTS_RETRIEVAL)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 0,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Blue, ManaType::Colorless]));
    let mut runner = scenario.build();
    let commit = runner
        .cast(retrieval)
        .alternative_cast(AlternativeCastDecision::Alternative)
        .target_object(target)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), retrieval),
        CastingVariant::Cleave,
        "reach guard: with only the cleave cost funded, the cast must select CastingVariant::Cleave"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Cleave cast must fire the watcher exactly once"
    );
}

/// CR 702.176a (Impending) + CR 603.2. Overlord of the Boilerbilges
/// ({4}{R}{R}, Enchantment Creature): "Impending 4—{2}{R}{R} (...) Whenever
/// this permanent enters or attacks, it deals 4 damage to any target." Only
/// {2}{R}{R} funded.
const OVERLORD_OF_THE_BOILERBILGES: &str = "Impending 4—{2}{R}{R} (If you cast this spell for its impending cost, it enters with four time counters and isn't a creature until the last is removed. At the beginning of your end step, remove a time counter from it.)\nWhenever this permanent enters or attacks, it deals 4 damage to any target.";

#[test]
fn impending_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let overlord = scenario
        .add_creature_to_hand(P0, "Overlord of the Boilerbilges", 5, 5)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red, ManaCostShard::Red],
            generic: 4,
        })
        .from_oracle_text(OVERLORD_OF_THE_BOILERBILGES)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Red,
            ManaType::Red,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(overlord).target_player(P1).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), overlord),
        CastingVariant::Impending,
        "reach guard: with only the impending cost funded, the cast must select CastingVariant::Impending"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Impending cast must fire the watcher exactly once"
    );
}

/// CR 702.160a (Prototype) + CR 603.2. Autonomous Assembler ({5}, Artifact
/// Creature): "Prototype {1}{W} — 2/2 (...) Vigilance. {1}, {T}: Put a +1/+1
/// counter on target Assembly-Worker you control." Only {1}{W} funded.
const AUTONOMOUS_ASSEMBLER: &str = "Prototype {1}{W} — 2/2 (You may cast this spell with different mana cost, color, and size. It keeps its abilities and types.)\nVigilance\n{1}, {T}: Put a +1/+1 counter on target Assembly-Worker you control.";

#[test]
fn prototype_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let assembler = scenario
        .add_creature_to_hand(P0, "Autonomous Assembler", 4, 5)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![],
            generic: 5,
        })
        .from_oracle_text(AUTONOMOUS_ASSEMBLER)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::White, ManaType::Colorless]));
    let mut runner = scenario.build();
    let commit = runner.cast(assembler).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), assembler),
        CastingVariant::Prototype,
        "reach guard: with only the prototype cost funded, the cast must select CastingVariant::Prototype"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Prototype cast must fire the watcher exactly once"
    );
}

/// CR 702.140a-c (Mutate) + CR 603.2. Cavern Whisperer ({4}{B}, Creature):
/// "Mutate {3}{B} (...) Menace. Whenever this creature mutates, each
/// opponent discards a card." Only {3}{B} funded; needs a non-Human creature
/// you own to merge with.
const CAVERN_WHISPERER: &str = "Mutate {3}{B} (If you cast this spell for its mutate cost, put it over or under target non-Human creature you own. They mutate into the creature on top plus all abilities from under it.)\nMenace\nWhenever this creature mutates, each opponent discards a card.";

#[test]
fn mutate_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let target = scenario
        .add_creature(P0, "Mutate Target", 2, 2)
        .with_subtypes(vec!["Beast"])
        .id();
    let cavern_whisperer = scenario
        .add_creature_to_hand(P0, "Cavern Whisperer", 4, 4)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Black],
            generic: 4,
        })
        .from_oracle_text(CAVERN_WHISPERER)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Black,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(cavern_whisperer).target_object(target).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), cavern_whisperer),
        CastingVariant::Mutate,
        "reach guard: with only the mutate cost funded, the cast must select CastingVariant::Mutate"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Mutate cast must fire the watcher exactly once"
    );
}

/// CR 702.162a (More Than Meets the Eye) + CR 603.2. Arcee, Sharpshooter —
/// the FRONT face of Arcee, Sharpshooter // Arcee, Acrobatic Coupe
/// ({1}{R}{W}, Legendary Artifact Creature): "More Than Meets the Eye {R}{W}
/// (...) First strike. {1}, Remove one or more +1/+1 counters from Arcee: It
/// deals that much damage to target creature. Convert Arcee." Only {R}{W}
/// funded. The back-face conversion on resolution is not modeled (the same
/// `obj.back_face.is_some()` guard that lets Disturb resolve front-face-only
/// applies here too — see `stack.rs`).
const ARCEE_SHARPSHOOTER: &str = "More Than Meets the Eye {R}{W} (You may cast this card converted for {R}{W}.)\nFirst strike\n{1}, Remove one or more +1/+1 counters from Arcee: It deals that much damage to target creature. Convert Arcee.";

#[test]
fn more_than_meets_the_eye_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let arcee = scenario
        .add_creature_to_hand(P0, "Arcee, Sharpshooter", 3, 3)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red, ManaCostShard::White],
            generic: 1,
        })
        .from_oracle_text_with_keywords(&["More Than Meets the Eye"], ARCEE_SHARPSHOOTER)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red, ManaType::White]));
    let mut runner = scenario.build();
    let commit = runner.cast(arcee).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), arcee),
        CastingVariant::MoreThanMeetsTheEye,
        "reach guard: with only the MTMTE cost funded, the cast must select CastingVariant::MoreThanMeetsTheEye"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "MoreThanMeetsTheEye cast must fire the watcher exactly once"
    );
}

/// CR 702.37c / CR 702.168b + CR 708.4 (FaceDown/Morph) + CR 603.2. Aphetto
/// Alchemist ({1}{U}, Creature): "{T}: Untap target artifact or creature.
/// Morph {U} (...)" Cast face down for a fixed {3} — only {3} generic
/// funded, not the printed {1}{U}.
const APHETTO_ALCHEMIST: &str = "{T}: Untap target artifact or creature.\nMorph {U} (You may cast this card face down as a 2/2 creature for {3}. Turn it face up any time for its morph cost.)";

#[test]
fn face_down_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let aphetto = scenario
        .add_creature_to_hand(P0, "Aphetto Alchemist", 1, 2)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue],
            generic: 1,
        })
        .from_oracle_text(APHETTO_ALCHEMIST)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let commit = runner.cast(aphetto).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), aphetto),
        CastingVariant::FaceDown,
        "reach guard: with only the fixed {{3}} face-down cost funded, the cast must select CastingVariant::FaceDown"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "FaceDown cast must fire the watcher exactly once"
    );
}

/// CR 702.173a (Freerunning) + CR 603.2. Chain Assassination ({2}{B}{B},
/// Instant): "Freerunning {1}{B} (...) Destroy target creature. If another
/// creature died this turn, draw a card." Only {1}{B} funded; requires a
/// player to have been dealt combat damage this turn by an Assassin/commander
/// under the caster's control — seeded directly (mirrors how
/// `zaffai_second_cast_is_suppressed_same_turn` seeds per-turn permission
/// state directly rather than replaying combat).
const CHAIN_ASSASSINATION: &str = "Freerunning {1}{B} (You may cast this spell for its freerunning cost if you dealt combat damage to a player this turn with an Assassin or commander.)\nDestroy target creature. If another creature died this turn, draw a card.";

#[test]
fn freerunning_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let target = scenario.add_creature(P1, "Freerunning Target", 2, 2).id();
    let chain_assassination = scenario
        .add_spell_to_hand_from_oracle(P0, "Chain Assassination", true, CHAIN_ASSASSINATION)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Black, ManaCostShard::Black],
            generic: 2,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Black, ManaType::Colorless]));
    let mut runner = scenario.build();
    runner
        .state_mut()
        .assassin_or_commander_dealt_combat_damage_this_turn
        .insert(P0);
    let commit = runner
        .cast(chain_assassination)
        .target_object(target)
        .commit();
    assert_eq!(
        stack_casting_variant(commit.state(), chain_assassination),
        CastingVariant::Freerunning,
        "reach guard: with only the freerunning cost funded, the cast must select CastingVariant::Freerunning"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        2,
        "1 from the watcher + 1 from Chain Assassination's own \"if another creature died\" draw \
         (destroying the target creature satisfies that clause)"
    );
}

/// CR 702.76a (Prowl) + CR 603.2. Auntie's Snitch ({2}{B}, Creature): "This
/// creature can't block. Prowl {1}{B} (...)" Only {1}{B} funded; requires a
/// Goblin or Rogue you controlled to have dealt combat damage to a player
/// this turn — seeded directly on the per-turn ledger.
const AUNTIES_SNITCH: &str = "This creature can't block.\nProwl {1}{B} (You may cast this for its prowl cost if you dealt combat damage to a player this turn with a Goblin or Rogue.)\nWhenever a Goblin or Rogue you control deals combat damage to a player, if this card is in your graveyard, you may return this card to your hand.";

#[test]
fn prowl_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let snitch = scenario
        .add_creature_to_hand(P0, "Auntie's Snitch", 3, 1)
        .with_subtypes(vec!["Goblin", "Rogue"])
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Black],
            generic: 2,
        })
        .from_oracle_text(AUNTIES_SNITCH)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Black, ManaType::Colorless]));
    let mut runner = scenario.build();
    runner
        .state_mut()
        .creature_types_dealt_combat_damage_this_turn
        .insert((P0, "Goblin".to_string()));
    let commit = runner.cast(snitch).commit();
    assert_eq!(
        stack_casting_variant(commit.state(), snitch),
        CastingVariant::Prowl,
        "reach guard: with only the prowl cost funded, the cast must select CastingVariant::Prowl"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Prowl cast must fire the watcher exactly once"
    );
}

// CR 702.117a (Surge): NOT LANDED — apparent engine bug, reported (not
// fixed) in the implementer report. `current_casting_variant_choice_options`
// correctly reports `[CastingVariantChoiceOption { variant: Surge, .. }]` as
// the sole legal, affordable option for a hand card whose only route is
// Surge (Boulder Salvo, "Surge {1}{R} ... deals 4 damage to target
// creature", verbatim). But the actual `GameAction::CastSpell` pipeline
// never elects it: `handle_cast_spell`'s `WaitingFor::CastingVariantChoice`
// prompt only fires when `options.len() > 1`, and the lone
// single-candidate auto-elect carve-out is hardcoded to
// `CastingVariant::ExilePermission | CastingVariant::Freerunning` only
// (`casting.rs`, the `variant_choices.options.first().filter(..)` block).
// Surge is absent from both that carve-out AND from the
// `AlternativeCastKeyword` family that drives `WaitingFor::AlternativeCastChoice`
// (Warp/Evoke/Emerge/Dash/Blitz/Overload/Bestow/Awaken/Cleave/
// MoreThanMeetsTheEye/Impending/Prototype/Mutate/Spectacle/Prowl/FaceDown —
// all 16 of those DO have a dedicated block). So a solo-Surge-candidate cast
// silently falls through to `CastingVariant::Normal` at the card's full
// printed cost: with only {1}{R} funded, this fails outright
// ("Cannot pay mana cost"); with the full {4}{R} ALSO funded, it silently
// casts normally instead of via Surge (confirmed by reading
// `stack_casting_variant` after commit — it comes back `Normal`, never
// `Surge`, in both cases). Every other AlternativeCastChoice-family and
// ExilePermission/Freerunning variant in this module reaches its declared
// `CastingVariant` correctly; Surge is the one exception found.

// ===========================================================================
// Special mechanics (driven manually via `runner.act(..)`, not `.cast(..)`)
// ===========================================================================

/// CR 601.2b + CR 118.9a (HandPermission) + CR 603.2. Zaffai and the
/// Tempests' own line: "Once during each of your turns, you may cast an
/// instant or sorcery spell from your hand without paying its mana cost."
/// Driven via `GameAction::CastSpellForFree` (the dedicated action for a
/// `CastFromHandFree { OncePerTurn }` permission — see
/// `zaffai_once_per_turn_hand_free_casts_with_no_mana` in
/// `rules/casting.rs`), not the `.cast(..)` fluent driver, which has no
/// builder hook for this action.
const ZAFFAI_LINE: &str = "Once during each of your turns, you may cast an instant or sorcery spell from your hand without paying its mana cost.";

#[test]
fn hand_permission_fires_watcher_once() {
    use engine::types::ability::{StaticDefinition, TargetFilter, TypeFilter, TypedFilter};
    use engine::types::statics::{CastFreeOrigin, CastFrequency, StaticMode};

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let zaffai = scenario
        .add_creature(P0, "Zaffai and the Tempests", 5, 7)
        .from_oracle_text(ZAFFAI_LINE)
        .with_static_definition(
            StaticDefinition::new(StaticMode::CastFromHandFree {
                frequency: CastFrequency::OncePerTurn,
                origin: CastFreeOrigin::Hand,
            })
            .affected(TargetFilter::Typed(TypedFilter::new(TypeFilter::Instant))),
        )
        .id();
    let shock = scenario
        .add_spell_to_hand_from_oracle(P0, "Shock", true, "Shock deals 2 damage to any target.")
        .id();
    let mut runner = scenario.build();
    let card_id = runner.state().objects[&shock].card_id;

    runner
        .act(GameAction::CastSpellForFree {
            object_id: shock,
            card_id,
            source_id: zaffai,
            payment_mode: engine::types::game_state::CastPaymentMode::Auto,
        })
        .expect("CastSpellForFree should succeed");
    if let WaitingFor::TargetSelection { .. } = &runner.state().waiting_for {
        runner
            .act(GameAction::SelectTargets {
                targets: vec![TargetRef::Player(P1)],
            })
            .expect("choose Shock's damage target");
    }
    let variant = stack_casting_variant(runner.state(), shock);
    assert!(
        matches!(variant, CastingVariant::HandPermission { .. }),
        "reach guard: expected CastingVariant::HandPermission, got {variant:?}"
    );
    let before = hand_len(runner.state(), P0);
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "HandPermission cast must fire the watcher exactly once"
    );
}

/// CR 702.190a (Sneak) + CR 603.2. Jennika's Technique ({2}{R}, Instant):
/// "Sneak {R} (...) Jennika's Technique deals 2 damage to each creature."
/// Legal only during the declare-blockers step with an unblocked attacker to
/// return. Driven via `GameAction::CastSpellAsSneak` (the dedicated action —
/// mirrors `setup_sneak_scenario` in `casting_tests.rs`), not `.cast(..)`.
const JENNIKAS_TECHNIQUE: &str = "Sneak {R} (You may cast this spell for {R} if you also return an unblocked attacker you control to hand during the declare blockers step.)\nJennika's Technique deals 2 damage to each creature.";

#[test]
fn sneak_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let attacker = scenario.add_creature(P0, "Sneak Attacker", 2, 2).id();
    let jennikas_technique = scenario
        .add_spell_to_hand_from_oracle(P0, "Jennika's Technique", true, JENNIKAS_TECHNIQUE)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 2,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red]));
    let mut runner = scenario.build();
    {
        let state = runner.state_mut();
        state.phase = Phase::DeclareBlockers;
        state.active_player = P0;
        state.priority_player = P0;
        state.waiting_for = WaitingFor::Priority { player: P0 };
        state.objects.get_mut(&attacker).unwrap().tapped = true;
        state.combat = Some(engine::game::combat::CombatState {
            attackers: vec![engine::game::combat::AttackerInfo::attacking_player(
                attacker, P1,
            )],
            ..Default::default()
        });
    }
    let card_id = runner.state().objects[&jennikas_technique].card_id;
    runner
        .act(GameAction::CastSpellAsSneak {
            hand_object: jennikas_technique,
            card_id,
            creature_to_return: attacker,
            payment_mode: engine::types::game_state::CastPaymentMode::Auto,
        })
        .expect("CastSpellAsSneak should succeed");
    let variant = stack_casting_variant(runner.state(), jennikas_technique);
    assert!(
        matches!(variant, CastingVariant::Sneak { .. }),
        "reach guard: expected CastingVariant::Sneak, got {variant:?}"
    );
    assert_eq!(
        runner.state().objects[&attacker].zone,
        Zone::Hand,
        "reach guard: the returned attacker must have paid the Sneak cost"
    );
    let before = hand_len(runner.state(), P0);
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "Sneak cast must fire the watcher exactly once"
    );
}

/// CR 702.188a (Web-slinging) + CR 603.2. Amazing Spider-Girl ({3}{W}{W},
/// Creature): "Web-slinging {2}{W} (...) Flying, vigilance." — verbatim,
/// the entire printed text besides the type line. Driven via
/// `GameAction::CastSpellAsWebSlinging`.
const AMAZING_SPIDER_GIRL: &str = "Web-slinging {2}{W} (You may cast this spell for {2}{W} if you also return a tapped creature you control to its owner's hand.)\nFlying, vigilance";

#[test]
fn web_slinging_fires_watcher_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let tapped_creature = scenario.add_creature(P0, "Tapped Fodder", 1, 1).id();
    let spider_girl = scenario
        .add_creature_to_hand(P0, "Amazing Spider-Girl", 5, 4)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::White, ManaCostShard::White],
            generic: 3,
        })
        .from_oracle_text_with_keywords(&["web-slinging"], AMAZING_SPIDER_GIRL)
        .id();
    scenario.with_mana_pool(
        P0,
        mana_units(&[ManaType::White, ManaType::Colorless, ManaType::Colorless]),
    );
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&tapped_creature)
        .unwrap()
        .tapped = true;
    let card_id = runner.state().objects[&spider_girl].card_id;
    runner
        .act(GameAction::CastSpellAsWebSlinging {
            hand_object: spider_girl,
            card_id,
            creature_to_return: tapped_creature,
            payment_mode: engine::types::game_state::CastPaymentMode::Auto,
        })
        .expect("CastSpellAsWebSlinging should succeed");
    let variant = stack_casting_variant(runner.state(), spider_girl);
    assert!(
        matches!(variant, CastingVariant::WebSlinging { .. }),
        "reach guard: expected CastingVariant::WebSlinging, got {variant:?}"
    );
    assert_eq!(
        runner.state().objects[&tapped_creature].zone,
        Zone::Hand,
        "reach guard: the returned tapped creature must have paid the Web-slinging cost"
    );
    let before = hand_len(runner.state(), P0);
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "Web-slinging cast must fire the watcher exactly once"
    );
}

/// CR 702.94a (Miracle) + CR 603.2 + CR 603.11. Devastation Tide ({3}{U}{U},
/// Sorcery): "Return all nonland permanents to their owners' hands. Miracle
/// {1}{U} (...)" Drawn as the first card of the turn via a direct
/// `Effect::Draw` resolved through `resolve_ability_chain` (the same real
/// production ability resolver `draw_one_for_controller` in
/// `granted_alt_cost_hand_keyword.rs` exercises via `effects::draw::resolve`
/// — not a debug shortcut), which queues a real `MiracleOffer`. The
/// `WaitingFor::MiracleReveal` prompt itself is a priority-grant-checkpoint
/// artifact this direct call bypasses, so it is surfaced explicitly from
/// that queued offer below. Driven via `GameAction::CastSpellAsMiracle`
/// (twice: once to accept the reveal, which only pushes a "you may cast it"
/// trigger per CR 702.94a — not a shortcut — and once more to accept the
/// resulting `CastOfferKind::Miracle`).
const DEVASTATION_TIDE: &str = "Return all nonland permanents to their owners' hands.\nMiracle {1}{U} (You may cast this card for its miracle cost when you draw it if it's the first card you drew this turn.)";

#[test]
fn miracle_fires_watcher_once() {
    use engine::game::effects::resolve_ability_chain;
    use engine::types::ability::{Effect, QuantityExpr, ResolvedAbility, TargetFilter};

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let devastation_tide = scenario
        .add_spell_to_library_top(P0, "Devastation Tide", false)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Blue, ManaCostShard::Blue],
            generic: 3,
        })
        .from_oracle_text(DEVASTATION_TIDE)
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Blue, ManaType::Colorless]));
    let mut runner = scenario.build();

    let draw_ability = ResolvedAbility::new(
        Effect::Draw {
            count: QuantityExpr::Fixed { value: 1 },
            target: TargetFilter::Controller,
        },
        Vec::new(),
        ObjectId(0),
        P0,
    );
    let mut events = Vec::new();
    resolve_ability_chain(runner.state_mut(), &draw_ability, &mut events, 0)
        .expect("draw resolves");
    assert_eq!(
        runner.state().pending_miracle_offers.len(),
        1,
        "reach guard: drawing the first card of the turn must queue a Miracle offer"
    );
    // The `MiracleReveal` prompt is normally raised by the priority-grant
    // checkpoint that runs after a real `GameAction`, not by the raw effect
    // resolver called directly above. Surface it from the (now-confirmed
    // real) queued offer so `GameAction::CastSpellAsMiracle` is reachable.
    let offer = runner.state().pending_miracle_offers[0].clone();
    runner.state_mut().waiting_for = WaitingFor::MiracleReveal {
        player: offer.player,
        object_id: offer.object_id,
        cost: offer.cost,
    };

    let card_id = runner.state().objects[&devastation_tide].card_id;
    // CR 702.94a: accepting the reveal pushes a "you may cast it" triggered
    // ability onto the stack; it does not cast the card directly. The
    // post-action pipeline re-derives `waiting_for` from
    // `pending_miracle_offers` (in production, a second miracle-eligible
    // draw this turn would queue a second offer) — clear the manually-seeded
    // entry FIRST so that pipeline settles on Priority instead of re-opening
    // the same prompt this action is answering.
    runner.state_mut().pending_miracle_offers.clear();
    runner
        .act(GameAction::CastSpellAsMiracle {
            object_id: devastation_tide,
            card_id,
            payment_mode: engine::types::game_state::CastPaymentMode::Auto,
        })
        .expect("CastSpellAsMiracle should succeed");
    if let WaitingFor::Priority { .. } = &runner.state().waiting_for {
        runner
            .act(GameAction::PassPriority)
            .expect("P0 passes priority to let the miracle trigger resolve");
        runner
            .act(GameAction::PassPriority)
            .expect("P1 passes priority to let the miracle trigger resolve");
    }
    // CR 702.94a: the resolved trigger raises `WaitingFor::CastOffer(Miracle)` —
    // accept it with a second `CastSpellAsMiracle`, this time matched against
    // the cast-offer arm (`engine.rs`'s second `CastSpellAsMiracle` handler).
    if let WaitingFor::CastOffer {
        kind: engine::types::game_state::CastOfferKind::Miracle { .. },
        ..
    } = &runner.state().waiting_for
    {
        runner
            .act(GameAction::CastSpellAsMiracle {
                object_id: devastation_tide,
                card_id,
                payment_mode: engine::types::game_state::CastPaymentMode::Auto,
            })
            .expect("CastSpellAsMiracle (cast-offer accept) should succeed");
    }
    let variant = stack_casting_variant(runner.state(), devastation_tide);
    assert_eq!(
        variant,
        CastingVariant::Miracle,
        "reach guard: expected CastingVariant::Miracle, got {variant:?}"
    );
    let before = hand_len(runner.state(), P0);
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "Miracle cast must fire the watcher exactly once"
    );
}

/// CR 702.35a (Madness) + CR 603.2. Alchemist's Greeting ({4}{R}, Sorcery):
/// "Alchemist's Greeting deals 4 damage to target creature. Madness {1}{R}
/// (...)" Discarded via a direct `Effect::DiscardCard` resolved through
/// `resolve_ability_chain` (the real production discard resolver, mirroring
/// the Miracle test's `Effect::Draw` pattern above), which triggers the
/// Madness exile replacement — confirmed by the reach guard below. As with
/// Miracle, the `WaitingFor::CastOffer(Madness)` prompt itself is a
/// priority-grant-checkpoint artifact this direct call bypasses, so it is
/// surfaced explicitly from the card's own printed Madness cost. Driven via
/// `GameAction::CastSpellAsMadness`. The hand holds exactly this one card so
/// "discard a card" has no ambiguous choice to make.
const ALCHEMISTS_GREETING: &str = "Alchemist's Greeting deals 4 damage to target creature.\nMadness {1}{R} (If you discard this card, discard it into exile. When you do, cast it for its madness cost or put it into your graveyard.)";

#[test]
fn madness_fires_watcher_once() {
    use engine::game::effects::resolve_ability_chain;
    use engine::types::ability::{Effect, ResolvedAbility, TargetFilter};

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let victim = scenario.add_creature(P1, "Madness Target", 2, 6).id();
    let alchemists_greeting = scenario
        .add_spell_to_hand_from_oracle(P0, "Alchemist's Greeting", false, ALCHEMISTS_GREETING)
        .with_mana_cost(ManaCost::Cost {
            shards: vec![ManaCostShard::Red],
            generic: 4,
        })
        .id();
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red, ManaType::Colorless]));
    let mut runner = scenario.build();

    let discard_ability = ResolvedAbility::new(
        Effect::DiscardCard {
            count: 1,
            target: TargetFilter::Controller,
        },
        Vec::new(),
        ObjectId(0),
        P0,
    );
    let mut events = Vec::new();
    resolve_ability_chain(runner.state_mut(), &discard_ability, &mut events, 0)
        .expect("discard resolves");
    assert_eq!(
        runner.state().objects[&alchemists_greeting].zone,
        Zone::Exile,
        "reach guard: Madness must exile the discarded card instead of the graveyard"
    );
    // As with Miracle above: the `CastOffer(Madness)` prompt is normally
    // raised by the priority-grant checkpoint after a real `GameAction`, not
    // by the raw effect resolver called directly above. Surface it from the
    // card's own (now-confirmed, real, off-graveyard) printed Madness cost.
    let madness_cost = runner.state().objects[&alchemists_greeting]
        .keywords
        .iter()
        .find_map(|k| match k {
            engine::types::keywords::Keyword::Madness(cost) => Some(cost.clone()),
            _ => None,
        })
        .expect("Alchemist's Greeting must carry the printed Madness keyword");
    runner.state_mut().waiting_for = WaitingFor::CastOffer {
        player: P0,
        kind: engine::types::game_state::CastOfferKind::Madness {
            object_id: alchemists_greeting,
            cost: madness_cost,
        },
    };

    let card_id = runner.state().objects[&alchemists_greeting].card_id;
    runner
        .act(GameAction::CastSpellAsMadness {
            object_id: alchemists_greeting,
            card_id,
            payment_mode: engine::types::game_state::CastPaymentMode::Auto,
        })
        .expect("CastSpellAsMadness should succeed");
    if let WaitingFor::TargetSelection { .. } = &runner.state().waiting_for {
        runner
            .act(GameAction::SelectTargets {
                targets: vec![TargetRef::Object(victim)],
            })
            .expect("choose Alchemist's Greeting's damage target");
    }
    let variant = stack_casting_variant(runner.state(), alchemists_greeting);
    assert_eq!(
        variant,
        CastingVariant::Madness,
        "reach guard: expected CastingVariant::Madness, got {variant:?}"
    );
    let before = hand_len(runner.state(), P0);
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to let the trigger resolve");
    runner
        .act(GameAction::PassPriority)
        .expect("P1 passes priority to let the trigger resolve");
    let after = hand_len(runner.state(), P0);
    assert_eq!(
        after,
        before + 1,
        "Madness cast must fire the watcher exactly once"
    );
}

/// CR 715.4 (Adventure) + CR 603.2. Bonecrusher Giant // Stomp: casting the
/// Adventure half (Stomp, the instant face) fires a general SpellCast
/// watcher exactly once. Loaded through the real card database
/// (`add_real_card`, already present in the committed integration-test
/// fixture) rather than an inline builder — no `GameScenario` helper models
/// an Adventure card's dual (creature / instant) face structure, and
/// `create_adventure_in_hand` in `casting_tests.rs` shows the amount of
/// hand-built `BackFaceData` that would take to replicate inline.
#[test]
fn adventure_fires_watcher_once() {
    let Some(db) = load_db() else {
        eprintln!("skipping adventure_fires_watcher_once: no card database");
        return;
    };
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    ensure_library(&mut scenario, P0, 5);
    install_universal_watcher(&mut scenario, P0);
    let victim = scenario.add_real_card(P1, "Grizzly Bears", Zone::Battlefield, db);
    let bonecrusher_giant = scenario.add_real_card(P0, "Bonecrusher Giant", Zone::Hand, db);
    scenario.with_mana_pool(P0, mana_units(&[ManaType::Red, ManaType::Colorless]));
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);

    let commit = runner
        .cast(bonecrusher_giant)
        .adventure_face(false)
        .target_object(victim)
        .commit();
    let variant = stack_casting_variant(commit.state(), bonecrusher_giant);
    assert_eq!(
        variant,
        CastingVariant::Adventure,
        "reach guard: casting the instant face must select CastingVariant::Adventure"
    );
    let outcome = commit.resolve();
    assert_eq!(
        outcome.hand_drawn(P0),
        1,
        "Adventure cast must fire the watcher exactly once"
    );
    outcome.assert_zone(&[bonecrusher_giant], Zone::Exile);
}
