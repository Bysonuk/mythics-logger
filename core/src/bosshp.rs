//! A boss pull's boss health at its end, by the server's own rule, so the
//! app can tell which wipe got a boss lowest before it sends anything (the
//! Backlog tab's "Kills and each boss's best wipe").
//!
//! Ported from the server's parser (`mt10/logs/parser.py`: `_advanced`,
//! `_boss_health` and the handlers that call them), and checked against it
//! on the owner's real pulls (`examples/backlog_size.rs --boss-hp` writes
//! each pull's place and number, and the server's parser reads the same
//! bytes; counts only, never a line or a name):
//!
//! - With Advanced Combat Logging on, most events carry a block about one
//!   unit, with its current and maximum health. The last reading of each
//!   creature (`Creature-…` or `Vehicle-…`) in the pull is kept.
//! - The block is at the server's field 12 (counting the event name as 0)
//!   for spell damage, heals and `SPELL_CAST_SUCCESS`, and field 9 for
//!   `SWING_DAMAGE`, `SWING_DAMAGE_LANDED` and `ENVIRONMENTAL_DAMAGE`. Here
//!   the event name isn't a field, so those are 11 and 8.
//! - The log never says which creature is the boss, so: the biggest health
//!   pool seen, and any within half of it (a council's other members),
//!   summed. Rounded to 2 decimals, as Python's `round`. A kill is 0.
//! - The block's length is measured from the pull's first
//!   `SPELL_CAST_SUCCESS` (nothing comes after the block on a cast); a
//!   length of 0, or `ADVANCED_LOG_ENABLED,0` in the header, means no block.
//!
//! The server reads a few fields before the block on some events (a spell's
//! id and name; the environment's type, which comes after the block) and
//! gives up on a line where they're missing or not numbers; so does this.

use crate::line::fields;
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct BossHp {
    /// The header's `ADVANCED_LOG_ENABLED`, if it had one.
    advanced: Option<bool>,
    /// The header's `COMBAT_LOG_VERSION`.
    version: Option<u32>,
    /// The advanced block's length, once measured.
    block: Option<usize>,
    /// Each creature's last (current, maximum) health.
    units: HashMap<String, (i64, i64)>,
}

impl BossHp {
    pub fn new(advanced: Option<bool>, version: Option<u32>) -> Self {
        Self {
            advanced,
            version,
            ..Self::default()
        }
    }

    /// The block's length: measured, else 19 fields in version 22 and 17
    /// before (the server's `ADVANCED_FIELDS`); 0 with advanced logging off.
    fn block_len(&self) -> usize {
        if self.advanced == Some(false) {
            return 0;
        }
        self.block
            .unwrap_or(if self.version == Some(22) { 19 } else { 17 })
    }

    /// One line of the pull: its event name and the fields after it.
    pub fn feed(&mut self, event: &str, rest: &str) {
        let at = match event {
            "SPELL_DAMAGE"
            | "SPELL_PERIODIC_DAMAGE"
            | "SPELL_BUILDING_DAMAGE"
            | "RANGE_DAMAGE"
            | "SPELL_HEAL"
            | "SPELL_PERIODIC_HEAL"
            | "SPELL_CAST_SUCCESS" => 11,
            "SWING_DAMAGE" | "SWING_DAMAGE_LANDED" | "ENVIRONMENTAL_DAMAGE" => 8,
            _ => return,
        };
        let f = fields(rest);
        // The source's and target's GUIDs and names, read first.
        if f.len() < 6 {
            return;
        }
        if at == 11 {
            // A spell's id and name, read before the block.
            if f.len() < 10 || f[8].parse::<i64>().is_err() {
                return;
            }
            if event == "SPELL_CAST_SUCCESS" && self.block.is_none() && self.advanced != Some(false)
            {
                // The server's `len(f) - 12`, with the event name in f.
                self.block = Some((f.len() + 1).saturating_sub(12));
            }
        }
        let block = self.block_len();
        // The environment's type, after the block, is read before it too.
        if event == "ENVIRONMENTAL_DAMAGE" && f.len() <= 8 + block {
            return;
        }
        if block == 0 || f.len() <= at + 3 {
            return;
        }
        let unit = f[at];
        if !(unit.starts_with("Creature-") || unit.starts_with("Vehicle-")) {
            return;
        }
        let (Ok(cur), Ok(max)) = (f[at + 2].parse::<i64>(), f[at + 3].parse::<i64>()) else {
            return;
        };
        match self.units.get_mut(unit) {
            Some(u) => *u = (cur, max),
            None => {
                self.units.insert(unit.to_string(), (cur, max));
            }
        }
    }

