//! Which of a past log's pulls are sent in full, and which as a summary.
//!
//! The owner's decision (29 Sep 2026): past logs send "kills and each boss's
//! best wipe" by default, with "all pulls" as the alternative (a setting on
//! the Backlog tab). Per log file (one session) and boss (encounter and
//! difficulty):
//! - every kill goes in full;
//! - so does the one wipe that got the boss lowest (`crate::bosshp`, the
//!   server's own rule), the longest if several got as low;
//! - every other wipe goes as a pull summary: when, how long, the boss's
//!   health at the end and the roster hash, a few hundred bytes instead of
//!   megabytes (`POST /api/logger/pull-summaries`), so the site's pull
//!   counts, wipe history and best % stay right.
//!
//! Keys, and the bosses inside them, always go in full, and so does a wipe
//! the server couldn't file (no roster hash, no start time). Live logging
//! is untouched: it sends every pull in full.

use crate::api::Visibility;
use crate::backlog::FileReport;
use crate::queue::{Item, Origin};
use crate::splitter::{Kind, Segment};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

/// The Backlog tab's setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BacklogPulls {
    /// Every kill and each boss's best wipe in full; other wipes summarised.
    #[default]
    KillsAndBestWipe,
    /// Every pull in full.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Full,
    Summary,
}

fn is_pull(s: &Segment) -> bool {
    s.opened_as == Kind::Encounter
}

fn is_kill(s: &Segment) -> bool {
    s.success == Some(true)
}

/// A wipe can go as a summary only if the server can file it and tell a
/// later full copy of it apart: a boss, a start time and a roster hash.
pub fn can_summarise(s: &Segment) -> bool {
    is_pull(s)
        && !is_kill(s)
        && s.encounter_id.is_some()
        && s.roster_hash.is_some()
        && !s.start_time.is_empty()
}

/// Better wipe first: lower boss health (unknown is worst), then the longer
/// pull, then the later one.
fn better(a: &Segment, b: &Segment) -> Ordering {
    let hp = |s: &Segment| s.boss_hp_pct.unwrap_or(f64::INFINITY);
    hp(a)
        .partial_cmp(&hp(b))
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.duration_ms.unwrap_or(0).cmp(&a.duration_ms.unwrap_or(0)))
        .then_with(|| b.start_offset.cmp(&a.start_offset))
}

/// How each of one log file's segments is sent, in the same order.
pub fn plan(segments: &[&Segment], mode: BacklogPulls) -> Vec<How> {
    let mut out = vec![How::Full; segments.len()];
    if mode == BacklogPulls::All {
        return out;
    }
    let mut bosses: HashMap<(Option<u32>, Option<u32>), Vec<usize>> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        if is_pull(s) && !is_kill(s) {
            bosses
                .entry((s.encounter_id, s.difficulty))
                .or_default()
                .push(i);
        }
    }
    for wipes in bosses.values() {
        let best = wipes
            .iter()
            .copied()
            .min_by(|&a, &b| better(segments[a], segments[b]));
        for &i in wipes {
            if Some(i) != best && can_summarise(segments[i]) {
                out[i] = How::Summary;
            }
        }
    }
    out
}

/// A past log's pulls and keys as queue items, each to go in full or as a
/// summary. The plan is made over the whole file, so which wipe is a boss's
/// best doesn't change with what's been sent; `skip` then leaves out what
/// the app or the server already has.
pub fn queue_items(
    report: &FileReport,
    mode: BacklogPulls,
    visibility: Visibility,
    region: &str,
    skip: impl Fn(&Segment) -> bool,
) -> Vec<Item> {
    let segs: Vec<&Segment> = report.uploadable().collect();
    let how = plan(&segs, mode);
    segs.into_iter()
        .zip(how)
        .filter(|(s, _)| !skip(s))
        .map(|(s, how)| {
            let path = report.file.path.clone();
            match how {
                How::Full => Item::new(Origin::Backlog, path, s.clone(), visibility, region),
                How::Summary => Item::summarised_wipe(path, s.clone(), visibility, region),
            }
        })
        .collect()
}

/// What sending a file's pulls and keys would come to: segments in full and
/// their bytes before compression, and summaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Sizes {
    pub full: u32,
    pub full_bytes: u64,
    pub summaries: u32,
}

