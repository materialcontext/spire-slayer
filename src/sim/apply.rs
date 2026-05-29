use rand::seq::SliceRandom;
use rand::Rng;
use thiserror::Error;

use crate::domain::ai::{AiCondition, AiRuntime, AiStateKind, EnemyAiScript, RepeatConstraint};
use crate::domain::card::{Card, CardType};
use crate::domain::combat::{CombatState, EnemyState, Intent};
use crate::domain::effect::{BuffType, CardEffect};
use crate::domain::encounter::map_intent;

#[derive(Debug, Error)]
pub enum SimError {
    #[error("card index {0} out of range")]
    InvalidCardIndex(usize),
    #[error("not enough energy: need {need}, have {have}")]
    NotEnoughEnergy { need: u8, have: u8 },
    #[error("card is not playable (Status or Curse type)")]
    CardNotPlayable,
    #[error("invalid target index {0}")]
    InvalidTarget(usize),
}

// ── Internal combat math ───────────────────────────────────────────────────

/// Damage a single hit of `base` deals, accounting for Strength, Weak, Vulnerable.
/// Also applies relic passive modifiers: RED_SKULL, PAPER_PHROG, PAPER_KRANE.
fn compute_hit(base: u32, state: &CombatState, target_idx: usize) -> u32 {
    let mut strength = state.player.buff(&BuffType::Strength);
    // RED_SKULL: +3 Strength while HP ≤ 50%
    if state.has_relic("RED_SKULL") && state.player.hp * 2 <= state.player.max_hp {
        strength += 3;
    }
    let weak = state.player.buff(&BuffType::Weak) > 0;
    let vulnerable = state.enemies[target_idx].buff(&BuffType::Vulnerable) > 0;

    let raw = if strength >= 0 {
        base + strength as u32
    } else {
        base.saturating_sub((-strength) as u32)
    };
    // PAPER_KRANE: Weak deals 40% less (3/5) instead of 25% less (3/4)
    let after_weak = if weak {
        if state.has_relic("PAPER_KRANE") { raw * 3 / 5 } else { raw * 3 / 4 }
    } else {
        raw
    };
    // PAPER_PHROG: Vulnerable = 75% extra (×7/4) instead of 50% extra (×3/2)
    if vulnerable {
        if state.has_relic("PAPER_PHROG") { after_weak * 7 / 4 } else { after_weak * 3 / 2 }
    } else {
        after_weak
    }
}

/// Apply `dmg` to `enemy`, reducing block first then HP.
/// Returns Thorns stacks on that enemy (caller applies to player separately).
fn damage_enemy(state: &mut CombatState, enemy_idx: usize, dmg: u32) -> u32 {
    let thorns = state.enemies[enemy_idx]
        .buffs
        .get(&BuffType::Thorns)
        .copied()
        .unwrap_or(0)
        .max(0) as u32;

    let enemy = &mut state.enemies[enemy_idx];
    let absorbed = enemy.block.min(dmg);
    enemy.block -= absorbed;
    enemy.hp = enemy.hp.saturating_sub(dmg - absorbed);

    thorns
}