    /// The boss's health when the pull ended, in percent; `None` if no
    /// creature's health was seen.
    pub fn pct(&self, success: Option<bool>) -> Option<f64> {
        let biggest = self.units.values().map(|u| u.1).max()?;
        if biggest <= 0 {
            return None;
        }
        let (cur, total) = self
            .units
            .values()
            .filter(|u| i128::from(u.1) * 2 >= i128::from(biggest))
            .fold((0i128, 0i128), |(c, t), u| {
                (c + i128::from(u.0.max(0)), t + i128::from(u.1))
            });
        if success == Some(true) {
            return Some(0.0);
        }
        // As Python's `round(100 * cur / total, 2)`: one correctly rounded
        // division of the exact numbers, then the nearest 2-decimal value.
        let x = (100 * cur) as f64 / total as f64;
        format!("{x:.2}").parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swing(target: &str, cur: i64, max: i64) -> String {
        format!(
            "Player-1-A,\"P\",0x512,0x0,{target},\"Boss\",0x10a48,0x0,{target},0000000000000000,{cur},{max},0,0,0,0,0,0,1,0,0,0,1.0,2.0,2810,3.0,80,1000,900,-1,1,0,0,0,nil,nil"
        )
    }

    fn cast(block: usize) -> String {
        let mut s = "Player-1-A,\"P\",0x512,0x0,0000000000000000,nil,0x80000000,0x80000000,1459,\"Spell\",0x40".to_string();
        for _ in 0..block {
            s.push_str(",0");
        }
        s
    }

    #[test]
    fn the_last_reading_of_the_biggest_pool_and_any_within_half_of_it() {
        let mut hp = BossHp::new(Some(true), Some(22));
        hp.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-1-A", 900, 1000));
        hp.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-2-B", 400, 600));
        // An add under half the biggest pool: not a boss.
        hp.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-3-C", 0, 400));
        hp.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-1-A", 300, 1000));
        // (300 + 400) / (1000 + 600)
        assert_eq!(hp.pct(Some(false)), Some(43.75));
        assert_eq!(hp.pct(None), Some(43.75), "an unfinished pull too");
        assert_eq!(hp.pct(Some(true)), Some(0.0));
    }

    #[test]
    fn rounds_to_two_decimals_and_counts_overkill_as_zero() {
        let mut hp = BossHp::new(None, Some(22));
        hp.feed(
            "SWING_DAMAGE_LANDED",
            &swing("Vehicle-0-1-2810-1-1-A", 1, 3),
        );
        assert_eq!(hp.pct(Some(false)), Some(33.33));
        hp.feed(
            "SWING_DAMAGE_LANDED",
            &swing("Vehicle-0-1-2810-1-1-A", -50, 3),
        );
        assert_eq!(hp.pct(Some(false)), Some(0.0));
    }

    #[test]
    fn players_and_short_lines_are_ignored() {
        let mut hp = BossHp::new(Some(true), Some(22));
        hp.feed("SWING_DAMAGE", &swing("Player-1-B", 10, 100));
        hp.feed("SWING_DAMAGE", "Player-1-A,\"P\",0x512,0x0,Creature-1");
        hp.feed(
            "SPELL_AURA_APPLIED",
            &swing("Creature-0-1-2810-1-1-A", 1, 2),
        );
        assert_eq!(hp.pct(Some(false)), None);
    }

    fn spell(target: &str, spell_id: &str, cur: i64, max: i64) -> String {
        format!(
            "Player-1-A,\"P\",0x512,0x0,{target},\"Boss\",0x10a48,0x0,{spell_id},\"Fireball\",0x4,{target},0000000000000000,{cur},{max},0,0,0,0,0,0,1,0,0,0,1.0,2.0,2810,3.0,80,500,400,-1,4,0,0,0,nil,nil"
        )
    }

    #[test]
    fn spell_damage_and_heals_read_the_targets_block_after_the_spell() {
        let mut hp = BossHp::new(Some(true), Some(22));
        hp.feed(
            "SPELL_DAMAGE",
            &spell("Creature-0-1-2810-1-1-A", "133", 250, 1000),
        );
        assert_eq!(hp.pct(Some(false)), Some(25.0));
        hp.feed(
            "SPELL_PERIODIC_HEAL",
            &spell("Creature-0-1-2810-1-1-A", "774", 500, 1000),
        );
        assert_eq!(hp.pct(Some(false)), Some(50.0));
        // The server gives up on a line whose spell id isn't a number.
        hp.feed(
            "SPELL_DAMAGE",
            &spell("Creature-0-1-2810-1-1-A", "x", 1, 1000),
        );
        assert_eq!(hp.pct(Some(false)), Some(50.0));
    }

    #[test]
    fn environmental_damage_needs_its_type_after_the_block() {
        let mut hp = BossHp::new(Some(true), Some(22));
        // The 8 base fields and a block of 19, and no type after it.
        let unit = "Creature-0-1-2810-1-1-A";
        let bare = format!(
            "0000000000000000,nil,0x80000000,0x80000000,{unit},\"Boss\",0x10a48,0x0,{unit},0000000000000000,1,4,0,0,0,0,0,0,1,0,0,0,1.0,2.0,2810,3.0,80"
        );
        hp.feed("ENVIRONMENTAL_DAMAGE", &bare);
        assert_eq!(hp.pct(Some(false)), None);
        hp.feed("ENVIRONMENTAL_DAMAGE", &format!("{bare},Falling,100"));
        assert_eq!(hp.pct(Some(false)), Some(25.0));
    }

    #[test]
    fn no_block_without_advanced_logging() {
        let mut off = BossHp::new(Some(false), Some(22));
        off.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-1-A", 1, 2));
        assert_eq!(off.pct(Some(false)), None);
        // Measured as empty from the first cast: nothing after it counts.
        let mut measured = BossHp::new(None, Some(22));
        measured.feed("SPELL_CAST_SUCCESS", &cast(0));
        measured.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-1-A", 1, 2));
        assert_eq!(measured.pct(Some(false)), None);
        let mut on = BossHp::new(None, Some(22));
        on.feed("SPELL_CAST_SUCCESS", &cast(19));
        on.feed("SWING_DAMAGE", &swing("Creature-0-1-2810-1-1-A", 1, 2));
        assert_eq!(on.pct(Some(false)), Some(50.0));
    }
}
