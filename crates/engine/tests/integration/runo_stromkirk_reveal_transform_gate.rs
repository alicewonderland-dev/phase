//! Issue #8586: Runo Stromkirk // Krothuss, Lord of the Deep — verbatim Oracle
//! front face:
//!   "Flying
//!    When Runo enters, put up to one target creature card from your graveyard
//!    on top of your library.
//!    At the beginning of your upkeep, look at the top card of your library.
//!    You may reveal that card. If a creature card with mana value 6 or greater
//!    is revealed this way, transform Runo."
//!
//! THE DEFECT this file guards. `parse_if_revealed_card_type_conditional`
//! consumed `" card is revealed this way"` as a single `tag()`, leaving no slot
//! for the postnominal property. Runo's `"with mana value 6 or greater"` sits
//! exactly in that missing slot, so the head declined and the ENTIRE gate was
//! dropped (`condition: null` on the `Transform` node) — the upkeep trigger
//! turned Runo over regardless of what was on top of the library. CR 701.20a
//! (revealing) is the authorizing rule for the `"revealed this way"` anaphor;
//! CR 202.3 defines the mana value the gate bounds.
//!
//! MEASURED ROWS, at BASE (`3bc9531e7`) and after the parser fix:
//!
//! | row | shape | BASE | after the fix |
//! |---|---|---|---|
//! | `runo_declining_a_reveal_below_the_mana_value_gate_does_not_transform` | MV2 creature on top, DECLINE | `true` (wrong) | **`false`** |
//! | `runo_declining_a_reveal_of_a_noncreature_does_not_transform` | MV6 instant on top, DECLINE | `true` (wrong) | **`false`** |
//! | `runo_declining_a_qualifying_reveal_still_transforms_residual_defect` | MV6 creature on top, DECLINE | `true` | `true` (unchanged) |
//! | `runo_fixture_parses_to_a_transform_gated_on_the_mana_value_floor` | parse-only | condition `None` | `Some(Cmc { GE, 6 })` |
//!
//! REVERTING the parser change flips rows 1, 2 and 4. Row 3 is a labelled
//! characterization row and a live positive control — see its own doc comment.
//!
//! WHEN DW#5 LANDS, flipping row 3's `true` to `false` is NOT the only edit the
//! file needs. Once a DECLINED reveal stops writing the reveal ledger, reach
//! guard 2 (`last_revealed_ids.len() == 1`) fails on ALL THREE runtime rows and
//! rows 1 and 2 stop discriminating. That is deliberate — the file goes loudly
//! red rather than silently vacuous — but the DW#5 fixer must re-anchor that
//! guard on a signal the declined path still produces (or move these rows to
//! the ACCEPT side), not just edit row 3.
//!
//! TWO MEASURED FOOT-GUNS, both closed here on purpose:
//!   1. The fixture's card NAME must be exactly `"Runo Stromkirk"` — see the
//!      warning on `RUNO_ORACLE`. A misnamed fixture reports
//!      `transformed=false, answered=true, last_revealed=1` on BOTH sides: it
//!      looks like a healthy negative and measures nothing.
//!   2. The harness shape must start at `Phase::PreCombatMain` with Runo under
//!      **P1** and then `advance_to_phase(Phase::Upkeep)`. The phase TRANSITION
//!      is what fires the trigger, and the trigger carries `OnlyDuringYourTurn`,
//!      so a Runo under P0 in this shape is never offered the reveal at all.
//!
//! Every runtime row asserts four reach guards in the same test (the optional
//! was genuinely offered and answered, exactly one card was looked at, a
//! `back_face` is installed, and the object's name), so no negative can pass
//! vacuously.
//!
//! `DEFERRED(phase 2)`: the ACCEPT side is NOT covered here. `Effect::Transform`
//! still reads `ability.targets` positionally, so an accepted qualifying reveal
//! is a silent no-op — this phase makes the gate parse and be ENFORCED, it does
//! not make the transform happen. Phase 2 of this work extends this file with
//! the accept-side rows and with Delver of Secrets / Sidequest: Catch a Fish;
//! this file is deliberately NOT a complete description of the card yet.
//! `setup` and `seed_library_top` are parameterized on card name, oracle text,
//! top-card core type and mana value so that extension needs no rewrite of the
//! rows below.