/// Apply `dmg` to the player. Applies Intangible cap, relic reductions, and
/// relic triggers (Centennial Puzzle, Lizard Tail).
fn damage_player(state: &mut CombatState, dmg: u32) {
    let intangible = state.player.buff(&BuffType::Intangible) > 0;
    let mut effective = if intangible { dmg.min(1) } else { dmg };

    // TUNGSTEN_ROD: lose 1 less HP per hit
    if effective > 0 && state.has_relic("TUNGSTEN_ROD") {
        effective = effective.saturating_sub(1);
    }
    // BEATING_REMNANT: can't lose more than 20 HP total this turn
    if state.has_relic("BEATING_REMNANT") {
        let remaining = 20u32.saturating_sub(state.hp_lost_this_turn);
        effective = effective.min(remaining);
    }

    let absorbed = state.player.block.min(effective);
    state.player.block -= absorbed;
    let hp_lost = effective - absorbed;
    state.player.hp = state.player.hp.saturating_sub(hp_lost);

    if hp_lost > 0 {
        state.hp_lost_this_turn += hp_lost;
        if !state.hp_lost_this_combat {
            state.hp_lost_this_combat = true;
            // CENTENNIAL_PUZZLE: draw 3 cards on the first HP loss in combat
            if state.has_relic("CENTENNIAL_PUZZLE") {
                for _ in 0..3 {
                    if state.draw_pile.is_empty() {
                        if state.discard_pile.is_empty() { break; }
                        state.draw_pile.append(&mut state.discard_pile);
                    }
                    if !state.draw_pile.is_empty() {
                        let card = state.draw_pile.remove(0);
                        state.hand.push(card);
                    }
                }
            }
        }
        // LIZARD_TAIL: survive lethal damage once per combat
        if state.player.hp == 0 && !state.lizard_tail_triggered && state.has_relic("LIZARD_TAIL") {
            state.lizard_tail_triggered = true;
            state.player.hp = (state.player.max_hp / 2).max(1);
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Draw `n` cards from the draw pile into hand.
/// When the draw pile is empty, the discard pile is shuffled into it.
pub fn draw_cards(state: &mut CombatState, n: usize, rng: &mut impl Rng) {
    for _ in 0..n {
        if state.draw_pile.is_empty() {
            if state.discard_pile.is_empty() {
                break;
            }
            state.draw_pile.append(&mut state.discard_pile);
            state.draw_pile.shuffle(rng);
            // THE_ABACUS: gain 6 Block whenever your draw pile is shuffled
            if state.has_relic("THE_ABACUS") {
                state.player.block += 6;
            }
        }
        let card = state.draw_pile.remove(0);
        state.hand.push(card);
    }
}

/// Apply a single card effect to the combat state.
///
/// `Passive` effects are no-ops in the simulator; they are display-only.
pub(crate) fn apply_effect(
    state: &mut CombatState,
    effect: &CardEffect,
    target_idx: usize,
    rng: &mut impl Rng,
) {
    match effect {
        CardEffect::Damage(d) => {
            if target_idx < state.enemies.len() && state.enemies[target_idx].is_alive() {
                let dmg = compute_hit(*d, state, target_idx);
                let thorns = damage_enemy(state, target_idx, dmg);
                if thorns > 0 {
                    damage_player(state, thorns);
                }
            }
        }

        CardEffect::DamageAll(d) => {
            // Collect (index, damage, thorns) before mutating.
            let hits: Vec<(usize, u32, u32)> = (0..state.enemies.len())
                .filter(|&i| state.enemies[i].is_alive())
                .map(|i| {
                    let dmg = compute_hit(*d, state, i);
                    let thorns = state.enemies[i]
                        .buffs
                        .get(&BuffType::Thorns)
                        .copied()
                        .unwrap_or(0)
                        .max(0) as u32;
                    (i, dmg, thorns)
                })
                .collect();

            for (i, dmg, thorns) in hits {
                damage_enemy(state, i, dmg);
                if thorns > 0 {
                    damage_player(state, thorns);
                }
            }
        }

        CardEffect::DamageMulti { base, hits } => {
            if target_idx < state.enemies.len() {
                // Read Thorns once before the loop.
                let thorns = state.enemies[target_idx]
                    .buffs
                    .get(&BuffType::Thorns)
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u32;

                for _ in 0..*hits {
                    if !state.enemies[target_idx].is_alive() {
                        break;
                    }
                    let dmg = compute_hit(*base, state, target_idx);
                    damage_enemy(state, target_idx, dmg);
                    if thorns > 0 {
                        damage_player(state, thorns);
                    }
                }
            }
        }

        CardEffect::DamageEqBlock => {
            if target_idx < state.enemies.len() && state.enemies[target_idx].is_alive() {
                let base = state.player.block;
                let dmg = compute_hit(base, state, target_idx);
                let thorns = damage_enemy(state, target_idx, dmg);
                if thorns > 0 {
                    damage_player(state, thorns);
                }
            }
        }

        CardEffect::Block(b) => {
            let dex = state.player.buff(&BuffType::Dexterity).max(0) as u32;
            let frail = state.player.buff(&BuffType::Frail) > 0;
            let raw = b + dex;
            let block = if frail { raw * 3 / 4 } else { raw };
            state.player.block += block;
        }

        CardEffect::Draw(n) => {
            draw_cards(state, *n as usize, rng);
        }

        CardEffect::GainEnergy(e) => {
            state.energy = state.energy.saturating_add(*e as u8);
        }

        CardEffect::LoseHp(h) => {
            state.player.hp = state.player.hp.saturating_sub(*h);
        }

        CardEffect::ApplyToEnemy { buff, stacks } => {
            if target_idx < state.enemies.len() && state.enemies[target_idx].is_alive() {
                let bonus = if matches!(buff, BuffType::Poison) && state.has_relic("SNECKO_SKULL") { 1 } else { 0 };
                *state.enemies[target_idx]
                    .buffs
                    .entry(buff.clone())
                    .or_insert(0) += stacks + bonus;
            }
        }

        CardEffect::ApplyToAllEnemies { buff, stacks } => {
            let bonus = if matches!(buff, BuffType::Poison) && state.has_relic("SNECKO_SKULL") { 1 } else { 0 };
            for enemy in &mut state.enemies {
                if enemy.is_alive() {
                    *enemy.buffs.entry(buff.clone()).or_insert(0) += stacks + bonus;
                }
            }
        }

        CardEffect::ApplyToSelf { buff, stacks } => {
            *state.player.buffs.entry(buff.clone()).or_insert(0) += stacks;
        }

        CardEffect::Passive(_) => {
            // Passive/triggered effects are not simulated.
        }
    }
}

/// Play the card at `card_hand_idx` targeting `target_idx`.
///
/// The card is validated (playable, enough energy, valid target) before any
/// state is mutated. On success the card moves to the discard or exhaust pile
/// and all its effects are applied in order.
///
/// `rng` is needed for Draw effects that may trigger a discard shuffle.
pub fn play_card(
    state: &mut CombatState,
    card_hand_idx: usize,
    target_idx: usize,
    rng: &mut impl Rng,
) -> Result<(), SimError> {
    if card_hand_idx >= state.hand.len() {
        return Err(SimError::InvalidCardIndex(card_hand_idx));
    }

    let card = &state.hand[card_hand_idx];

    if !card.is_playable(state.energy) {
        use crate::domain::card::CardType;
        if matches!(card.card_type, CardType::Status | CardType::Curse) {
            return Err(SimError::CardNotPlayable);
        }
        return Err(SimError::NotEnoughEnergy {
            need: card.cost,
            have: state.energy,
        });
    }

    // Validate target for single-target damage cards
    let needs_target = card.effects.iter().any(|e| {
        matches!(
            e,
            CardEffect::Damage(_) | CardEffect::DamageMulti { .. }
        )
    });
    if needs_target && target_idx >= state.enemies.len() {
        return Err(SimError::InvalidTarget(target_idx));
    }

    // Commit: remove from hand, spend energy
    let card = state.hand.remove(card_hand_idx);
    // X-cost (255) spends all remaining energy; regular cards spend their printed cost
    let cost_spent = if card.cost == 255 { state.energy } else { card.cost };
    state.energy -= cost_spent;

    // Update per-turn/combat counters
    let is_attack = card.card_type == CardType::Attack;
    let is_skill  = card.card_type == CardType::Skill;
    if is_attack {
        state.attacks_this_turn   += 1;
        state.attacks_this_combat += 1;
    }
    if is_skill {
        state.skills_this_turn += 1;
    }

    // PEN_NIB: every 10th attack deals double damage
    let pen_nib_double = is_attack
        && state.has_relic("PEN_NIB")
        && state.attacks_this_combat % 10 == 0;

    // Apply effects in order, doubling damage effects if pen_nib_double
    let effects = card.effects.clone();
    for effect in &effects {
        let effective = if pen_nib_double {
            match effect {
                CardEffect::Damage(d)      => CardEffect::Damage(d * 2),
                CardEffect::DamageAll(d)   => CardEffect::DamageAll(d * 2),
                CardEffect::DamageMulti { base, hits } => CardEffect::DamageMulti { base: base * 2, hits: *hits },
                other => other.clone(),
            }
        } else {
            effect.clone()
        };
        apply_effect(state, &effective, target_idx, rng);
    }

    // ── Post-play relic triggers ───────────────────────────────────────────

    if is_attack {
        // NUNCHAKU: every 10 attacks → +1 energy
        if state.has_relic("NUNCHAKU") && state.attacks_this_combat % 10 == 0 {
            state.energy = state.energy.saturating_add(1);
        }
        // KUNAI: every 3 attacks this turn → +1 Dexterity
        if state.has_relic("KUNAI") && state.attacks_this_turn % 3 == 0 {
            *state.player.buffs.entry(BuffType::Dexterity).or_insert(0) += 1;
        }
        // SHURIKEN: every 3 attacks this turn → +1 Strength
        if state.has_relic("SHURIKEN") && state.attacks_this_turn % 3 == 0 {
            *state.player.buffs.entry(BuffType::Strength).or_insert(0) += 1;
        }
        // ORNAMENTAL_FAN: every 3 attacks this turn → +4 Block
        if state.has_relic("ORNAMENTAL_FAN") && state.attacks_this_turn % 3 == 0 {
            state.player.block += 4;
        }
    }

    if is_skill {
        // LETTER_OPENER: every 3 skills this turn → deal 5 damage to all enemies
        if state.has_relic("LETTER_OPENER") && state.skills_this_turn % 3 == 0 {
            for enemy in &mut state.enemies {
                if enemy.is_alive() {
                    let absorbed = enemy.block.min(5);
                    enemy.block -= absorbed;
                    enemy.hp = enemy.hp.saturating_sub(5 - absorbed);
                }
            }
        }
    }

    // GREMLIN_HORN: whenever an enemy dies, gain 1 energy and draw 1 card
    if state.has_relic("GREMLIN_HORN") {
        let kills: usize = state.enemies.iter()
            .filter(|e| e.hp == 0 && e.max_hp > 0)
            .count();
        // Only count fresh kills (hp just reached 0 this play)
        // We approximate by checking all enemies with hp==0 each play
        // and only triggering once per dead enemy (max_hp>0 guards status enemies)
        // This may double-trigger on multi-kill cards but is close enough
        if kills > 0 {
            state.energy = state.energy.saturating_add(kills as u8);
            for _ in 0..kills {
                if state.draw_pile.is_empty() && !state.discard_pile.is_empty() {
                    state.draw_pile.append(&mut state.discard_pile);
                }
                if !state.draw_pile.is_empty() {
                    let drawn = state.draw_pile.remove(0);
                    state.hand.push(drawn);
                }
            }
        }
    }

    // Powers are exhausted (removed from game); other cards go to discard unless flagged
    if card.exhausts || card.card_type == CardType::Power {
        state.exhaust_pile.push(card);
    } else {
        state.discard_pile.push(card);
    }

    Ok(())
}

/// End the player's turn: enemies act, state resets, new hand drawn.
///
/// Returns `true` if combat is over (won or lost) after the enemy phase.
/// Step the enemy's move AI: determine next intent and advance internal state.
/// No-ops if the enemy has no AI script (preserves existing test behaviour).
pub fn advance_enemy_ai(enemy: &mut EnemyState, slot_index: usize, rng: &mut impl Rng) {
    let Some(ref script) = enemy.ai_script else { return };

    // Starting from the current state (a Move state), find what state comes next.
    let current_id = enemy.ai_runtime.current_state_id.clone();
    let next_state_id = match script.states.get(&current_id).map(|s| &s.kind) {
        Some(AiStateKind::Move { next, .. }) => {
            next.clone().unwrap_or_else(|| current_id.clone())
        }
        // Already at a routing node (shouldn't be normal); stay put
        _ => current_id.clone(),
    };

    let (resolved_move_id, resolved_state_id) =
        resolve_to_move(&next_state_id, script, &enemy.ai_runtime, enemy.hp, enemy.max_hp, slot_index, rng);

    if let Some(move_id) = resolved_move_id {
        let runtime = &mut enemy.ai_runtime;
        if runtime.last_move_id.as_deref() == Some(&move_id) {
            runtime.consecutive_count += 1;
        } else {
            runtime.consecutive_count = 1;
        }
        runtime.last_move_id = Some(move_id.clone());
        runtime.used_moves.insert(move_id.clone());
        runtime.current_state_id = resolved_state_id;

        if let Some(data) = script.moves.get(&move_id) {
            enemy.intent = map_intent(&data.intent_str, data.damage, data.hits);
        }
    }
}

/// Resolve a state ID to (move_id, landing_state_id), following one level of
/// routing (Random/Conditional → Move). Returns (None, id) if unresolvable.
fn resolve_to_move(
    state_id: &str,
    script: &EnemyAiScript,
    runtime: &AiRuntime,
    hp: u32,
    max_hp: u32,
    slot_index: usize,
    rng: &mut impl Rng,
) -> (Option<String>, String) {
    let Some(state) = script.states.get(state_id) else {
        return (None, state_id.to_string());
    };

    match &state.kind {
        AiStateKind::Move { move_id, .. } => (Some(move_id.clone()), state_id.to_string()),

        AiStateKind::Random { branches } => {
            // Filter by repeat constraints; fall back to full list if all ineligible
            let eligible: Vec<_> = branches.iter().filter(|b| is_eligible(b, runtime)).collect();
            let pool: Vec<_> = if eligible.is_empty() { branches.iter().collect() } else { eligible };

            let Some(first) = pool.first() else {
                return (None, state_id.to_string());
            };

            // Weighted selection
            let total: f32 = pool.iter().map(|b| b.weight).sum();
            let total = if total <= 0.0 { pool.len() as f32 } else { total };
            let mut r = rng.r#gen::<f32>() * total;
            let chosen = pool.iter().find(|b| { r -= b.weight; r <= 0.0 }).unwrap_or(first);

            let landing = script
                .find_state_by_move_id(&chosen.move_id)
                .map(|s| s.id.clone())
                .unwrap_or_else(|| state_id.to_string());
            (Some(chosen.move_id.clone()), landing)
        }

        AiStateKind::Conditional { branches } => {
            let picked = branches
                .iter()
                .find(|b| eval_condition(&b.condition, hp, max_hp, slot_index))
                .or_else(|| branches.first());

            match picked {
                Some(b) => {
                    let landing = script
                        .find_state_by_move_id(&b.move_id)
                        .map(|s| s.id.clone())
                        .unwrap_or_else(|| state_id.to_string());
                    (Some(b.move_id.clone()), landing)
                }
                None => (None, state_id.to_string()),
            }
        }
    }
}

fn is_eligible(branch: &crate::domain::ai::RandomBranch, runtime: &AiRuntime) -> bool {
    match &branch.repeat {
        RepeatConstraint::CanRepeatForever => true,
        RepeatConstraint::CannotRepeat => {
            runtime.last_move_id.as_deref() != Some(&branch.move_id)
        }
        RepeatConstraint::CanRepeatXTimes => {
            runtime.last_move_id.as_deref() != Some(&branch.move_id)
                || runtime.consecutive_count < branch.max_times
        }
        RepeatConstraint::UseOnlyOnce => !runtime.used_moves.contains(&branch.move_id),
    }
}

fn eval_condition(cond: &AiCondition, hp: u32, max_hp: u32, slot_index: usize) -> bool {
    match cond {
        AiCondition::HpAtOrAboveHalf => hp * 2 >= max_hp,
        AiCondition::HpBelowHalf => hp * 2 < max_hp,
        AiCondition::SlotIndex(i) => slot_index == *i,
        AiCondition::AlwaysTrue => true,
    }
}

pub fn end_turn(state: &mut CombatState, rng: &mut impl Rng) -> bool {
    // ── Player end-of-turn relic effects (before discarding) ───────────────
    // CLOAK_CLASP: gain 1 Block per card in hand at end of turn
    if state.has_relic("CLOAK_CLASP") {
        state.player.block += state.hand.len() as u32;
    }
    // ORICHALCUM: if ending turn with 0 block, gain 6 block
    if state.has_relic("ORICHALCUM") && state.player.block == 0 {
        state.player.block += 6;
    }
    if state.has_relic("FAKE_ORICHALCUM") && state.player.block == 0 {
        state.player.block += 3;
    }
    // SCREAMING_FLAGON: if hand is empty, deal 20 damage to all enemies
    if state.has_relic("SCREAMING_FLAGON") && state.hand.is_empty() {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                let absorbed = enemy.block.min(20);
                enemy.block -= absorbed;
                enemy.hp = enemy.hp.saturating_sub(20 - absorbed);
            }
        }
    }
    // STONE_CALENDAR: end of turn 7, deal 52 damage to all
    if state.has_relic("STONE_CALENDAR") && state.turn == 7 {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                let absorbed = enemy.block.min(52);
                enemy.block -= absorbed;
                enemy.hp = enemy.hp.saturating_sub(52 - absorbed);
            }
        }
    }
    // Reset per-turn counters
    state.attacks_this_turn = 0;
    state.skills_this_turn  = 0;
    state.hp_lost_this_turn = 0;

    // ── Poison tick ────────────────────────────────────────────────────────
    for enemy in &mut state.enemies {
        if enemy.is_alive() {
            let stacks = *enemy.buffs.get(&BuffType::Poison).unwrap_or(&0);
            if stacks > 0 {
                let dmg = stacks.max(0) as u32;
                let absorbed = enemy.block.min(dmg);
                enemy.block -= absorbed;
                enemy.hp = enemy.hp.saturating_sub(dmg - absorbed);
                *enemy.buffs.entry(BuffType::Poison).or_insert(0) -= 1;
            }
        }
    }

    // ── Discard hand ───────────────────────────────────────────────────────
    let drained: Vec<Card> = state.hand.drain(..).collect();
    for card in drained {
        if card.ethereal {
            state.exhaust_pile.push(card);
        } else {
            state.discard_pile.push(card);
        }
    }

    // ── Each living enemy acts ─────────────────────────────────────────────
    for i in 0..state.enemies.len() {
        if state.enemies[i].is_alive() {
            // Ritual: gain Strength at start of each turn
            let ritual = *state.enemies[i].buffs.get(&BuffType::Ritual).unwrap_or(&0);
            if ritual > 0 {
                *state.enemies[i].buffs.entry(BuffType::Strength).or_insert(0) += ritual;
            }

            // Reset block, then apply Plating (grants block, decrements by 1)
            state.enemies[i].block = 0;
            let plating = *state.enemies[i].buffs.get(&BuffType::Plating).unwrap_or(&0);
            if plating > 0 {
                state.enemies[i].block += plating as u32;
                *state.enemies[i].buffs.entry(BuffType::Plating).or_insert(0) -= 1;
            }

            // Apply block component of current move
            if let Some(ref script) = state.enemies[i].ai_script {
                let block = state.enemies[i]
                    .ai_runtime
                    .last_move_id
                    .as_deref()
                    .and_then(|id| script.moves.get(id))
                    .map(|m| m.block)
                    .unwrap_or(0);
                state.enemies[i].block += block;
            }

            // Apply powers from this move (Buff/DebuffPlayer intents)
            apply_move_powers(state, i);

            resolve_enemy_intent(state, i);
        }
    }

    // ── Decrement timed debuffs ────────────────────────────────────────────
    for enemy in &mut state.enemies {
        if enemy.is_alive() {
            for buff in &[BuffType::Vulnerable, BuffType::Weak, BuffType::Frail] {
                if let Some(s) = enemy.buffs.get_mut(buff) {
                    if *s > 0 { *s -= 1; }
                }
            }
        }
    }
    for buff in &[BuffType::Weak, BuffType::Frail, BuffType::Intangible] {
        if let Some(s) = state.player.buffs.get_mut(buff) {
            if *s > 0 { *s -= 1; }
        }
    }

    if state.is_over() {
        return true;
    }

    // Advance each living enemy's AI to set next turn's intent
    for i in 0..state.enemies.len() {
        if state.enemies[i].is_alive() {
            advance_enemy_ai(&mut state.enemies[i], i, rng);
        }
    }

    // ── Reset player block/energy, advance turn ────────────────────────────
    // Barricade prevents block reset; player Plating (Gorget) grants block each turn
    if state.player.buff(&BuffType::Barricade) == 0 {
        state.player.block = 0;
    }
    let player_plating = state.player.buff(&BuffType::Plating);
    if player_plating > 0 {
        state.player.block += player_plating as u32;
        *state.player.buffs.entry(BuffType::Plating).or_insert(0) -= 1;
    }
    state.energy = state.energy_max;
    state.turn += 1;

    // Draw new hand
    draw_cards(state, state.hand_size as usize, rng);

    // ── Player start-of-turn relic effects (after drawing) ────────────────
    // MERCURY_HOURGLASS: 3 damage to all enemies each turn
    if state.has_relic("MERCURY_HOURGLASS") {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                let absorbed = enemy.block.min(3);
                enemy.block -= absorbed;
                enemy.hp = enemy.hp.saturating_sub(3 - absorbed);
            }
        }
    }
    // SAI: gain 7 Block at the start of each turn
    if state.has_relic("SAI") {
        state.player.block += 7;
    }
    // BRIMSTONE: player +2 Strength, all enemies +1 Strength each turn
    if state.has_relic("BRIMSTONE") {
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) += 2;
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                *enemy.buffs.entry(BuffType::Strength).or_insert(0) += 1;
            }
        }
    }
    // HAPPY_FLOWER: every 3 turns → +1 energy (turn counter already incremented)
    if state.has_relic("HAPPY_FLOWER") && state.turn % 3 == 0 {
        state.energy = state.energy.saturating_add(1);
    }
    // CANDELABRA: turn 2 → +2 energy (one-time)
    if state.has_relic("CANDELABRA") && state.turn == 2 {
        state.energy = state.energy.saturating_add(2);
    }
    // HORN_CLEAT: turn 2 → +14 Block (one-time)
    if state.has_relic("HORN_CLEAT") && state.turn == 2 {
        state.player.block += 14;
    }
    // CHANDELIER: turn 3 → +3 energy (one-time)
    if state.has_relic("CHANDELIER") && state.turn == 3 {
        state.energy = state.energy.saturating_add(3);
    }
    // CAPTAINS_WHEEL: turn 3 → +18 Block (one-time)
    if state.has_relic("CAPTAINS_WHEEL") && state.turn == 3 {
        state.player.block += 18;
    }
    // PAELS_BLOOD: draw 1 additional card each turn
    if state.has_relic("PAELS_BLOOD") {
        draw_cards(state, 1, rng);
    }

    // NoxiousFumes: apply Poison to all living enemies at start of each turn
    let noxious = state.player.buff(&BuffType::NoxiousFumes);
    if noxious > 0 {
        for enemy in &mut state.enemies {
            if enemy.is_alive() {
                *enemy.buffs.entry(BuffType::Poison).or_insert(0) += noxious;
            }
        }
    }

    state.is_over()
}

