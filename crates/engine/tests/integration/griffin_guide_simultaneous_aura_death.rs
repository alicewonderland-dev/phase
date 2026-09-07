//! Regression: Griffin Guide's "When enchanted creature dies, create a 2/2
//! white Griffin creature token with flying" must still fire when its host is
//! a TOKEN that departs the battlefield in the same simultaneous event as the
//! Aura itself (CR 704.5m puts an unattached Aura into its owner's graveyard
//! the instant its host leaves).
//!
//! Oracle (verified against Scryfall, oracle_id
//! c5323a43-82de-4340-8578-b3ffcc66f8fa): "Enchant creature\nEnchanted
//! creature gets +2/+2 and has flying.\nWhen enchanted creature dies, create a
//! 2/2 white Griffin creature token with flying."
//!
//! Official ruling (2022-12-08): "If Griffin Guide and the enchanted creature
//! go to the graveyard at the same time, Griffin Guide's last ability will
//! trigger."
//!
//! Root cause (narrower than the initial "check_unattached_auras never links
//! co-departure" hypothesis this file started from — that hypothesis does not
//! survive contact with the actual behavior, see below): the ordinary
//! non-token case already works. `check_state_based_actions` ends every
//! fixpoint iteration by calling `zones::stamp_simultaneous_from_slice` over
//! the WHOLE iteration's event slice (`sba.rs` ~349-357), which retroactively
//! links every battlefield-origin departure produced in that iteration —
//! including a host whose death came from `check_creature_deaths` and an Aura
//! that only became unattached afterward via `check_unattached_auras`, in the
//! SAME iteration. That covers `griffin_guide_solo_host_death_creates_token`
//! below, which passed even before the fix in this PR.
//!
//! The gap is CR 704.5d: `check_token_cease_to_exist` runs BETWEEN the
//! unattached-Aura/Equipment sweeps and that final stamp (`sba.rs` line ~295,
//! before line ~349), and removes a dead token from `state.objects` entirely
//! (`zones::cease_object` — "not a zone change, no event emitted"). The old
//! `stamp_simultaneous_from_slice` / `departed_subset` filters read
//! `state.objects.get(id).is_some_and(|o| o.zone != Battlefield)` to decide
//! "did this really leave" — which reads a token that already ceased to exist
//! as `None`, i.e. as "still on the battlefield, don't include it." That
//! silently drops the token host out of the co-departure group the stamp is
//! building, so the token's own `ZoneChanged` record's `co_departed` never
//! gets its co-departing Aura added — the trigger admission arm at
//! `triggers.rs` ~4968-5067 scans exactly that field and finds it empty. A
//! non-token host stays in `state.objects` (just off the battlefield), so it
//! reads as departed correctly. The fix (`zones.rs`): `None` also means
//! departed — a still-on-battlefield object cannot be absent from
//! `state.objects`, so absence is unambiguous proof it left (and then, for a
//! token, ceased to exist).
//!
//! `griffin_guide_simultaneous_trade_both_hosts_die_together` isolated this:
//! P0's non-token attacker's Griffin Guide fired correctly even before the
//! fix; only P1's Griffin Guide (attached to the token blocker) failed —
//! confirming the bug is token-existence-filter-shaped, not
//! co-departure-linkage-shaped in general.
//! `griffin_guide_solo_token_host_death_still_creates_token` isolates the
//! same fact in the minimal single-host shape, and is the test that actually
//! flips (fails) when the `zones.rs` fix is reverted — the two non-token
//! tests keep passing either way, since the existing co-departure stamp
//! already covered a non-token host before this PR.
//!
//! Equipment/Fortification (CR 704.5n, `check_unattached_equipment`) do NOT
//! share this bug, despite sharing the "attachment becomes illegal when its
//! host leaves" shape with Auras: CR 704.5n leaves an unattached
//! Equipment/Fortification ON the battlefield (unlike CR 704.5m's Aura
//! graveyard), so its own dies-observing trigger is found by the ordinary
//! live-battlefield scan and never needs the co-departure LKI machinery this
//! bug lives in. `equipment_token_host_dies_still_creates_token` below is a
//! documented NEGATIVE result for that hypothesis (it passes with or without
//! the fix) — kept as coverage that directly answers "does
//! check_unattached_equipment have the same gap," not as a second
//! discriminating regression test.
//!
//! CR references:
//!   - CR 603.10a: An ability that triggers when a permanent leaves the
//!     battlefield uses the game state immediately before the event
//!     (last-known information) to determine if it triggers.
//!   - CR 704.5d: A token in a zone other than the battlefield ceases to
//!     exist.
//!   - CR 704.5m: An Aura attached to nothing legal is put into its owner's
//!     graveyard. This happens in the SAME state-based-action pass as its
//!     host's death, so the Aura's own departure and its host's departure are
//!     part of one simultaneous event.
//!
//! All tests below drive the death through the real SBA pipeline (marking
//! lethal damage, then a single `check_state_based_actions` call, or real
//! combat) rather than a manual zone move, so `check_creature_deaths` and
//! `check_unattached_auras`/`check_unattached_equipment` run in the same
//! fixpoint iteration exactly as they would in a real game — a manual
//! pre-move would put the host's `ZoneChanged` event outside the SBA call's
//! own event slice and misrepresent the bug.

