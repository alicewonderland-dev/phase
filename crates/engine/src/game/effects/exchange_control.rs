use crate::game::targeting;
use crate::types::ability::Duration;
use crate::types::ability::{
    ContinuousModification, Effect, EffectError, EffectKind, ResolvedAbility, TargetFilter,
    TargetRef,
};
use crate::types::events::GameEvent;
use crate::types::game_state::GameState;
use crate::types::identifiers::ObjectId;
use crate::types::zones::Zone;

/// CR 701.12a: Exchange control of two permanents, or a permanent and a spell
/// (CR 701.12a + CR 400.7a — see `control_is_exchangeable` below).
///
/// Object resolution for each slot:
/// - A context-ref filter (`SelfRef` — "this artifact and target …", Avarice
///   Totem / Eyes Everywhere / Phyrexian Infiltrator; `TriggeringSource` —
///   "that spell", Perplexing Chimera) → resolved through the single 4-tier
///   authority `targeting::resolved_targets`.
/// - Any other filter → consumed in order from `ability.targets`.
///
/// CR 701.12a: If the entire exchange can't be completed (missing object,
/// off-battlefield/off-stack), no part of the exchange occurs (all-or-nothing).
/// CR 701.12b: If both permanents are controlled by the same player, the
/// exchange effect does nothing.
pub fn resolve(
    state: &mut GameState,
    ability: &ResolvedAbility,
    events: &mut Vec<GameEvent>,
) -> Result<(), EffectError> {
    let Effect::ExchangeControl { target_a, target_b } = &ability.effect else {
        // Should not be reached: dispatcher in effects/mod.rs only routes
        // ExchangeControl variants here.
        return Ok(());
    };

    // Diagnostic: both slot filters being `Any` indicates either an
    // old-format `card-data.json` row that deserialised via the serde default,
    // or a parser gap. A bare `Any/Any` slot set plus a slot-less
    // `ability.targets` produces a silent no-op — flag it so regressions are
    // visible in logs rather than disappearing into the CR 701.12a
    // all-or-nothing branch.
    if matches!(target_a, TargetFilter::Any) && matches!(target_b, TargetFilter::Any) {
        tracing::warn!(
            source_id = ?ability.source_id,
            "ExchangeControl resolved with both target filters = Any — check for a parser gap"
        );
    }

    // Each non-context-ref slot consumes one TargetRef::Object from
    // ability.targets, in declaration order. Context-ref slots (SelfRef,
    // TriggeringSource) are resolved through `targeting::resolved_targets`.
    let mut object_targets = ability.targets.iter().filter_map(|t| match t {
        TargetRef::Object(id) => Some(*id),
        TargetRef::Player(_) => None,
    });
    // CR 608.2k + CR 608.2c: a context-ref slot surfaces no target and is bound at
    // resolution time by the single 4-tier authority `targeting::resolved_targets` —
    // its tier-1 short-circuit owns the resolution-local anaphors (`SelfRef`, and
    // with it the CR 400.7 `self_ref_is_current` check), and its pure-event-context
    // tier owns `TriggeringSource` AHEAD of the `ability.targets` tier, so per-slot
    // index discipline survives a mixed declared/context-ref pair. It delegates the
    // event tier to `targeting::resolve_event_context_target`; there is no second
    // resolver here.
    //
    // SCOPE OF THAT GUARANTEE: it holds for the filters `resolved_targets`
    // owns a tier for — `SelfRef`, `SourceOrPaired`, `CostPaidObject`,
    // `AmassedArmy`, `ParentTarget{,Slot}`, and the
    // `is_pure_event_context_filter` group (which covers `TriggeringSource`).
    // Of those, `SelfRef` and `TriggeringSource` are the only context refs the
    // corpus produces in an `ExchangeControl` slot. `is_context_ref()` admits
    // more than that, and any filter WITHOUT a tier falls through to
    // `resolved_targets`' terminal `ability.targets.clone()` — so it would
    // return the sibling slot's declared target and both slots would resolve
    // to the same object (CR 701.12b no-op). That is a latent shape, not a
    // reachable one; see the matching note in `ability_utils.rs`'s slot
    // builder. Adding a new context-ref filter to an ExchangeControl parse
    // means giving it a tier in `resolved_targets` first.
    // NOTE: `resolve_event_context_target` must NOT be called directly — it has no
    // `SelfRef` arm, so it would silently break the Avarice Totem / Eyes Everywhere /
    // Phyrexian Infiltrator class.
    let resolve_slot = |filter: &TargetFilter, iter: &mut dyn Iterator<Item = ObjectId>| {
        if !filter.is_context_ref() {
            return iter.next();
        }
        targeting::resolved_targets(ability, filter, state)
            .into_iter()
            .find_map(|t| match t {
                TargetRef::Object(id) => Some(id),
                // CR 701.12a: a player-valued ref cannot be an exchange subject —
                // the exchange can't be completed, so no part of it occurs.
                TargetRef::Player(_) => None,
            })
    };

    let Some(id_a) = resolve_slot(target_a, &mut object_targets) else {
        // CR 701.12a: Can't complete exchange — do nothing.
        events.push(GameEvent::EffectResolved {
            kind: EffectKind::ExchangeControl,
            source_id: ability.source_id,
            subject: None,
        });
        return Ok(());
    };
    let Some(id_b) = resolve_slot(target_b, &mut object_targets) else {
        events.push(GameEvent::EffectResolved {
            kind: EffectKind::ExchangeControl,
            source_id: ability.source_id,
            subject: None,
        });
        return Ok(());
    };

    // CR 701.12a + CR 400.7a: control of an object can be exchanged wherever
    // control is a meaningful characteristic — the battlefield (CR 110.2) and
    // the stack (CR 112.2, CR 109.4: "Only objects on the stack or on the
    // battlefield have a controller"). A SPELL subject is legal precisely
    // because CR 400.7a carries the control change through onto the permanent
    // that spell becomes, and CR 110.2b assigns that permanent's by-default
    // controller to the player who put the spell onto the stack. Any other zone
    // (an object that has already left the stack — countered in response)
    // cannot complete the exchange, so per CR 701.12a no part of it occurs.
    fn control_is_exchangeable(zone: Zone) -> bool {
        matches!(zone, Zone::Battlefield | Zone::Stack)
    }

    // CR 701.12a: Both objects must exist and be in an exchangeable zone. The
    // controller read below is what makes this depend on the stack seed
    // (`layers::evaluate_layers`'s CR 112.2 base + CR 613.1b re-derivation): for
    // a stack subject, `obj.controller` is origin-zone data before that seed and
    // the live, re-derived controller after.
    let (controller_a, controller_b) = {
        let Some(obj_a) = state.objects.get(&id_a) else {
            events.push(GameEvent::EffectResolved {
                kind: EffectKind::ExchangeControl,
                source_id: ability.source_id,
                subject: None,
            });
            return Ok(());
        };
        let Some(obj_b) = state.objects.get(&id_b) else {
            events.push(GameEvent::EffectResolved {
                kind: EffectKind::ExchangeControl,
                source_id: ability.source_id,
                subject: None,
            });
            return Ok(());
        };
        if !control_is_exchangeable(obj_a.zone) || !control_is_exchangeable(obj_b.zone) {
            events.push(GameEvent::EffectResolved {
                kind: EffectKind::ExchangeControl,
                source_id: ability.source_id,
                subject: None,
            });
            return Ok(());
        }
        (obj_a.controller, obj_b.controller)
    };

    // CR 701.12b: Same controller → no effect. CR 701.12b is written for two
    // PERMANENTS; the permanent-and-spell case rests on CR 701.12a's general
    // all-or-nothing principle plus CR 701.12b's same-controller principle —
    // there is no separate rule for a spell whose live controller already
    // matches the permanent's.
    if controller_a == controller_b {
        events.push(GameEvent::EffectResolved {
            kind: EffectKind::ExchangeControl,
            source_id: ability.source_id,
            subject: None,
        });
        return Ok(());
    }

    // CR 701.12a: Bidirectional control exchange via two transient continuous effects.
    // Object A gets controller_b, object B gets controller_a. Duration honours
    // the resolved ability (e.g. "until end of turn") with `Permanent` as the
    // default — mirrors `gain_control::resolve`.
    let duration = ability.duration.clone().unwrap_or(Duration::Permanent);
    state.add_transient_continuous_effect(
        ability.source_id,
        controller_b,
        duration.clone(),
        TargetFilter::SpecificObject { id: id_a },
        vec![ContinuousModification::ChangeController],
        None,
    );
    state.add_transient_continuous_effect(
        ability.source_id,
        controller_a,
        duration,
        TargetFilter::SpecificObject { id: id_b },
        vec![ContinuousModification::ChangeController],
        None,
    );

    events.push(GameEvent::EffectResolved {
        kind: EffectKind::ExchangeControl,
        source_id: ability.source_id,
        subject: None,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::zones::create_object;
    use crate::types::ability::{Effect, TargetRef};
    use crate::types::identifiers::{CardId, ObjectId};
    use crate::types::player::PlayerId;

    fn make_exchange_ability(target_a: ObjectId, target_b: ObjectId) -> ResolvedAbility {
        ResolvedAbility::new(
            Effect::ExchangeControl {
                target_a: TargetFilter::Any,
                target_b: TargetFilter::Any,
            },
            vec![TargetRef::Object(target_a), TargetRef::Object(target_b)],
            ObjectId(100),
            PlayerId(0),
        )
    }

    #[test]
    fn exchange_control_swaps_controllers() {
        let mut state = GameState::new_two_player(42);
        let obj_a = create_object(
            &mut state,
            CardId(1),
            PlayerId(0),
            "Bear".to_string(),
            Zone::Battlefield,
        );
        let obj_b = create_object(
            &mut state,
            CardId(2),
            PlayerId(1),
            "Wolf".to_string(),
            Zone::Battlefield,
        );

        let ability = make_exchange_ability(obj_a, obj_b);
        let mut events = Vec::new();

        resolve(&mut state, &ability, &mut events).unwrap();

        // Should create two transient continuous effects (bidirectional ChangeController)
        assert_eq!(state.transient_continuous_effects.len(), 2);

        // First effect: Object A gets controller_b (PlayerId(1))
        let tce_a = state
            .transient_continuous_effects
            .iter()
            .find(|e| e.affected == TargetFilter::SpecificObject { id: obj_a })
            .expect("Should have effect for obj_a");
        assert_eq!(tce_a.controller, PlayerId(1));
        assert_eq!(
            tce_a.modifications,
            vec![ContinuousModification::ChangeController]
        );

        // Second effect: Object B gets controller_a (PlayerId(0))
        let tce_b = state
            .transient_continuous_effects
            .iter()
            .find(|e| e.affected == TargetFilter::SpecificObject { id: obj_b })
            .expect("Should have effect for obj_b");
        assert_eq!(tce_b.controller, PlayerId(0));
    }

    #[test]
    fn exchange_control_same_controller_is_noop() {
        let mut state = GameState::new_two_player(42);
        let obj_a = create_object(
            &mut state,
            CardId(1),
            PlayerId(0),
            "Bear".to_string(),
            Zone::Battlefield,
        );
        let obj_b = create_object(
            &mut state,
            CardId(2),
            PlayerId(0),
            "Wolf".to_string(),
            Zone::Battlefield,
        );

        let ability = make_exchange_ability(obj_a, obj_b);
        let mut events = Vec::new();

        // CR 701.12b: Same controller → do nothing.
        resolve(&mut state, &ability, &mut events).unwrap();
        assert!(
            state.transient_continuous_effects.is_empty(),
            "Should create no transient effects for same-controller exchange"
        );
    }

    #[test]
    fn exchange_control_missing_target_is_noop() {
        let mut state = GameState::new_two_player(42);
        let obj_a = create_object(
            &mut state,
            CardId(1),
            PlayerId(0),
            "Bear".to_string(),
            Zone::Battlefield,
        );

        // CR 701.12a: One target missing → all-or-nothing, do nothing.
        let ability = make_exchange_ability(obj_a, ObjectId(999));
        let mut events = Vec::new();

        resolve(&mut state, &ability, &mut events).unwrap();
        assert!(state.transient_continuous_effects.is_empty());
    }

    #[test]
    fn exchange_control_fewer_than_two_targets() {
        let mut state = GameState::new_two_player(42);
        let obj_a = create_object(
            &mut state,
            CardId(1),
            PlayerId(0),
            "Bear".to_string(),
            Zone::Battlefield,
        );

        // Only one target — can't complete exchange.
        let ability = ResolvedAbility::new(
            Effect::ExchangeControl {
                target_a: TargetFilter::Any,
                target_b: TargetFilter::Any,
            },
            vec![TargetRef::Object(obj_a)],
            ObjectId(100),
            PlayerId(0),
        );
        let mut events = Vec::new();
        resolve(&mut state, &ability, &mut events).unwrap();
        assert!(state.transient_continuous_effects.is_empty());
    }

    /// CR 613.1b + CR 701.12a: End-to-end layer pipeline test. Resolves an
    /// exchange-control effect then runs `evaluate_layers` and asserts the two
    /// targets' `controller` fields are ACTUALLY swapped — not merely that
    /// transient effects exist. This is the regression guard for Bug B:
    /// previously both `ChangeController` effects read `source.controller`
    /// (the caster) and set both objects to the caster instead of swapping.
    #[test]
    fn exchange_control_layer_pipeline_actually_swaps_controllers() {
        let mut state = GameState::new_two_player(42);
        let obj_a = create_object(
            &mut state,
            CardId(1),
            PlayerId(0),
            "Bear".to_string(),
            Zone::Battlefield,
        );
        let obj_b = create_object(
            &mut state,
            CardId(2),
            PlayerId(1),
            "Wolf".to_string(),
            Zone::Battlefield,
        );
        // Source is controlled by PlayerId(0) (the caster) — deliberately chosen
        // to match the old buggy behaviour (source.controller == caster) so the
        // test would FAIL pre-fix (both objects would end up under PlayerId(0)).
        let source = create_object(
            &mut state,
            CardId(3),
            PlayerId(0),
            "Switcheroo".to_string(),
            Zone::Stack,
        );

        let ability = ResolvedAbility::new(
            Effect::ExchangeControl {
                target_a: TargetFilter::Any,
                target_b: TargetFilter::Any,
            },
            vec![TargetRef::Object(obj_a), TargetRef::Object(obj_b)],
            source,
            PlayerId(0),
        );
        let mut events = Vec::new();
        resolve(&mut state, &ability, &mut events).unwrap();

        // Run the layer pipeline (CR 613).
        crate::game::layers::evaluate_layers(&mut state);

        assert_eq!(
            state.objects.get(&obj_a).unwrap().controller,
            PlayerId(1),
            "obj_a should now be controlled by PlayerId(1) after swap"
        );
        assert_eq!(
            state.objects.get(&obj_b).unwrap().controller,
            PlayerId(0),
            "obj_b should now be controlled by PlayerId(0) after swap"
        );
    }
}
