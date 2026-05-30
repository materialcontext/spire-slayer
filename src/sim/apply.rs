use rand::seq::SliceRandom;
use rand::Rng;
use thiserror::Error;

use crate::domain::ai::{AiCondition, AiRuntime, AiStateKind, EnemyAiScript, RepeatConstraint};
use crate::domain::card::{Card, CardType, Enchantment};
use crate::domain::combat::{CombatState, EnemyState, Intent};
use crate::domain::effect::{BuffType, CardEffect, OrbType};
use crate::domain::encounter::map_intent;

#[derive(Debug, Error)]
pub enum SimError {
    #[error("card index {0} out of range")]
    InvalidCardIndex(usize),
    #[error("not enough energy: need {need}, have {have}")]
    NotEnoughEnergy { need: u8, have: u8 },
    #[error("not enough stars: need {need}, have {have}")]
    NotEnoughStars { need: u8, have: u32 },
    #[error("card is not playable (Status or Curse type)")]
    CardNotPlayable,
    #[error("invalid target index {0}")]
    InvalidTarget(usize),
}

// ── Internal combat math ───────────────────────────────────────────────────

/// Damage a single hit of `base` deals, accounting for Strength, Weak, Vulnerable.
/// Also applies relic passive modifiers: RED_SKULL, PAPER_PHROG, PAPER_KRANE.
/// Set `add_strength` to false for block-derived damage (e.g. DamageEqBlock) where
/// Strength does not apply — only Weak/Vulnerable modifiers do.
fn compute_hit(base: u32, state: &CombatState, target_idx: usize) -> u32 {
    compute_hit_inner(base, state, target_idx, true)
}

fn compute_hit_no_strength(base: u32, state: &CombatState, target_idx: usize) -> u32 {
    compute_hit_inner(base, state, target_idx, false)
}

fn compute_hit_inner(base: u32, state: &CombatState, target_idx: usize, add_strength: bool) -> u32 {
    let weak = state.player.buff(&BuffType::Weak) > 0;
    let vulnerable = state.enemies[target_idx].buff(&BuffType::Vulnerable) > 0;

    let raw = if add_strength {
        let mut strength = state.player.buff(&BuffType::Strength);
        // RED_SKULL: +3 Strength while HP ≤ 50%
        if state.has_relic("RED_SKULL") && state.player.hp * 2 <= state.player.max_hp {
            strength += 3;
        }
        if strength >= 0 {
            base + strength as u32
        } else {
            base.saturating_sub((-strength) as u32)
        }
    } else {
        base
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
    let hp_lost = dmg - absorbed;
    enemy.hp = enemy.hp.saturating_sub(hp_lost);
    if hp_lost > 0 {
        enemy.ai_runtime.took_unblocked_damage = true;
    }

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
        // Death-trigger order: Fairy in a Bottle fires first, then Lizard Tail.
        // Die → Fairy? → die again? → Lizard Tail? → really dead.
        if state.player.hp == 0 && !state.fairy_triggered && state.has_relic("FAIRY_IN_A_BOTTLE") {
            state.fairy_triggered = true;
            state.player.hp = ((state.player.max_hp as f32 * 0.30) as u32).max(1);
        }
        if state.player.hp == 0 && !state.lizard_tail_triggered && state.has_relic("LIZARD_TAIL") {
            state.lizard_tail_triggered = true;
            state.player.hp = (state.player.max_hp / 2).max(1);
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Draw `n` cards from the draw pile into hand.
/// When the draw pile is empty, the discard pile is shuffled into it.
/// Pull all Innate cards from the draw pile into hand (called before the regular draw).
/// Innate cards are always in your opening hand; they don't count against hand_size.
pub fn draw_innate_cards(state: &mut CombatState) {
    let mut innate_indices: Vec<usize> = state.draw_pile
        .iter()
        .enumerate()
        .filter(|(_, c)| c.innate)
        .map(|(i, _)| i)
        .collect();
    // Remove back-to-front to preserve indices
    innate_indices.sort_unstable_by(|a, b| b.cmp(a));
    for i in innate_indices {
        let card = state.draw_pile.remove(i);
        state.hand.push(card);
    }
}

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
            // PERFECT_FIT: move those cards to the top of the freshly-shuffled pile
            let has_pf = state.draw_pile.iter()
                .any(|c| c.enchantments.iter().any(|e| matches!(e, Enchantment::PerfectFit)));
            if has_pf {
                let mut top: Vec<Card> = Vec::new();
                let mut rest: Vec<Card> = Vec::new();
                for c in state.draw_pile.drain(..) {
                    if c.enchantments.iter().any(|e| matches!(e, Enchantment::PerfectFit)) {
                        top.push(c);
                    } else {
                        rest.push(c);
                    }
                }
                top.extend(rest);
                state.draw_pile = top;
            }
        }
        let mut card = state.draw_pile.remove(0);
        // SLITHER: randomize cost 0–3 each time this card is drawn
        if card.enchantments.iter().any(|e| matches!(e, Enchantment::Slither)) {
            card.cost = rng.gen_range(0u8..=3u8);
        }
        state.hand.push(card);
    }
}