impl Sizes {
    /// About what goes over the wire: the full segments at the backlog's
    /// ratio, and the summaries.
    pub fn estimate(&self) -> u64 {
        (self.full_bytes as f64 / crate::chunker::BACKLOG_RATIO) as u64
            + u64::from(self.summaries) * crate::chunker::SUMMARY_BYTES
    }
}

/// `queue_items`'s sizes, without making the items.
pub fn sizes(report: &FileReport, mode: BacklogPulls, skip: impl Fn(&Segment) -> bool) -> Sizes {
    let segs: Vec<&Segment> = report.uploadable().collect();
    let how = plan(&segs, mode);
    let mut out = Sizes::default();
    for (s, how) in segs.into_iter().zip(how) {
        if skip(s) {
            continue;
        }
        match how {
            How::Full => {
                out.full += 1;
                out.full_bytes += s.size;
            }
            How::Summary => out.summaries += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(
        enc: u32,
        diff: u32,
        success: Option<bool>,
        hp: Option<f64>,
        dur: u64,
        at: u64,
    ) -> Segment {
        Segment {
            kind: if success.is_some() {
                Kind::Encounter
            } else {
                Kind::Segment
            },
            opened_as: Kind::Encounter,
            complete: success.is_some(),
            encounter_id: Some(enc),
            name: Some("Plexus Sentinel".into()),
            difficulty: Some(diff),
            group_size: Some(20),
            instance_id: Some(2810),
            key_level: None,
            map_id: None,
            affixes: vec![],
            success,
            duration_ms: Some(dur),
            boss_hp_pct: hp,
            bosses: vec![],
            start_time: format!("9/28/2026 20:{:02}:00.0001", at % 60),
            end_time: String::new(),
            header: None,
            zone_line: None,
            advanced: Some(true),
            start_offset: at * 1000,
            end_offset: at * 1000 + 500,
            size: 500,
            sha256: format!("{at:064x}"),
            roster_hash: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn kills_and_each_bosss_best_wipe_go_in_full() {
        let segs = [
            pull(3129, 16, Some(false), Some(80.0), 60_000, 1),
            pull(3129, 16, Some(false), Some(23.4), 220_000, 2),
            pull(3129, 16, Some(false), Some(51.0), 120_000, 3),
            pull(3129, 16, Some(true), Some(0.0), 312_000, 4),
            // Another difficulty is another boss: its only wipe goes.
            pull(3129, 15, Some(false), Some(90.0), 30_000, 5),
            pull(3130, 16, Some(false), Some(64.2), 90_000, 6),
            pull(3130, 16, Some(false), Some(64.2), 95_000, 7),
        ];
        let refs: Vec<&Segment> = segs.iter().collect();
        use How::{Full, Summary};
        assert_eq!(
            plan(&refs, BacklogPulls::KillsAndBestWipe),
            [Summary, Full, Summary, Full, Full, Summary, Full],
            "the tie goes to the longer pull"
        );
        assert_eq!(plan(&refs, BacklogPulls::All), [Full; 7]);
    }

    #[test]
    fn unknown_health_ranks_last_and_unfinished_pulls_count_as_wipes() {
        let segs = [
            pull(3129, 16, Some(false), None, 400_000, 1),
            pull(3129, 16, None, Some(40.0), 100_000, 2),
            pull(3129, 16, Some(false), Some(55.5), 100_000, 3),
        ];
        let refs: Vec<&Segment> = segs.iter().collect();
        use How::{Full, Summary};
        assert_eq!(
            plan(&refs, BacklogPulls::KillsAndBestWipe),
            [Summary, Full, Summary]
        );
    }

    #[test]
    fn keys_and_wipes_the_server_couldnt_file_go_in_full() {
        let mut key = pull(0, 8, Some(false), None, 1_000, 1);
        key.opened_as = Kind::Key;
        key.kind = Kind::Key;
        key.encounter_id = None;
        let mut no_roster = pull(3129, 16, Some(false), Some(99.0), 10_000, 2);
        no_roster.roster_hash = None;
        let best = pull(3129, 16, Some(false), Some(10.0), 10_000, 3);
        let segs = [key, no_roster, best];
        let refs: Vec<&Segment> = segs.iter().collect();
        assert_eq!(plan(&refs, BacklogPulls::KillsAndBestWipe), [How::Full; 3]);
    }
}