use engine::game::effects::attach::attach_to;
use engine::game::layers::evaluate_layers;
use engine::game::sba::check_state_based_actions;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::trigger_index::reindex_object_triggers;
use engine::game::triggers::{drain_order_triggers_with_identity, process_triggers};
use engine::types::identifiers::ObjectId;
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use super::rules::run_combat;

const GRIFFIN_GUIDE_ORACLE: &str = "Enchant creature\n\
Enchanted creature gets +2/+2 and has flying.\n\
When enchanted creature dies, create a 2/2 white Griffin creature token with flying.";

/// Every Griffin token (by name) currently controlled by `controller`.
fn griffin_tokens(runner: &GameRunner, controller: PlayerId) -> Vec<ObjectId> {
    runner
        .state()
        .objects
        .values()
        .filter(|o| o.is_token && o.name.contains("Griffin") && o.controller == controller)
        .map(|o| o.id)
        .collect()
}

/// Reach-guard shared by both tests: Griffin Guide's dies trigger must have
/// actually parsed (not `Unimplemented`/`Unknown`) so a token count of zero
/// below is the bug under test, not a parser regression masquerading as one.
fn assert_griffin_guide_parses() {
    let parsed = engine::parser::oracle::parse_oracle_text(
        GRIFFIN_GUIDE_ORACLE,
        "Griffin Guide",
        &[],
        &["Enchantment".to_string()],
        &["Aura".to_string()],
    );
    assert!(
        !parsed.triggers.is_empty(),
        "reach guard: Griffin Guide must parse a dies trigger"
    );
    assert!(
        parsed.triggers.iter().all(|t| !matches!(
            t.execute.as_ref().map(|e| e.effect.as_ref()),
            Some(engine::types::ability::Effect::Unimplemented { .. })
        )),
        "reach guard: Griffin Guide's dies trigger must not be Unimplemented"
    );
}

fn build_griffin_guide(scenario: &mut GameScenario, controller: PlayerId) -> ObjectId {
    scenario
        .add_creature(controller, "Griffin Guide", 0, 0)
        .as_enchantment()
        .with_subtypes(vec!["Aura"])
        .from_oracle_text(GRIFFIN_GUIDE_ORACLE)
        .id()
}