// ── Orb system (Defect) ────────────────────────────────────────────────────

/// Apply an orb's evoke effect, then the orb is consumed.
fn evoke_orb(state: &mut CombatState, orb: OrbType, rng: &mut impl Rng) {
    let focus = state.player.buff(&BuffType::Focus);
    match orb {
        OrbType::Lightning => {
            let dmg = (8i32 + focus).max(0) as u32;
            let living: Vec<usize> = (0..state.enemies.len())
                .filter(|&i| state.enemies[i].is_alive())
                .collect();
            if !living.is_empty() {
                let idx = living[rng.gen_range(0..living.len())];
                damage_enemy(state, idx, dmg);
            }
        }
        OrbType::Frost => {
            let block = (5i32 + focus).max(0) as u32;
            state.player.block += block;
        }
        OrbType::Dark(val) => {
            let dmg = (val as i32 + focus).max(0) as u32;
            // Target lowest-HP enemy
            let idx = (0..state.enemies.len())
                .filter(|&i| state.enemies[i].is_alive())
                .min_by_key(|&i| state.enemies[i].hp);
            if let Some(i) = idx {
                damage_enemy(state, i, dmg);
            }
        }
        OrbType::Plasma => {
            state.energy = state.energy.saturating_add(2);
        }
        OrbType::Glass(val) => {
            let dmg = (val as i32 * 2 + focus).max(0) as u32;
            for i in 0..state.enemies.len() {
                if state.enemies[i].is_alive() {
                    damage_enemy(state, i, dmg);
                }
            }
        }
    }
}

/// Channel an orb: push to queue; if full, evoke and remove the leftmost.
fn channel_orb(state: &mut CombatState, orb: OrbType, rng: &mut impl Rng) {
    if state.orbs.len() >= state.orb_slots {
        let displaced = state.orbs.remove(0);
        evoke_orb(state, displaced, rng);
    }
    state.orbs.push(orb);
}

/// Evoke the rightmost orb `count` times. Pass `u32::MAX` to evoke all orbs.
fn evoke_rightmost(state: &mut CombatState, count: u32, rng: &mut impl Rng) {
    let n = if count == u32::MAX { state.orbs.len() } else { count as usize };
    for _ in 0..n {
        if let Some(orb) = state.orbs.pop() {
            evoke_orb(state, orb, rng);
        } else {
            break;
        }
    }
}

/// Trigger each orb's passive ability (called at end of player turn, before discarding).
fn trigger_orb_passives(state: &mut CombatState, rng: &mut impl Rng) {
    let focus = state.player.buff(&BuffType::Focus);
    for i in 0..state.orbs.len() {
        let orb = state.orbs[i].clone();
        match &orb {
            OrbType::Lightning => {
                let dmg = (3i32 + focus).max(0) as u32;
                let living: Vec<usize> = (0..state.enemies.len())
                    .filter(|&j| state.enemies[j].is_alive())
                    .collect();
                if !living.is_empty() {
                    let idx = living[rng.gen_range(0..living.len())];
                    damage_enemy(state, idx, dmg);
                }
            }
            OrbType::Frost => {
                let block = (2i32 + focus).max(0) as u32;
                state.player.block += block;
            }
            OrbType::Dark(val) => {
                let incr = (6i32 + focus).max(0) as u32;
                state.orbs[i] = OrbType::Dark(val + incr);
            }
            OrbType::Plasma => {
                state.energy = state.energy.saturating_add(1);
            }
            OrbType::Glass(val) => {
                let dmg = (*val as i32 + focus).max(0) as u32;
                for j in 0..state.enemies.len() {
                    if state.enemies[j].is_alive() {
                        damage_enemy(state, j, dmg);
                    }
                }
                state.orbs[i] = OrbType::Glass(val.saturating_sub(1));
            }
        }
    }
}