use engine::game::game_object::BackFaceData;
use engine::game::scenario::{GameRunner, GameScenario, P1};
use engine::game::zones::create_object;
use engine::parser::oracle::parse_oracle_text;
use engine::types::ability::{
    AbilityCondition, AbilityDefinition, Comparator, Effect, FilterProp, QuantityExpr,
};
use engine::types::actions::GameAction;
use engine::types::card_type::{CardType, CoreType, Supertype};
use engine::types::game_state::{GameState, WaitingFor};
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::mana::ManaCost;
use engine::types::phase::Phase;
use engine::types::player::PlayerId;

/// Runo Stromkirk — verbatim Scryfall Oracle text of the FRONT face, including
/// the `Flying` and enters-the-battlefield lines.
///
/// ⚠ The card name passed to `from_oracle_text` MUST be exactly
/// `"Runo Stromkirk"`. `~` normalization runs once at the single parser entry
/// point (`parse_oracle_ir` calls `normalize_card_name_refs` before it splits
/// the text into lines), so any other name leaves `"transform Runo"`
/// unnormalized, **no `Effect::Transform` node enters the AST at all**, and
/// every row in this file reports `transformed=false, answered=true,
/// last_revealed=1` — it looks like a healthy negative and measures nothing.
/// The parse-level control
/// `runo_fixture_parses_to_a_transform_gated_on_the_mana_value_floor` exists to
/// catch exactly this.
const RUNO_ORACLE: &str = "Flying\nWhen Runo enters, put up to one target creature card from your graveyard on top of your library.\nAt the beginning of your upkeep, look at the top card of your library. You may reveal that card. If a creature card with mana value 6 or greater is revealed this way, transform Runo.";

/// Krothuss, Lord of the Deep — the back face, taken from the card corpus
/// (`card-data.json`: Legendary Creature — Kraken Horror, 3/5, Flying), built
/// on the `azors_gateway_transform_condition.rs::sanctum_of_the_sun_back_face`
/// template.
///
/// `layout_kind: Some(LayoutKind::Transform)` is set because that is what a
/// real double-faced card carries, NOT because any assertion depends on it:
/// the `TransformScope::Single` path does not consult it —
/// `is_double_faced_permanent` (`game/transform.rs`) gates only the `resolve_all`
/// scope. No test logic here may come to depend on that field.
fn krothuss_back_face() -> BackFaceData {
    BackFaceData {
        is_swap_snapshot: false,
        name: "Krothuss, Lord of the Deep".to_string(),
        power: Some(3),
        toughness: Some(5),
        loyalty: None,
        printed_loyalty: None,
        defense: None,
        card_types: CardType {
            supertypes: vec![Supertype::Legendary],
            core_types: vec![CoreType::Creature],
            subtypes: vec!["Kraken".to_string(), "Horror".to_string()],
        },
        mana_cost: ManaCost::default(),
        keywords: vec![],
        abilities: vec![],
        trigger_definitions: Default::default(),
        replacement_definitions: Default::default(),
        static_definitions: Default::default(),
        color: vec![],
        printed_ref: None,
        modal: None,
        additional_cost: None,
        strive_cost: None,
        casting_restrictions: vec![],
        casting_options: vec![],
        layout_kind: Some(engine::types::card::LayoutKind::Transform),
        parse_warnings: vec![],
    }
}

/// Put a card of a known core type and mana value on top of `player`'s library,
/// so the upkeep "look at the top card" step has a real input for BOTH legs of
/// `AbilityCondition::RevealedHasCardType`: the type leg (`card_types` via
/// `object_has_core_type`) and the property leg (`additional_filter` via
/// `matches_target_filter`). Both `card_types` and `base_card_types` are set —
/// the two legs read different fields.
fn seed_library_top(
    state: &mut GameState,
    player: PlayerId,
    name: &str,
    core_type: CoreType,
    mana_value: u32,
) -> ObjectId {
    let id = create_object(
        state,
        CardId(state.next_object_id),
        player,
        name.to_string(),
        engine::types::zones::Zone::Library,
    );
    let obj = state.objects.get_mut(&id).unwrap();
    obj.card_types.core_types = vec![core_type];
    obj.base_card_types.core_types = vec![core_type];
    obj.mana_cost = ManaCost::generic(mana_value);
    let player_state = state.players.iter_mut().find(|p| p.id == player).unwrap();
    player_state.library.retain(|&oid| oid != id);
    player_state.library.insert(0, id);
    id
}