/// A SINGLE, non-token Griffin-Guide-enchanted creature dies alone (its own
/// combat opponent survives) — the minimal case CR 704.5m + CR 603.10a must
/// handle. Characterization, not a regression test for the fix in this PR:
/// this already passed before the fix (the existing `stamp_simultaneous_from_
/// slice` co-departure link already covers a non-token host — see the module
/// doc). Kept as a positive baseline so `griffin_guide_solo_token_host_death_
/// still_creates_token` below has something to contrast against: same shape,
/// token host, and THAT one is the actual discriminating test.
#[test]
fn griffin_guide_solo_host_death_creates_token() {
    assert_griffin_guide_parses();

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);

    let host = scenario.add_creature(P0, "Solo Host", 2, 2).id();
    let aura = build_griffin_guide(&mut scenario, P0);

    let mut runner = scenario.build();

    attach_to(runner.state_mut(), aura, host);
    evaluate_layers(runner.state_mut());
    reindex_object_triggers(runner.state_mut(), aura);

    assert_eq!(
        runner.state().objects[&aura]
            .attached_to
            .and_then(|t| t.as_object()),
        Some(host),
        "precondition: Griffin Guide must be attached to the host"
    );

    // Lethal damage regardless of the Aura's +2/+2 buff amount.
    runner
        .state_mut()
        .objects
        .get_mut(&host)
        .unwrap()
        .damage_marked = 99;

    let mut sba_events = Vec::new();
    check_state_based_actions(runner.state_mut(), &mut sba_events);

    // Reach guard: the host actually died AND the Aura actually became
    // unattached and departed in the same SBA pass (CR 704.5m). If either
    // stayed on the battlefield, a zero token count downstream would be a
    // setup failure, not the bug under test.
    assert_eq!(
        runner.state().objects[&host].zone,
        Zone::Graveyard,
        "reach guard: host must die from lethal damage"
    );
    assert_eq!(
        runner.state().objects[&aura].zone,
        Zone::Graveyard,
        "reach guard: CR 704.5m must graveyard the now-unattached Aura in the same pass"
    );

    process_triggers(runner.state_mut(), &sba_events);
    drain_order_triggers_with_identity(runner.state_mut());
    runner.advance_until_stack_empty();

    let tokens = griffin_tokens(&runner, P0);
    assert_eq!(
        tokens.len(),
        1,
        "CR 603.10a: Griffin Guide's dies trigger must fire via last-known \
         information even though the Aura co-departed with its own host; \
         found tokens: {tokens:?}"
    );
}

/// Same shape as the test above, but the host is a TOKEN. This is the actual
/// discriminating regression test for this PR's fix: reverting the `zones.rs`
/// `is_none_or` change (back to `is_some_and`) makes this fail while the
/// non-token version above keeps passing — proving the bug is specifically
/// about `check_token_cease_to_exist` (CR 704.5d) racing the co-departure
/// stamp, not about co-departure linkage in general. See the module doc.
#[test]
fn griffin_guide_solo_token_host_death_still_creates_token() {
    assert_griffin_guide_parses();

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);

    let host = scenario.add_creature(P0, "Solo Token Host", 2, 2).id();
    let aura = build_griffin_guide(&mut scenario, P0);

    let mut runner = scenario.build();
    runner.state_mut().objects.get_mut(&host).unwrap().is_token = true;

    attach_to(runner.state_mut(), aura, host);
    evaluate_layers(runner.state_mut());
    reindex_object_triggers(runner.state_mut(), aura);

    assert_eq!(
        runner.state().objects[&aura]
            .attached_to
            .and_then(|t| t.as_object()),
        Some(host),
        "precondition: Griffin Guide must be attached to the token host"
    );

    runner
        .state_mut()
        .objects
        .get_mut(&host)
        .unwrap()
        .damage_marked = 99;

    let mut sba_events = Vec::new();
    check_state_based_actions(runner.state_mut(), &mut sba_events);

    // Reach guard: CR 704.5d may have already removed the dead token from
    // `state.objects` entirely by the time this assertion runs — its absence
    // is itself proof it died and left (see the module doc); a still-existing
    // object must be off the battlefield either way.
    assert!(
        runner
            .state()
            .objects
            .get(&host)
            .is_none_or(|o| o.zone != Zone::Battlefield),
        "reach guard: token host must die from lethal damage (or have ceased to exist)"
    );
    assert_eq!(
        runner.state().objects[&aura].zone,
        Zone::Graveyard,
        "reach guard: CR 704.5m must graveyard the now-unattached Aura in the same pass"
    );

    process_triggers(runner.state_mut(), &sba_events);
    drain_order_triggers_with_identity(runner.state_mut());
    runner.advance_until_stack_empty();

    let tokens = griffin_tokens(&runner, P0);
    assert_eq!(
        tokens.len(),
        1,
        "CR 603.10a + CR 704.5d: Griffin Guide's dies trigger must fire via \
         last-known information even though its token host ceased to exist \
         before the co-departure stamp ran; found tokens: {tokens:?}"
    );
}