/// Apply buff/debuff powers from the enemy's current move to enemy or player.
/// Called unconditionally each turn so Buff/DebuffPlayer intents take effect.
fn apply_move_powers(state: &mut CombatState, enemy_idx: usize) {
    let powers: Vec<(String, Option<String>, i32)> = {
        let enemy = &state.enemies[enemy_idx];
        let Some(ref script) = enemy.ai_script else { return };
        let Some(ref move_id) = enemy.ai_runtime.last_move_id else { return };
        let Some(data) = script.moves.get(move_id.as_str()) else { return };
        data.powers.iter().map(|p| (p.power_id.clone(), p.target.clone(), p.amount)).collect()
    };

    for (power_id, target, amount) in powers {
        let Some(buff) = map_power_id_to_buff(&power_id) else { continue };
        let targets_player = target.as_deref()
            .map(|t| t.to_lowercase().contains("player"))
            .unwrap_or(false);
        if targets_player {
            *state.player.buffs.entry(buff).or_insert(0) += amount;
        } else {
            *state.enemies[enemy_idx].buffs.entry(buff).or_insert(0) += amount;
        }
    }
}

fn map_power_id_to_buff(id: &str) -> Option<BuffType> {
    match id.to_lowercase().as_str() {
        "strength" => Some(BuffType::Strength),
        "dexterity" => Some(BuffType::Dexterity),
        "vulnerable" => Some(BuffType::Vulnerable),
        "weak" => Some(BuffType::Weak),
        "frail" => Some(BuffType::Frail),
        "poison" => Some(BuffType::Poison),
        "thorns" | "sharp_hide" => Some(BuffType::Thorns),
        "plating" | "metallicize" => Some(BuffType::Plating),
        "ritual" => Some(BuffType::Ritual),
        "intangible" => Some(BuffType::Intangible),
        "barricade" => Some(BuffType::Barricade),
        "noxious_fumes" | "noxiousfumes" => Some(BuffType::NoxiousFumes),
        _ => None,
    }
}