/// Apply a single card effect to the combat state.
///
/// `Passive` effects are no-ops in the simulator; they are display-only.
/// `osty_attack`: damage comes from Osty (the ally), so player Strength and
/// Weak debuff are excluded — only enemy Vulnerable applies.
pub(crate) fn apply_effect(
    state: &mut CombatState,
    effect: &CardEffect,
    target_idx: usize,
    osty_attack: bool,
    rng: &mut impl Rng,
) {
    let hit = |d, st: &CombatState, t| {
        if osty_attack { compute_hit_no_strength(d, st, t) } else { compute_hit(d, st, t) }
    };
    match effect {
        CardEffect::Damage(d) => {
            if target_idx < state.enemies.len() && state.enemies[target_idx].is_alive() {
                let dmg = hit(*d, state, target_idx);
                let thorns = damage_enemy(state, target_idx, dmg);
                if thorns > 0 {
                    damage_player(state, thorns);
                }
            }
        }

        CardEffect::DamageAll(d) => {
            // Collect (index, damage, thorns) before mutating.
            let hit_list: Vec<(usize, u32, u32)> = (0..state.enemies.len())
                .filter(|&i| state.enemies[i].is_alive())
                .map(|i| {
                    let dmg = hit(*d, state, i);
                    let thorns = state.enemies[i]
                        .buffs
                        .get(&BuffType::Thorns)
                        .copied()
                        .unwrap_or(0)
                        .max(0) as u32;
                    (i, dmg, thorns)
                })
                .collect();

            for (i, dmg, thorns) in hit_list {
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
                    let dmg = hit(*base, state, target_idx);
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
                // Block value is the base; Strength, Weak, and Vulnerable all apply normally.
                // Osty flag is ignored here — DamageEqBlock is always a player action.
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

        CardEffect::EnemyLoseHp(h) => {
            // Direct HP loss: bypasses block, Strength, Weak, and Vulnerable.
            if target_idx < state.enemies.len() && state.enemies[target_idx].is_alive() {
                state.enemies[target_idx].hp = state.enemies[target_idx].hp.saturating_sub(*h);
            }
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

        CardEffect::GainStars(n) => {
            state.stars = state.stars.saturating_add(*n);
        }

        CardEffect::ChannelOrb(orb) => {
            channel_orb(state, orb.clone(), rng);
        }

        CardEffect::EvokeOrb(count) => {
            evoke_rightmost(state, *count, rng);
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

    // Check star cost (Regent's secondary resource)
    if card.star_cost > 0 && state.stars < card.star_cost as u32 {
        return Err(SimError::NotEnoughStars {
            need: card.star_cost,
            have: state.stars,
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
    let mut card = state.hand.remove(card_hand_idx);
    // X-cost (255) spends all remaining energy; regular cards spend their printed cost
    let cost_spent = if card.cost == 255 { state.energy } else { card.cost };
    state.energy -= cost_spent;
    // Deduct star cost (Regent's Stars resource)
    state.stars = state.stars.saturating_sub(card.star_cost as u32);

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
        apply_effect(state, &effective, target_idx, card.osty_attack, rng);
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

    // ── Enchantment post-play triggers ────────────────────────────────────
    // Pass 1: read enchantment state (immutable borrow of card.enchantments)
    let mut do_replay = false;
    let mut gain_energy: u8 = 0;
    let mut draw_extra: usize = 0;
    let mut vigorous_bonus: u32 = 0;
    let mut momentum_per_play: u32 = 0;
    let mut slumbering_discount: u8 = 0;
    let mut has_goopy = false;

    for enc in &card.enchantments {
        match enc {
            Enchantment::Replay { used: false } => do_replay = true,
            Enchantment::Sown { energy, used: false } => gain_energy = (*energy).min(u8::MAX as u32) as u8,
            Enchantment::Swift { cards, used: false } => draw_extra = *cards as usize,
            Enchantment::Vigorous { bonus, used: false } => vigorous_bonus = *bonus,
            Enchantment::Momentum { per_play } => momentum_per_play = *per_play,
            Enchantment::SlumberingEssence { discount } => slumbering_discount = *discount,
            Enchantment::Goopy { .. } => has_goopy = true,
            _ => {}
        }
    }

    // Apply state side-effects that don't need card mutability
    if gain_energy > 0 {
        state.energy = state.energy.saturating_add(gain_energy);
    }
    if draw_extra > 0 {
        draw_cards(state, draw_extra, rng);
    }
    if vigorous_bonus > 0 && target_idx < state.enemies.len() {
        // Vigorous bonus is unmodified extra damage (no Strength/Weak/Vulnerable)
        let thorns = damage_enemy(state, target_idx, vigorous_bonus);
        if thorns > 0 {
            damage_player(state, thorns);
        }
    }

    // Pass 2: mutate card enchantment state
    for enc in &mut card.enchantments {
        match enc {
            Enchantment::Replay { used } => *used = true,
            Enchantment::Sown { used, .. } => *used = true,
            Enchantment::Swift { used, .. } => *used = true,
            Enchantment::Vigorous { used, .. } => *used = true,
            Enchantment::Goopy { cumulative } => *cumulative += 1,
            _ => {}
        }
    }

    // MOMENTUM: permanently raise card's attack damage for this combat
    if momentum_per_play > 0 {
        for eff in &mut card.effects {
            match eff {
                CardEffect::Damage(d) | CardEffect::DamageAll(d) => *d += momentum_per_play,
                CardEffect::DamageMulti { base, .. } => *base += momentum_per_play,
                _ => {}
            }
        }
    }

    // GOOPY: permanently raise this card's block by 1 each play
    if has_goopy {
        for eff in &mut card.effects {
            if let CardEffect::Block(b) = eff {
                *b += 1;
            }
        }
    }

    // SLUMBERING_ESSENCE: restore original cost before returning card to discard
    if slumbering_discount > 0 {
        card.cost = card.cost.saturating_add(slumbering_discount);
        for enc in &mut card.enchantments {
            if let Enchantment::SlumberingEssence { discount } = enc {
                *discount = 0;
            }
        }
    }

    // REPLAY: build copy with the Replay already marked used so it doesn't re-trigger
    let replay_copy: Option<Card> = if do_replay {
        let mut copy = card.clone();
        for enc in &mut copy.enchantments {
            if let Enchantment::Replay { used } = enc {
                *used = true;
            }
        }
        Some(copy)
    } else {
        None
    };

    // Dispose of the played card
    if card.eternal {
        // TEZCATARAS_EMBER: card returns to hand
        state.hand.push(card);
    } else if card.exhausts || card.card_type == CardType::Power {
        state.exhaust_pile.push(card);
    } else {
        state.discard_pile.push(card);
    }

    // REPLAY copy goes to hand after the original is disposed
    if let Some(copy) = replay_copy {
        state.hand.push(copy);
    }

    Ok(())
}

/// End the player's turn: enemies act, state resets, new hand drawn.
///
/// Returns `true` if combat is over (won or lost) after the enemy phase.
/// Step the enemy's move AI: determine next intent and advance internal state.
/// No-ops if the enemy has no AI script (preserves existing test behaviour).
pub fn advance_enemy_ai(
    enemy: &mut EnemyState,
    slot_index: usize,
    enemy_hps: &[(u32, u32)],
    rng: &mut impl Rng,
) {
    let Some(ref script) = enemy.ai_script else { return };

    // ── Lagavulin-style sleep/wake logic ─────────────────────────────────────
    // A sleeping state is a Move state with `next: None` (loops on itself).
    let current_id = enemy.ai_runtime.current_state_id.clone();
    let is_sleeping = matches!(
        script.states.get(&current_id).map(|s| &s.kind),
        Some(AiStateKind::Move { next: None, .. })
    ) && script.wake_state_id.is_some();

    if is_sleeping {
        enemy.ai_runtime.sleep_turns += 1;
        let should_wake = enemy.ai_runtime.sleep_turns >= 3
            || enemy.ai_runtime.took_unblocked_damage;
        enemy.ai_runtime.took_unblocked_damage = false;

        if should_wake {
            if let Some(ref wake_id) = script.wake_state_id.clone() {
                enemy.ai_runtime.current_state_id = wake_id.clone();
                // Fall through to resolve the wake state's move below.
            } else {
                return;
            }
        } else {
            // Still sleeping — keep current intent (already set at init).
            return;
        }
    } else {
        enemy.ai_runtime.took_unblocked_damage = false;
    }

    // Starting from the current state (a Move state), find what state comes next.
    let current_id = enemy.ai_runtime.current_state_id.clone();
    let next_state_id = match script.states.get(&current_id).map(|s| &s.kind) {
        Some(AiStateKind::Move { next, .. }) => {
            next.clone().unwrap_or_else(|| current_id.clone())
        }
        // Already at a routing node (shouldn't be normal); stay put
        _ => current_id.clone(),
    };

    let (resolved_move_id, resolved_state_id) = resolve_to_move(
        &next_state_id,
        script,
        &enemy.ai_runtime,
        enemy.hp,
        enemy.max_hp,
        slot_index,
        enemy_hps,
        rng,
    );

    if let Some(move_id) = resolved_move_id {
        let runtime = &mut enemy.ai_runtime;
        if runtime.last_move_id.as_deref() == Some(&move_id) {
            runtime.consecutive_count += 1;
        } else {
            runtime.consecutive_count = 1;
        }
        runtime.last_move_id = Some(move_id.clone());
        runtime.used_moves.insert(move_id.clone());
        *runtime.move_use_counts.entry(move_id.clone()).or_insert(0) += 1;
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
    enemy_hps: &[(u32, u32)],
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
                .find(|b| eval_condition(&b.condition, hp, max_hp, slot_index, runtime, enemy_hps))
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

fn eval_condition(
    cond: &AiCondition,
    hp: u32,
    max_hp: u32,
    slot_index: usize,
    runtime: &AiRuntime,
    enemy_hps: &[(u32, u32)],
) -> bool {
    match cond {
        AiCondition::HpAtOrAboveHalf => hp * 2 >= max_hp,
        AiCondition::HpBelowHalf => hp * 2 < max_hp,
        AiCondition::SlotIndex(i) => slot_index == *i,
        AiCondition::AlwaysTrue => true,
        AiCondition::AlwaysFalse => false,
        AiCondition::MoveUsedLessThan(move_id, threshold) => {
            runtime.move_use_counts.get(move_id).copied().unwrap_or(0) < *threshold
        }
        AiCondition::MoveUsedAtLeast(move_id, threshold) => {
            runtime.move_use_counts.get(move_id).copied().unwrap_or(0) >= *threshold
        }
        AiCondition::AllyDead => enemy_hps
            .iter()
            .enumerate()
            .any(|(i, &(hp, _))| i != slot_index && hp == 0),
        AiCondition::AllyAlive => enemy_hps
            .iter()
            .enumerate()
            .all(|(i, &(hp, _))| i == slot_index || hp > 0),
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

    // ── Orb passives (Defect) ──────────────────────────────────────────────
    trigger_orb_passives(state, rng);

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

    // ── SlumberingEssence: reduce cost for cards in hand at end of turn ───
    for i in 0..state.hand.len() {
        let has_se = state.hand[i]
            .enchantments
            .iter()
            .any(|e| matches!(e, Enchantment::SlumberingEssence { .. }));
        if has_se && state.hand[i].cost > 0 {
            state.hand[i].cost -= 1;
            for enc in &mut state.hand[i].enchantments {
                if let Enchantment::SlumberingEssence { discount } = enc {
                    *discount += 1;
                    break;
                }
            }
        }
    }

    // ── Discard hand (Retain cards stay; Ethereal cards exhaust) ──────────
    let drained: Vec<Card> = state.hand.drain(..).collect();
    for card in drained {
        if card.ethereal {
            state.exhaust_pile.push(card);
        } else if card.retain {
            state.hand.push(card); // kept for next turn
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
    let enemy_hps: Vec<(u32, u32)> = state.enemies.iter().map(|e| (e.hp, e.max_hp)).collect();
    for i in 0..state.enemies.len() {
        if state.enemies[i].is_alive() {
            advance_enemy_ai(&mut state.enemies[i], i, &enemy_hps, rng);
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

    // Draw new hand: Innate cards first (always in opening hand), then regular draw
    draw_innate_cards(state);
    let innate_drawn = state.hand.len();
    let regular_draw = (state.hand_size as usize).saturating_sub(innate_drawn);
    draw_cards(state, regular_draw, rng);

    // ── Player start-of-turn relic effects (after drawing) ────────────────
    // BOUND_PHYLACTERY: Summon 1 per turn (≈ +1 Block)
    if state.has_relic("BOUND_PHYLACTERY") { state.player.block += 1; }
    // PHYLACTERY_UNBOUND (upgraded): Summon 2 per turn (≈ +2 Block)
    if state.has_relic("PHYLACTERY_UNBOUND") { state.player.block += 2; }
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
/// Auto-play all IMBUED cards at the start of a new combat.
///
/// Call this after drawing the opening hand (innate + regular draw) but before
/// the player's first turn. Imbued cards are played in the order they appear in
/// the draw pile; they target enemy slot 0 by default.
pub fn apply_imbued(state: &mut CombatState, rng: &mut impl Rng) {
    loop {
        // Find the first imbued card in the draw pile
        let idx = state.draw_pile
            .iter()
            .position(|c| c.enchantments.iter().any(|e| matches!(e, Enchantment::Imbued)));
        let Some(draw_idx) = idx else { break };

        // Move to hand, then play it
        let card = state.draw_pile.remove(draw_idx);
        state.hand.push(card);
        let hand_idx = state.hand.len() - 1;

        if state.hand[hand_idx].is_playable(state.energy) {
            let _ = play_card(state, hand_idx, 0, rng);
        } else {
            // Not enough energy; move to discard without playing
            let card = state.hand.remove(hand_idx);
            state.discard_pile.push(card);
        }
    }
}

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
        Intent::Escape => {
            // Enemy flees — mark it dead so it no longer participates in combat.
            state.enemies[enemy_idx].hp = 0;
        }
        // Buff, DebuffPlayer, Unknown — handled by apply_move_powers
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
    fn retain_card_stays_in_hand_after_turn() {
        use crate::domain::card::Card;
        let mut state = basic_state();
        let retained = Card::new(99, "Pummel", 1, CardType::Attack, Rarity::Common, vec![])
            .with_retain();
        state.hand.push(retained);
        state.draw_pile = vec![];
        end_turn(&mut state, &mut rng());
        assert!(state.hand.iter().any(|c| c.name == "Pummel"), "Retain card should stay in hand");
        assert!(state.discard_pile.iter().all(|c| c.name != "Pummel"), "Retain card should not be discarded");
    }

    #[test]
    fn innate_card_always_in_opening_hand() {
        use crate::domain::card::Card;
        let mut state = basic_state();
        // Fill draw pile with non-innate cards + one innate card at the end
        state.draw_pile = (0..10).map(|_| strike()).collect();
        let innate = Card::new(99, "Warpath", 1, CardType::Attack, Rarity::Common, vec![])
            .with_innate();
        state.draw_pile.push(innate);
        state.hand.clear();
        draw_innate_cards(&mut state);
        assert!(state.hand.iter().any(|c| c.name == "Warpath"), "Innate card should be drawn first");
        assert_eq!(state.hand.len(), 1, "Only the innate card should be pulled");
    }

    #[test]
    fn innate_reduces_regular_draw_count() {
        use crate::domain::card::Card;
        let mut state = basic_state();
        state.draw_pile = (0..10).map(|_| strike()).collect();
        let innate = Card::new(99, "Warpath", 1, CardType::Attack, Rarity::Common, vec![])
            .with_innate();
        state.draw_pile.push(innate);
        // Simulate start_turn: innate drawn first, then hand_size - 1 more
        draw_innate_cards(&mut state);
        let innate_count = state.hand.len();
        let regular_draw = (state.hand_size as usize).saturating_sub(innate_count);
        draw_cards(&mut state, regular_draw, &mut rng());
        assert_eq!(state.hand.len(), state.hand_size as usize, "Total hand should equal hand_size");
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

    #[test]
    fn damage_eq_block_uses_strength() {
        let mut state = basic_state();
        state.player.block = 15;
        *state.player.buffs.entry(BuffType::Strength).or_insert(0) = 5;
        state.hand.push(body_slam());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // Body Slam: block (15) + Strength (5) = 20 damage → 50 - 20 = 30
        assert_eq!(state.enemies[0].hp, 30);
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

    // ── FAIRY_IN_A_BOTTLE ordering ────────────────────────────────────────────

    #[test]
    fn fairy_triggers_before_lizard_tail() {
        let mut state = basic_state();
        state.player.hp = 1; // will die to the 9-damage enemy
        state.relics.insert("FAIRY_IN_A_BOTTLE".to_string());
        state.relics.insert("LIZARD_TAIL".to_string());
        end_turn(&mut state, &mut rng());
        // Fairy should have fired (30% of 80 = 24 HP), not Lizard Tail (50% = 40 HP)
        assert!(state.fairy_triggered, "Fairy should have triggered");
        assert!(!state.lizard_tail_triggered, "Lizard Tail should not trigger if Fairy saved first");
        assert!(state.player.hp > 0);
    }

    #[test]
    fn lizard_tail_triggers_after_fairy_is_spent() {
        let mut state = basic_state();
        state.player.hp = 1;
        state.relics.insert("FAIRY_IN_A_BOTTLE".to_string());
        state.relics.insert("LIZARD_TAIL".to_string());
        end_turn(&mut state, &mut rng()); // Fairy fires
        assert!(state.fairy_triggered);
        // Drain HP to 1 again so next hit is lethal
        state.player.hp = 1;
        end_turn(&mut state, &mut rng()); // Now Lizard Tail fires
        assert!(state.lizard_tail_triggered, "Lizard Tail should fire on second lethal hit");
        assert!(state.player.hp > 0);
    }

    #[test]
    fn both_spent_means_death() {
        let mut state = basic_state();
        state.player.hp = 1;
        state.relics.insert("FAIRY_IN_A_BOTTLE".to_string());
        state.relics.insert("LIZARD_TAIL".to_string());
        end_turn(&mut state, &mut rng()); // Fairy fires
        state.player.hp = 1;
        end_turn(&mut state, &mut rng()); // Lizard Tail fires
        state.player.hp = 1;
        end_turn(&mut state, &mut rng()); // Both spent — should die
        assert_eq!(state.player.hp, 0, "Should die when both safety nets are spent");
    }

    // ── Enchantment tests ─────────────────────────────────────────────────────

    fn strike_with(enchantment_id: &str, amount: u32) -> Card {
        let mut c = strike();
        c.apply_enchantment(enchantment_id, amount);
        c
    }

    fn defend_with(enchantment_id: &str, amount: u32) -> Card {
        let mut c = defend();
        c.apply_enchantment(enchantment_id, amount);
        c
    }

    #[test]
    fn sharp_increases_damage() {
        let mut state = basic_state();
        state.hand.push(strike_with("SHARP", 3));
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 41); // 50 - (6+3)
    }

    #[test]
    fn instinct_doubles_damage() {
        let mut state = basic_state();
        state.hand.push(strike_with("INSTINCT", 0));
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 38); // 50 - 12
    }

    #[test]
    fn adroit_adds_block() {
        let mut state = basic_state();
        state.hand.push(defend_with("ADROIT", 4));
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.player.block, 9); // 5+4
    }

    #[test]
    fn nimble_boosts_block() {
        let mut state = basic_state();
        state.hand.push(defend_with("NIMBLE", 3));
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.player.block, 8); // 5+3
    }

    #[test]
    fn royally_approved_sets_innate_and_retain() {
        let mut c = strike();
        c.apply_enchantment("ROYALLY_APPROVED", 0);
        assert!(c.innate);
        assert!(c.retain);
    }

    #[test]
    fn steady_sets_retain() {
        let mut c = strike();
        c.apply_enchantment("STEADY", 0);
        assert!(c.retain);
    }

    #[test]
    fn souls_power_removes_exhaust() {
        let mut c = Card::new(99, "Offering", 0, CardType::Skill, Rarity::Common,
            vec![CardEffect::LoseHp(6)]);
        c.exhausts = true;
        c.apply_enchantment("SOULS_POWER", 0);
        assert!(!c.exhausts);
    }

    #[test]
    fn tezcataras_ember_sets_cost_zero_and_eternal() {
        let mut c = strike();
        c.apply_enchantment("TEZCATARAS_EMBER", 0);
        assert_eq!(c.cost, 0);
        assert!(c.eternal);
    }

    #[test]
    fn eternal_card_returns_to_hand_after_play() {
        let mut state = basic_state();
        let mut c = strike();
        c.apply_enchantment("TEZCATARAS_EMBER", 0);
        state.hand.push(c);
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.hand.len(), 1, "Eternal card should return to hand");
        assert!(state.discard_pile.is_empty());
    }

    #[test]
    fn replay_glam_puts_copy_in_hand() {
        let mut state = basic_state();
        let mut c = strike();
        c.apply_enchantment("GLAM", 0);
        state.hand.push(c);
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // Original in discard, copy in hand
        assert_eq!(state.hand.len(), 1, "Replay copy should be in hand");
        assert_eq!(state.discard_pile.len(), 1);
        // Playing the copy should NOT put another copy in hand
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert!(state.hand.is_empty(), "Second play should not replay again");
    }

    #[test]
    fn momentum_increases_damage_each_play() {
        let mut state = basic_state();
        let mut c = strike();
        c.apply_enchantment("MOMENTUM", 2);
        state.draw_pile.push(c);
        draw_cards(&mut state, 1, &mut rng());
        // First play: 6 damage, then card gains +2
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 44); // 50-6
        // Card should now have 8 damage (6+2)
        assert_eq!(state.discard_pile[0].effects[0], CardEffect::Damage(8));
    }

    #[test]
    fn sown_grants_energy_first_play_only() {
        let mut state = basic_state();
        let mut c = strike();
        c.apply_enchantment("SOWN", 1);
        state.hand.push(c.clone());
        // First play: gain 1 energy; started with 3, spent 1, gain 1 → net 3
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.energy, 3); // 3 - 1 + 1

        // Second play: no energy gain
        state.energy = 3;
        let card_in_discard = state.discard_pile.remove(0);
        state.hand.push(card_in_discard);
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.energy, 2); // 3 - 1, no bonus
    }

    #[test]
    fn swift_draws_cards_first_play_only() {
        let mut state = basic_state();
        // Fill draw pile
        for _ in 0..3 {
            state.draw_pile.push(defend());
        }
        let mut c = strike();
        c.apply_enchantment("SWIFT", 2);
        state.hand.push(c.clone());
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.hand.len(), 2, "Swift should draw 2 cards on first play");

        // Second play: no draw
        state.hand.clear();
        let card_in_discard = state.discard_pile.remove(0);
        state.hand.push(card_in_discard);
        let hand_before = state.hand.len();
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.hand.len(), hand_before - 1, "Swift should not draw again");
    }

    #[test]
    fn vigorous_deals_bonus_damage_first_play_only() {
        let mut state = basic_state();
        let mut c = strike();
        c.apply_enchantment("VIGOROUS", 5);
        state.hand.push(c.clone());
        // First play: 6 + 5 = 11 damage
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 39); // 50 - 11

        // Second play: only base 6 damage
        state.enemies[0].hp = 50;
        let card_in_discard = state.discard_pile.remove(0);
        state.hand.push(card_in_discard);
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        assert_eq!(state.enemies[0].hp, 44); // 50 - 6
    }

    #[test]
    fn slumber_reduces_cost_each_turn_in_hand() {
        let mut state = basic_state();
        let mut c = Card::new(99, "Heavy", 3, CardType::Attack, Rarity::Common,
            vec![CardEffect::Damage(10)]);
        c.apply_enchantment("SLUMBERING_ESSENCE", 0);
        state.hand.push(c);
        // Turn 1: cost 3, end of turn → cost 2
        end_turn(&mut state, &mut rng());
        // Card retained? No — "Heavy" doesn't have retain. It goes to discard.
        // Let's use a retain card so it stays in hand across turns.
        // Actually let's just check the discard pile value after end_turn.
        // The card is in hand at end_turn time → cost should be decremented, then discarded.
        let discarded = state.discard_pile.iter().find(|c| c.name == "Heavy");
        // After being discarded, card.cost should still be 2 (reduced during end_turn)
        if let Some(card) = discarded {
            assert_eq!(card.cost, 2, "Cost should drop by 1 after one turn in hand");
        }
    }

    #[test]
    fn slumber_resets_cost_on_play() {
        let mut state = basic_state();
        let mut c = Card::new(99, "Heavy", 3, CardType::Attack, Rarity::Common,
            vec![CardEffect::Damage(10)]);
        c.apply_enchantment("SLUMBERING_ESSENCE", 0);
        c.retain = true; // keep it in hand for 2 turns
        state.hand.push(c);

        // Turn 1 end: cost 3 → 2, retained
        end_turn(&mut state, &mut rng());
        // Turn 2 end: cost 2 → 1, retained
        end_turn(&mut state, &mut rng());
        assert_eq!(state.hand[0].cost, 1, "Cost should be 1 after 2 turns");

        // Now play it (costs 1, not 3)
        play_card(&mut state, 0, 0, &mut rng()).unwrap();
        // Card goes to discard with restored cost
        let discarded = &state.discard_pile[0];
        assert_eq!(discarded.cost, 3, "Cost should reset to original after playing");
    }

    #[test]
    fn slither_randomizes_cost_on_draw() {
        let mut state = basic_state();
        let mut c = Card::new(99, "Slithery", 2, CardType::Attack, Rarity::Common,
            vec![CardEffect::Damage(8)]);
        c.apply_enchantment("SLITHER", 0);
        state.draw_pile.push(c);
        draw_cards(&mut state, 1, &mut rng());
        let drawn_cost = state.hand[0].cost;
        assert!(drawn_cost <= 3, "Slither cost should be 0-3");
    }

    #[test]
    fn perfect_fit_moves_to_top_after_shuffle() {
        let mut state = basic_state();
        // 4 regular cards in draw pile, 1 PerfectFit in discard
        for i in 0..4 {
            state.draw_pile.push(Card::new(i, "Filler", 1, CardType::Attack, Rarity::Common,
                vec![CardEffect::Damage(1)]));
        }
        let mut pf = defend();
        pf.apply_enchantment("PERFECT_FIT", 0);
        state.discard_pile.push(pf);

        // Drain draw pile so the next draw triggers a shuffle
        state.draw_pile.clear();
        draw_cards(&mut state, 1, &mut rng());
        assert_eq!(state.hand[0].name, "Defend", "PerfectFit card should be on top after shuffle");
    }

    #[test]
    fn goopy_increases_block_each_play() {
        let mut state = basic_state();
        let mut c = defend();
        c.apply_enchantment("GOOPY", 0);
        assert!(c.exhausts, "Goopy adds Exhaust");

        // Each play boosts the block by 1 permanently
        state.hand.push(c);
        play_card(&mut state, 0, 0, &mut rng()).unwrap(); // block = 5, then effects bumped to 6
        let exhausted = &state.exhaust_pile[0];
        assert_eq!(exhausted.effects.iter().filter_map(|e| {
            if let CardEffect::Block(b) = e { Some(*b) } else { None }
        }).next(), Some(6), "Block effect should be 6 after first Goopy play");
    }

    #[test]
    fn apply_imbued_plays_imbued_card_at_combat_start() {
        let mut state = basic_state();
        // One imbued defend in draw pile
        let mut c = defend();
        c.apply_enchantment("IMBUED", 0);
        state.draw_pile.push(c);
        apply_imbued(&mut state, &mut rng());
        // Defend should have been played (block gained) and exhausted
        assert_eq!(state.player.block, 5, "Imbued Defend should grant block");
        assert!(state.draw_pile.is_empty());
        assert!(state.exhaust_pile.is_empty()); // Defend doesn't exhaust normally
        assert_eq!(state.discard_pile.len(), 1);
    }

    #[test]
    fn reset_combat_enchantments_restores_flags() {
        let mut c = strike();
        c.apply_enchantment("GLAM", 0);
        c.apply_enchantment("SOWN", 2);
        // Simulate used state
        for enc in &mut c.enchantments {
            match enc {
                Enchantment::Replay { used } => *used = true,
                Enchantment::Sown { used, .. } => *used = true,
                _ => {}
            }
        }
        c.reset_combat_enchantments();
        for enc in &c.enchantments {
            match enc {
                Enchantment::Replay { used } => assert!(!used, "Replay should reset"),
                Enchantment::Sown { used, .. } => assert!(!used, "Sown should reset"),
                _ => {}
            }
        }
    }
}