/// The originally reported shape: two creatures, each enchanted by its own
/// Griffin Guide, trade in combat and die together. One host is a token
/// (P1's blocker), the other is not (P0's attacker) — this is deliberately
/// asymmetric so the fix cannot be mistaken for something token-specific.
#[test]
fn griffin_guide_simultaneous_trade_both_hosts_die_together() {
    assert_griffin_guide_parses();

    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);

    // P0's attacker: a real (non-token) permanent.
    let attacker = scenario.add_creature(P0, "Attacking Host", 2, 2).id();
    let attacker_aura = build_griffin_guide(&mut scenario, P0);

    // P1's blocker: a token permanent, to keep the token/non-token asymmetry
    // from the original report without treating it as load-bearing.
    let blocker = scenario.add_creature(P1, "Blocking Host", 2, 2).id();
    let blocker_aura = build_griffin_guide(&mut scenario, P1);

    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&blocker)
        .unwrap()
        .is_token = true;

    attach_to(runner.state_mut(), attacker_aura, attacker);
    attach_to(runner.state_mut(), blocker_aura, blocker);
    evaluate_layers(runner.state_mut());
    reindex_object_triggers(runner.state_mut(), attacker_aura);
    reindex_object_triggers(runner.state_mut(), blocker_aura);

    // Precondition: both Auras give +2/+2, so each 2/2 becomes a 4/4 —
    // symmetric combat damage (4 each) kills both simultaneously.
    assert_eq!(
        runner.state().objects[&attacker].toughness,
        Some(4),
        "precondition: attacker must be buffed to 4 toughness by its Griffin Guide"
    );
    assert_eq!(
        runner.state().objects[&blocker].toughness,
        Some(4),
        "precondition: blocker must be buffed to 4 toughness by its Griffin Guide"
    );

    run_combat(&mut runner, vec![attacker], vec![(blocker, attacker)]);
    runner.advance_until_stack_empty();

    // Reach guard: combat actually happened and both hosts + both Auras
    // genuinely departed the battlefield together (CR 704.5g lethal combat
    // damage on the creatures, CR 704.5m unattached-Aura sweep on the Auras,
    // all in the same simultaneous SBA event per CR 603.10a).
    assert_eq!(
        runner.state().objects[&attacker].zone,
        Zone::Graveyard,
        "reach guard: attacker must die from combat damage"
    );
    // CR 704.5d: a token that leaves the battlefield ceases to exist on the
    // very next SBA check — by the time combat has fully settled the token
    // blocker is gone from `state.objects` entirely, which is itself proof
    // it died and left (it never ceases to exist while still a battlefield
    // permanent).
    assert!(
        runner
            .state()
            .objects
            .get(&blocker)
            .is_none_or(|o| o.zone != Zone::Battlefield),
        "reach guard: token blocker must die from combat damage (or have already \
         ceased to exist per CR 704.5d, which implies the same thing)"
    );
    assert_eq!(
        runner.state().objects[&attacker_aura].zone,
        Zone::Graveyard,
        "reach guard: P0's Griffin Guide must co-depart with its host"
    );
    assert_eq!(
        runner.state().objects[&blocker_aura].zone,
        Zone::Graveyard,
        "reach guard: P1's Griffin Guide must co-depart with its host"
    );

    let p0_tokens = griffin_tokens(&runner, P0);
    let p1_tokens = griffin_tokens(&runner, P1);
    assert_eq!(
        p0_tokens.len(),
        1,
        "P0's Griffin Guide (non-token host) must still create its token on \
         simultaneous co-departure; found tokens: {p0_tokens:?}"
    );
    assert_eq!(
        p1_tokens.len(),
        1,
        "P1's Griffin Guide (token host) must still create its token on \
         simultaneous co-departure; found tokens: {p1_tokens:?}"
    );
}