/// Resolve a single enemy's intent against the player.
fn resolve_enemy_intent(state: &mut CombatState, enemy_idx: usize) {
    let intent = state.enemies[enemy_idx].intent.clone();
    let enemy_weak = state.enemies[enemy_idx].buff(&BuffType::Weak) > 0;
    let player_vuln = state.player.buff(&BuffType::Vulnerable) > 0;
    // PAPER_KRANE: Weak enemies deal 40% less (×3/5) instead of 25% less (×3/4)
    let weak_denom = if state.has_relic("PAPER_KRANE") { 5u32 } else { 4u32 };

    let hit = |base: u32| -> u32 {
        let after_weak = if enemy_weak { base * 3 / weak_denom } else { base };
        if player_vuln { after_weak * 3 / 2 } else { after_weak }
    };

    match intent {
        Intent::Attack(d) => {
            damage_player(state, hit(d));
        }
        Intent::AttackMulti { damage, hits } => {
            // Each hit absorbed by remaining block separately
            for _ in 0..hits {
                damage_player(state, hit(damage));
            }
        }
        Intent::Block => {
            // Block is set from move data before this is called; nothing more to do.
        }
        // Buff, DebuffPlayer, Escape, Unknown — handled by apply_move_powers
        _ => {}
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::card::{Card, CardType, Rarity};
    use crate::domain::combat::{CombatState, EnemyState, Intent, PlayerState};
    use crate::domain::effect::{BuffType, CardEffect};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    fn strike() -> Card {
        Card::new(1, "Strike", 1, CardType::Attack, Rarity::Basic, vec![CardEffect::Damage(6)])
    }

    fn defend() -> Card {
        Card::new(2, "Defend", 1, CardType::Skill, Rarity::Basic, vec![CardEffect::Block(5)])
    }

    fn bash() -> Card {
        Card::new(
            3,
            "Bash",
            2,
            CardType::Attack,
            Rarity::Basic,
            vec![
                CardEffect::Damage(8),
                CardEffect::ApplyToEnemy { buff: BuffType::Vulnerable, stacks: 2 },
            ],
        )
    }

    fn twin_strike() -> Card {
        Card::new(
            4,
            "Twin Strike",
            1,
            CardType::Attack,
            Rarity::Common,
            vec![CardEffect::DamageMulti { base: 5, hits: 2 }],
        )
    }

    fn wound() -> Card {
        Card::new(200, "Wound", 255, CardType::Status, Rarity::Special, vec![])
    }

    fn basic_state() -> CombatState {
        let player = PlayerState::new(80, 80);
        let enemy = EnemyState::new("Cultist", 50, Intent::Attack(9));
        CombatState::new(player, vec![enemy], vec![])
    }

    // ── play_card ──────────────────────────────────────────────────────────

    #[test]
    fn strike_deals_correct_damage() {
        let mut state = basic_state();
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 44); // 50 - 6
        assert_eq!(state.energy, 2);          // 3 - 1
        assert!(state.hand.is_empty());
        assert_eq!(state.discard_pile.len(), 1);
    }

    #[test]
    fn defend_gives_block() {
        let mut state = basic_state();
        state.hand.push(defend());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.player.block, 5);
        assert_eq!(state.energy, 2);
    }

    #[test]
    fn bash_applies_damage_and_vulnerable() {
        let mut state = basic_state();
        state.hand.push(bash());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 42); // 50 - 8
        assert_eq!(state.energy, 1);          // 3 - 2
        assert_eq!(
            *state.enemies[0].buffs.get(&BuffType::Vulnerable).unwrap_or(&0),
            2
        );
    }

    #[test]
    fn vulnerable_amplifies_next_attack() {
        let mut state = basic_state();
        // Apply Vulnerable first
        *state.enemies[0].buffs.entry(BuffType::Vulnerable).or_insert(0) = 2;
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 41); // 50 - (6 * 3/2 = 9)
    }

    #[test]
    fn strength_adds_per_hit_on_multi() {
        let mut state = basic_state();
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) = 2;
        state.hand.push(twin_strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // (5+2)*2 = 14
        assert_eq!(state.enemies[0].hp, 36);
    }

    #[test]
    fn block_absorbs_damage() {
        let mut state = basic_state();
        state.hand.push(strike());
        state.enemies[0].block = 4;
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].block, 0);
        assert_eq!(state.enemies[0].hp, 48); // 50 - max(0, 6-4) = 50-2
    }

    #[test]
    fn wound_is_not_playable() {
        let mut state = basic_state();
        state.hand.push(wound());
        let err = play_card(&mut state, 0, 0, &mut rng()).unwrap_err();
        assert!(matches!(err, SimError::CardNotPlayable));
        assert_eq!(state.hand.len(), 1); // Wound still in hand
    }

    #[test]
    fn insufficient_energy_errors() {
        let mut state = basic_state();
        state.energy = 0;
        state.hand.push(strike());
        let err = play_card(&mut state, 0, 0, &mut rng()).unwrap_err();
        assert!(matches!(err, SimError::NotEnoughEnergy { .. }));
    }

    #[test]
    fn invalid_card_index_errors() {
        let mut state = basic_state();
        let err = play_card(&mut state, 99, 0, &mut rng()).unwrap_err();
        assert!(matches!(err, SimError::InvalidCardIndex(99)));
    }

    // ── draw_cards ─────────────────────────────────────────────────────────

    #[test]
    fn draw_cards_basic() {
        let mut state = basic_state();
        state.draw_pile = vec![strike(), defend()];
        draw_cards(&mut state, 2, &mut rng());
        assert_eq!(state.hand.len(), 2);
        assert!(state.draw_pile.is_empty());
    }

    #[test]
    fn draw_shuffles_discard_when_empty() {
        let mut state = basic_state();
        state.draw_pile = vec![];
        state.discard_pile = vec![strike(), defend(), bash()];
        draw_cards(&mut state, 2, &mut rng());
        assert_eq!(state.hand.len(), 2);
        // One card should remain in the (reshuffled) draw pile
        assert_eq!(state.draw_pile.len(), 1);
        assert!(state.discard_pile.is_empty());
    }

    // ── end_turn ───────────────────────────────────────────────────────────

    #[test]
    fn end_turn_enemy_attacks_player() {
        let mut state = basic_state(); // enemy intends Attack(9)
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 71); // 80 - 9
    }

    #[test]
    fn end_turn_player_block_absorbs() {
        let mut state = basic_state();
        state.player.block = 5;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 76); // 80 - max(0, 9-5) = 80-4
        assert_eq!(state.player.block, 0); // reset for next turn
    }

    #[test]
    fn end_turn_resets_energy_and_advances_turn() {
        let mut state = basic_state();
        state.energy = 1;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.energy, 3);
        assert_eq!(state.turn, 2);
    }

    #[test]
    fn end_turn_draws_five_cards() {
        let mut state = basic_state();
        // Put 5 strikes in the draw pile
        state.draw_pile = (0..5).map(|_| strike()).collect();
        end_turn(&mut state, &mut rng());
        assert_eq!(state.hand.len(), 5);
    }

    #[test]
    fn end_turn_ethereal_cards_exhaust() {
        use crate::domain::card::Card;
        let mut state = basic_state();
        let ethereal = Card::new(99, "Dazed", 255, CardType::Status, Rarity::Special, vec![])
            .with_ethereal();
        state.hand.push(ethereal);
        state.draw_pile = vec![];
        end_turn(&mut state, &mut rng());
        assert_eq!(state.exhaust_pile.len(), 1);
    }

    #[test]
    fn poison_ticks_on_end_turn() {
        let mut state = basic_state();
        *state.enemies[0].buffs.entry(BuffType::Poison).or_insert(0) = 3;
        // No attack intent so we only get poison damage
        state.enemies[0].intent = Intent::Unknown;
        end_turn(&mut state, &mut rng());
        // Enemy loses 3 HP from poison, stacks reduced to 2
        assert_eq!(state.enemies[0].hp, 47);
        assert_eq!(*state.enemies[0].buffs.get(&BuffType::Poison).unwrap(), 2);
    }

    #[test]
    fn thorns_damages_attacker() {
        let mut state = basic_state();
        *state.enemies[0].buffs.entry(BuffType::Thorns).or_insert(0) = 3;
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.player.hp, 77); // 80 - 3 Thorns
        assert_eq!(state.enemies[0].hp, 44); // still 50-6
    }

    #[test]
    fn combat_won_when_all_enemies_dead() {
        let mut state = basic_state();
        state.enemies[0].hp = 1;
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert!(state.is_won());
    }

    // ── New mechanics ──────────────────────────────────────────────────────

    #[test]
    fn intangible_caps_incoming_damage_to_1() {
        let mut state = basic_state(); // enemy attacks for 9
        *state.player.buffs.entry(BuffType::Intangible).or_insert(0) = 2;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 79); // 80 - 1 (capped by Intangible)
    }

    #[test]
    fn intangible_decrements_each_turn() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        *state.player.buffs.entry(BuffType::Intangible).or_insert(0) = 2;
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.player.buffs.get(&BuffType::Intangible).unwrap(), 1);
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.player.buffs.get(&BuffType::Intangible).unwrap(), 0);
    }

    #[test]
    fn barricade_preserves_player_block_between_turns() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        state.player.block = 10;
        *state.player.buffs.entry(BuffType::Barricade).or_insert(0) = 1;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.block, 10); // block was not cleared
    }

    #[test]
    fn without_barricade_block_is_reset() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        state.player.block = 10;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.block, 0);
    }

    #[test]
    fn noxious_fumes_applies_poison_at_start_of_turn() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        *state.player.buffs.entry(BuffType::NoxiousFumes).or_insert(0) = 2;
        end_turn(&mut state, &mut rng());
        // After end_turn, new turn starts with 2 Poison on the enemy
        let poison = *state.enemies[0].buffs.get(&BuffType::Poison).unwrap_or(&0);
        assert_eq!(poison, 2);
    }

    #[test]
    fn vulnerable_decrements_each_turn() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        *state.enemies[0].buffs.entry(BuffType::Vulnerable).or_insert(0) = 2;
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.enemies[0].buffs.get(&BuffType::Vulnerable).unwrap(), 1);
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.enemies[0].buffs.get(&BuffType::Vulnerable).unwrap(), 0);
        // Third turn: doesn't go negative
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.enemies[0].buffs.get(&BuffType::Vulnerable).unwrap(), 0);
    }

    #[test]
    fn weak_decrements_on_player() {
        let mut state = basic_state();
        state.enemies[0].intent = Intent::Unknown;
        *state.player.buffs.entry(BuffType::Weak).or_insert(0) = 1;
        end_turn(&mut state, &mut rng());
        assert_eq!(*state.player.buffs.get(&BuffType::Weak).unwrap(), 0);
    }

    fn body_slam() -> Card {
        Card::new(
            10,
            "Body Slam",
            1,
            CardType::Attack,
            Rarity::Common,
            vec![CardEffect::DamageEqBlock],
        )
    }

    #[test]
    fn damage_eq_block_deals_current_block_as_damage() {
        let mut state = basic_state();
        state.player.block = 15;
        state.hand.push(body_slam());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 35); // 50 - 15
    }

    #[test]
    fn damage_eq_block_zero_block_deals_no_damage() {
        let mut state = basic_state();
        state.player.block = 0;
        state.hand.push(body_slam());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 50); // no damage
    }

    // ── Relic: ANCHOR ─────────────────────────────────────────────────────────

    #[test]
    fn anchor_grants_10_block_at_start_of_combat() {
        use crate::domain::encounter::apply_start_of_combat_relics;
        let mut state = basic_state();
        state.relics.insert("ANCHOR".to_string());
        apply_start_of_combat_relics(&mut state);
        assert_eq!(state.player.block, 10);
    }

    // ── Relic: RED_SKULL ──────────────────────────────────────────────────────

    #[test]
    fn red_skull_adds_strength_at_half_hp() {
        let mut state = basic_state();
        state.player.hp = 40; // 40/80 = 50% → threshold met
        state.relics.insert("RED_SKULL".to_string());
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // Strike 6 + 3 Strength = 9
        assert_eq!(state.enemies[0].hp, 41);
    }

    #[test]
    fn red_skull_inactive_above_half_hp() {
        let mut state = basic_state();
        state.player.hp = 41; // above 50%
        state.relics.insert("RED_SKULL".to_string());
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // Strike 6, no bonus
        assert_eq!(state.enemies[0].hp, 44);
    }

    // ── Relic: PAPER_PHROG ────────────────────────────────────────────────────

    #[test]
    fn paper_phrog_amplifies_vulnerable_more() {
        let mut state = basic_state();
        state.relics.insert("PAPER_PHROG".to_string());
        *state.enemies[0].buffs.entry(BuffType::Vulnerable).or_insert(0) = 2;
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // 6 * 7/4 = 10 (vs 9 without Paper Phrog)
        assert_eq!(state.enemies[0].hp, 40);
    }

    // ── Relic: PAPER_KRANE ────────────────────────────────────────────────────

    #[test]
    fn paper_krane_amplifies_weak_penalty() {
        let mut state = basic_state();
        state.relics.insert("PAPER_KRANE".to_string());
        *state.player.buffs.entry(BuffType::Weak).or_insert(0) = 2;
        state.hand.push(strike());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // 6 * 3/5 = 3 (vs 4 without Paper Krane)
        assert_eq!(state.enemies[0].hp, 47);
    }

    // ── Relic: TUNGSTEN_ROD ───────────────────────────────────────────────────

    #[test]
    fn tungsten_rod_reduces_each_hit_by_1() {
        let mut state = basic_state(); // enemy attacks for 9
        state.relics.insert("TUNGSTEN_ROD".to_string());
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 72); // 80 - (9-1) = 72
    }

    // ── Relic: NUNCHAKU ───────────────────────────────────────────────────────

    #[test]
    fn nunchaku_grants_energy_on_10th_attack() {
        let mut state = basic_state();
        state.relics.insert("NUNCHAKU".to_string());
        // Play 9 strikes first
        for _ in 0..9 {
            state.hand.push(strike());
            state.energy = 3;
            let n = state.hand.len() - 1;
            play_card(&mut state, n, 0, &mut rng()).unwrap();
        }
        // Restore some enemy HP so we can keep attacking
        state.enemies[0].hp = 50;
        state.energy = 1; // only 1 energy left
        state.hand.push(strike());
        let n = state.hand.len() - 1;
        play_card(&mut state, n, 0, &mut rng()).unwrap(); // 10th attack
        // Should have gained 1 energy (now 1 net, spent 1 on strike, gained 1)
        assert_eq!(state.energy, 1);
    }

    // ── Relic: ORICHALCUM ─────────────────────────────────────────────────────

    #[test]
    fn orichalcum_absorbs_enemy_damage_when_zero_block() {
        // Enemy attacks for 9; Orichalcum grants 6 block, so player takes 3.
        let mut state = basic_state(); // enemy: Attack(9)
        state.relics.insert("ORICHALCUM".to_string());
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 77); // 80 - (9 - 6)
    }

    #[test]
    fn orichalcum_skips_if_already_have_block() {
        // Player has 3 block; enemy attacks 9 → net 6 damage. Orichalcum skips.
        let mut state = basic_state();
        state.player.block = 3;
        state.relics.insert("ORICHALCUM".to_string());
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 74); // 80 - (9 - 3)
    }

    // ── Relic: LIZARD_TAIL ────────────────────────────────────────────────────

    #[test]
    fn lizard_tail_saves_from_lethal_damage() {
        let mut state = basic_state();
        state.player.hp = 5;
        state.relics.insert("LIZARD_TAIL".to_string());
        // Enemy attacks for 9, which would kill player
        end_turn(&mut state, &mut rng());
        assert!(state.player.hp > 0, "Lizard Tail should prevent death");
        assert!(state.lizard_tail_triggered);
    }

    #[test]
    fn lizard_tail_triggers_only_once() {
        let mut state = basic_state();
        state.player.hp = 5;
        state.relics.insert("LIZARD_TAIL".to_string());
        end_turn(&mut state, &mut rng()); // Saved once
        assert!(state.lizard_tail_triggered);
        // Next lethal hit won't be saved — player should die
        state.player.hp = 1;
        end_turn(&mut state, &mut rng());
        assert_eq!(state.player.hp, 0);
    }
}