/// Build the board and advance into the permanent's controller's own upkeep.
///
/// The permanent goes under **P1** and the scenario starts at
/// `Phase::PreCombatMain` so that `advance_to_phase(Phase::Upkeep)` crosses a
/// turn boundary into P1's turn: the phase TRANSITION is what fires an
/// "at the beginning of your upkeep" trigger, and this one carries
/// `TriggerConstraint::OnlyDuringYourTurn`.
///
/// Parameterized on card name, oracle text, top-card core type and mana value
/// (plus the back face) so a second and third fixture card can reuse it without
/// touching any row above.
fn setup(
    card_name: &str,
    oracle: &str,
    top_core: CoreType,
    top_mana_value: u32,
    back_face: BackFaceData,
) -> (GameRunner, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let subject = scenario
        .add_creature(P1, card_name, 1, 4)
        .from_oracle_text(oracle)
        .as_legendary()
        .id();
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&subject)
        .unwrap()
        .back_face = Some(back_face);
    seed_library_top(runner.state_mut(), P1, "Top Card", top_core, top_mana_value);
    runner.advance_to_phase(Phase::Upkeep);
    assert_eq!(
        runner.state().active_player,
        P1,
        "reach guard: the trigger is OnlyDuringYourTurn, so this must be P1's own upkeep"
    );
    (runner, subject)
}