/// Answers the brief's open question — does `check_unattached_equipment`
/// (CR 704.5n) share Griffin Guide's bug? — empirically: NO, and this test is
/// the record of that check, not a second discriminating regression test (it
/// passes identically with the `zones.rs` fix reverted). The real card
/// Dancing Sword has the same "when equipped creature dies" surface shape as
/// Griffin Guide, but CR 704.5n keeps an unattached Equipment/Fortification
/// ON the battlefield (unlike CR 704.5m's Aura graveyard) — so its dies
/// trigger is found by the ordinary live-battlefield scan, never reaches the
/// co-departure LKI arm this bug lives in, and needs no fix here. Fortification
/// shares `check_unattached_equipment` with Equipment (CR 704.5n names both
/// identically), so this same reasoning covers it without a separate
/// card-data instance.
///
/// Minimal synthetic Oracle text (not Dancing Sword's own, which wraps its
/// dies trigger in an optional self-transform orthogonal to this question)
/// isolates the mechanic being checked: a token host dying alone with an
/// Equipment attached.
const TEST_EQUIPMENT_ORACLE: &str =
    "Equipped creature gets +1/+0.\nWhen equipped creature dies, create a 1/1 white Spirit creature token.";

#[test]
fn equipment_token_host_dies_still_creates_token() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);

    let host = scenario.add_creature(P0, "Token Host", 2, 2).id();
    let equipment = scenario
        .add_artifact_from_oracle(P0, "Test Equipment", TEST_EQUIPMENT_ORACLE)
        .with_subtypes(vec!["Equipment"])
        .id();

    let mut runner = scenario.build();
    runner.state_mut().objects.get_mut(&host).unwrap().is_token = true;

    attach_to(runner.state_mut(), equipment, host);
    evaluate_layers(runner.state_mut());
    reindex_object_triggers(runner.state_mut(), equipment);

    assert_eq!(
        runner.state().objects[&equipment]
            .attached_to
            .and_then(|t| t.as_object()),
        Some(host),
        "precondition: Equipment must be attached to the token host"
    );

    runner
        .state_mut()
        .objects
        .get_mut(&host)
        .unwrap()
        .damage_marked = 99;

    let mut sba_events = Vec::new();
    check_state_based_actions(runner.state_mut(), &mut sba_events);

    // Reach guard: the token host must have either departed to a non-battlefield
    // zone or already ceased to exist per CR 704.5d (see the module doc); the
    // Equipment must have become unattached and moved off the battlefield in
    // the same SBA pass via CR 704.5n.
    assert!(
        runner
            .state()
            .objects
            .get(&host)
            .is_none_or(|o| o.zone != Zone::Battlefield),
        "reach guard: token host must die from lethal damage (or have ceased to exist)"
    );
    // CR 704.5n: unlike an Aura (CR 704.5m, graveyarded), an unattached
    // Equipment/Fortification "remains on the battlefield" — it just becomes
    // unattached in place.
    assert_eq!(
        runner.state().objects[&equipment].zone,
        Zone::Battlefield,
        "reach guard: CR 704.5n keeps the now-unattached Equipment on the battlefield"
    );
    assert!(
        runner.state().objects[&equipment].attached_to.is_none(),
        "reach guard: CR 704.5n must clear the Equipment's dangling attachment"
    );

    process_triggers(runner.state_mut(), &sba_events);
    drain_order_triggers_with_identity(runner.state_mut());
    runner.advance_until_stack_empty();

    let spirits: Vec<ObjectId> = runner
        .state()
        .objects
        .values()
        .filter(|o| o.is_token && o.name.contains("Spirit") && o.controller == P0)
        .map(|o| o.id)
        .collect();
    assert_eq!(
        spirits.len(),
        1,
        "CR 603.10a + CR 704.5n: Equipment's dies trigger must fire via \
         last-known information even though its token host ceased to exist \
         before the co-departure stamp ran; found tokens: {spirits:?}"
    );
}