/// Drain priority until the upkeep trigger's reveal offer appears, then answer
/// it with `accept`.
///
/// Returns whether the optional was offered AND successfully answered AND the
/// machine then left `OptionalEffectChoice` — the reach guard every row
/// asserts, negatives included (CR 608.2d: an effect's "you may" is a
/// resolution-time choice, so "did not transform" is only meaningful if the
/// choice was genuinely presented and answered).
///
/// ⚠ The three conjuncts are load-bearing; do NOT weaken this back to "was an
/// `OptionalEffectChoice` ever seen". If a later change makes
/// `DecideOptionalEffect` error at this seam, the machine stalls parked in
/// `OptionalEffectChoice` — and every OTHER reach guard still passes, because
/// `last_revealed_ids` was already written by the enclosing look step, the back
/// face is installed, the name is untouched and `transformed` is `false`. An
/// offered-only flag would let both negative rows go green while measuring
/// nothing at all.
fn drive_upkeep(runner: &mut GameRunner, accept: bool) -> bool {
    let mut answered = false;
    for _ in 0..20 {
        match &runner.state().waiting_for {
            WaitingFor::OptionalEffectChoice { .. } => {
                answered = runner
                    .act(GameAction::DecideOptionalEffect { accept })
                    .is_ok();
                if !answered {
                    break;
                }
            }
            WaitingFor::Priority { .. } => {
                if runner.act(GameAction::PassPriority).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
    answered
        && !matches!(
            runner.state().waiting_for,
            WaitingFor::OptionalEffectChoice { .. }
        )
}

/// Walk an ability chain (effect + sub_ability + else_ability) for the first
/// `Effect::Transform` node.
fn find_transform_node(def: &AbilityDefinition) -> Option<&AbilityDefinition> {
    if matches!(def.effect.as_ref(), Effect::Transform { .. }) {
        return Some(def);
    }
    def.sub_ability
        .as_deref()
        .and_then(find_transform_node)
        .or_else(|| def.else_ability.as_deref().and_then(find_transform_node))
}

/// Assert the four reach guards every runtime row shares. `expected_name` is
/// the name the object must carry AFTER the upkeep resolved — the front face
/// for a row that must not transform, the back face for one that must.
fn assert_reach_guards(
    runner: &GameRunner,
    subject: ObjectId,
    answered: bool,
    expected_name: &str,
) {
    assert!(
        answered,
        "reach guard (CR 608.2d): the reveal's OptionalEffectChoice must have been \
         offered and answered (and left behind), or this row measures nothing"
    );
    assert_eq!(
        runner.state().last_revealed_ids.len(),
        1,
        "reach guard: the look step must have produced exactly one card for the gate \
         to read; got {:?}",
        runner.state().last_revealed_ids
    );
    assert!(
        runner.state().objects[&subject].back_face.is_some(),
        "reach guard (CR 701.27a): a permanent with no back face cannot transform at \
         all, so a 'did not transform' assertion would be vacuous without this"
    );
    assert_eq!(
        runner.state().objects[&subject].name,
        expected_name,
        "reach guard: the object's name must agree with the transform assertion"
    );
}

/// DISCRIMINATOR #1. Top card is a **mana value 2 creature** — below Runo's
/// `mana value 6 or greater` floor — and the reveal is DECLINED.
///
/// Runo must NOT transform, and the assertion is permanently CR-correct: it is
/// over-determined by two independent reasons. Nothing was revealed at all
/// (CR 701.20a: to reveal a card is to show it to all players; a declined "you
/// may reveal" shows nothing), and independently the top card fails the gate on
/// its own terms (CR 202.3: mana value 2 is not 6 or greater).
///
/// Measured `true` (wrong) at BASE and `false` after the parser fix: at BASE
/// the postnominal property slot did not exist, the whole condition was dropped
/// to `null`, and the ungated `Transform` sub turned Runo over. This assertion
/// fails on revert.
#[test]
fn runo_declining_a_reveal_below_the_mana_value_gate_does_not_transform() {
    let (mut runner, runo) = setup(
        "Runo Stromkirk",
        RUNO_ORACLE,
        CoreType::Creature,
        2,
        krothuss_back_face(),
    );
    let answered = drive_upkeep(&mut runner, false);

    assert_reach_guards(&runner, runo, answered, "Runo Stromkirk");
    assert!(
        !runner.state().objects[&runo].transformed,
        "a mana-value-2 creature does not satisfy 'mana value 6 or greater' (CR 202.3), \
         and a declined reveal shows no card at all (CR 701.20a) — Runo must stay front-face up"
    );
}

/// DISCRIMINATOR #2. Top card is a **mana value 6 instant** — it clears the
/// mana-value floor but is not a creature card — and the reveal is DECLINED.
///
/// This row exercises the OTHER leg of the evaluator: `RevealedHasCardType`
/// checks `card_types` and `additional_filter` independently, so row 1 fails
/// the property leg while this one fails the type leg. Permanently CR-correct
/// for the same two independent reasons: nothing was revealed (CR 701.20a), and
/// an instant is not a creature card.
///
/// Measured `true` (wrong) at BASE and `false` after the parser fix. Fails on
/// revert.
#[test]
fn runo_declining_a_reveal_of_a_noncreature_does_not_transform() {
    let (mut runner, runo) = setup(
        "Runo Stromkirk",
        RUNO_ORACLE,
        CoreType::Instant,
        6,
        krothuss_back_face(),
    );
    let answered = drive_upkeep(&mut runner, false);

    assert_reach_guards(&runner, runo, answered, "Runo Stromkirk");
    assert!(
        !runner.state().objects[&runo].transformed,
        "an instant card is not a 'creature card' however large its mana value, and a \
         declined reveal shows no card at all (CR 701.20a) — Runo must stay front-face up"
    );
}

/// POSITIVE CONTROL (α) — **CHARACTERIZATION test. The outcome asserted below
/// is rules-INCORRECT and is asserted deliberately, as a record of current
/// behavior.**
///
/// Top card is a mana value 6 creature (it clears the gate on its own terms)
/// and the reveal is DECLINED. Current behavior transforms Runo anyway. Under
/// the rules it must not: CR 701.20a defines revealing as showing the card to
/// all players, and CR 608.2d makes the "you may reveal that card" a
/// resolution-time choice — a declined reveal shows no card, so the
/// `"…is revealed this way"` gate is unsatisfied and the transform should not
/// happen.
///
/// This assertion is EXPECTED TO BE FLIPPED by deferred work:
/// `DEFERRED(DW#5, Appendix A + PR body)` — DW#5, chartered in the run charter,
/// Appendix A; disclosed in the PR body under `Deferred / known-remaining`.
/// Whoever closes DW#5 should change `true` to `false` here and delete this
/// paragraph.
///
/// WHY IT IS HERE ANYWAY. It is the file's only LIVE positive control at this
/// seam: no Runo accept-side row transforms yet, so without it the two
/// discriminators above would be indistinguishable from a broken fixture. It
/// runs through the same `setup` and the same `drive_upkeep` as rows 1 and 2,
/// differing only in the top card's mana value, and so proves — live — that the
/// trigger fires, the optional is offered, `last_revealed_ids` is written, the
/// `Transform` node is reachable, and this object can in fact turn over. Note
/// the name reach guard is INVERTED here (back face, not front): a
/// same-direction copy-paste from rows 1 and 2 would make this row vacuous.
#[test]
fn runo_declining_a_qualifying_reveal_still_transforms_residual_defect() {
    let (mut runner, runo) = setup(
        "Runo Stromkirk",
        RUNO_ORACLE,
        CoreType::Creature,
        6,
        krothuss_back_face(),
    );
    let answered = drive_upkeep(&mut runner, false);

    assert_reach_guards(&runner, runo, answered, "Krothuss, Lord of the Deep");
    assert!(
        runner.state().objects[&runo].transformed,
        "CHARACTERIZATION (rules-INCORRECT, DW#5): current behavior transforms Runo on a \
         DECLINED reveal whose top card would have satisfied the gate. Per CR 701.20a + \
         CR 608.2d nothing was revealed, so this should be false"
    );
}

/// POSITIVE CONTROL (β) — parse-level fixture integrity, and a second
/// independent Phase-1 discriminator. Asserts nothing rules-incorrect, so
/// unlike (α) it is stable through every piece of deferred work.
///
/// It closes the measured fixture foot-gun by NAME: a fixture built under any
/// name other than `"Runo Stromkirk"` leaves `"transform Runo"` unnormalized
/// and drops the `Effect::Transform` node from the AST entirely, which the
/// runtime rows above would report as a healthy-looking negative.
///
/// THE `condition` ASSERTION BELOW IS LOAD-BEARING — DO NOT "SIMPLIFY" THIS
/// BACK TO A BARE `matches!(…, Effect::Transform { .. })`. That bare form is
/// `true` at BASE **and** `true` after the parser fix: it is not a
/// discriminator. Only the `Transform` node's `condition` moves, from `None` to
/// `RevealedHasCardType { [Creature], Cmc { GE, 6 } }` (CR 701.20a for the
/// `"revealed this way"` anaphor, CR 202.3 for the mana value it bounds).
#[test]
fn runo_fixture_parses_to_a_transform_gated_on_the_mana_value_floor() {
    let parsed = parse_oracle_text(RUNO_ORACLE, "Runo Stromkirk", &[], &[], &[]);
    let transform = parsed
        .triggers
        .iter()
        .filter_map(|t| t.execute.as_deref())
        .find_map(find_transform_node)
        .expect(
            "the fixture's parsed triggers must contain an Effect::Transform node — if this \
             fails, check that the card name is exactly \"Runo Stromkirk\" so `~` normalization \
             rewrites \"transform Runo\"",
        );

    let Some(AbilityCondition::RevealedHasCardType {
        card_types,
        additional_filter,
        ..
    }) = transform.condition.as_ref()
    else {
        panic!(
            "the Transform node must be gated by the revealed-card type condition; got {:?}",
            transform.condition
        );
    };
    assert_eq!(card_types, &vec![CoreType::Creature]);
    assert_eq!(
        additional_filter,
        &Some(FilterProp::Cmc {
            comparator: Comparator::GE,
            value: QuantityExpr::Fixed { value: 6 },
        }),
        "the 'with mana value 6 or greater' postnominal property must reach the gate"
    );
}
