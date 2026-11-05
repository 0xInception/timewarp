//! Your study material: notes, saved moments and matchup notes.
//!
//! One JSON file (`library.json`, next to the config). Recordings stay plain
//! MP4 + engine sidecar; nothing here ever rewrites them. Edits mark the
//! store dirty and the app flushes it shortly after, so typing doesn't hit
//! the disk on every keystroke.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use timewarp_engine::SessionMeta;
use timewarp_league::MarkerKind;

use crate::Game;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// Keyed by recording file name.
    pub games: BTreeMap<String, GameNotes>,
    /// Keyed by [`matchup_key`].
    pub matchups: BTreeMap<String, MatchupNotes>,
    #[serde(skip)]
    dirty: bool,
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameNotes {
    pub note: String,
    pub notes: Vec<TimedNote>,
    /// Keys of auto-tagged moments the user saved.
    pub saved: BTreeSet<String>,
    /// "Keep": never removed by storage cleanup.
    pub pinned: bool,
}

impl GameNotes {
    /// Anything the user invested in this game protects it from cleanup.
    pub fn is_protected(&self) -> bool {
        self.pinned
            || !self.note.trim().is_empty()
            || !self.notes.is_empty()
            || !self.saved.is_empty()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TimedNote {
    pub id: u64,
    pub video_time: f64,
    pub text: String,
    #[serde(default)]
    pub saved: bool,
    /// Lines drawn on the paused frame, shown again when you pause there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawing: Vec<Stroke>,
}

/// One freehand line, in 0..1 coordinates of the video picture so it lands
/// on the same spot at any window size.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub color: u8,
    pub points: Vec<[f32; 2]>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchupNotes {
    pub notes: String,
}

impl Store {
    fn path() -> PathBuf {
        // dev: a TIMEWARP_DIR demo library keeps its notes beside it
        if let Some(dir) = std::env::var_os("TIMEWARP_DIR") {
            return PathBuf::from(dir).join("library.json");
        }
        timewarp_engine::Config::path().with_file_name("library.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let path = Self::path();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let tmp = path.with_extension("json.tmp");
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)) {
                    log::error!("saving library: {e}");
                }
            }
            Err(e) => log::error!("serializing library: {e}"),
        }
    }

    pub fn game(&self, game: &Game) -> Option<&GameNotes> {
        self.games.get(&game_key(game))
    }

    pub fn game_mut(&mut self, key: &str) -> &mut GameNotes {
        self.dirty = true;
        self.games.entry(key.to_owned()).or_default()
    }

    pub fn matchup(&self, key: &str) -> Option<&MatchupNotes> {
        self.matchups.get(key)
    }

    pub fn set_matchup_notes(&mut self, key: &str, text: String) {
        self.dirty = true;
        self.matchups.entry(key.to_owned()).or_default().notes = text;
    }

    pub fn add_note(&mut self, game_key: &str, video_time: f64, text: String, drawing: Vec<Stroke>) -> u64 {
        let id = chrono::Local::now().timestamp_micros() as u64;
        let g = self.game_mut(game_key);
        g.notes.push(TimedNote { id, video_time, text, saved: false, drawing });
        g.notes.sort_by(|a, b| a.video_time.total_cmp(&b.video_time));
        id
    }

    /// Fold the notes of several recordings into one (after joining split
    /// recordings): timed notes shift by each part's offset, stars and Keep
    /// carry over, game notes are combined.
    pub fn merge_games(&mut self, parts: &[(String, f64)], into: &str) {
        let mut merged = GameNotes::default();
        for (key, offset) in parts {
            let Some(g) = self.games.remove(key) else { continue };
            if !g.note.trim().is_empty() {
                if !merged.note.is_empty() {
                    merged.note.push_str("\n\n");
                }
                merged.note.push_str(g.note.trim());
            }
            merged.notes.extend(g.notes.into_iter().map(|mut n| {
                n.video_time += offset;
                n
            }));
            merged.saved.extend(g.saved);
            merged.pinned |= g.pinned;
        }
        if let Some(existing) = self.games.remove(into) {
            merged.saved.extend(existing.saved);
            merged.pinned |= existing.pinned;
        }
        merged.notes.sort_by(|a, b| a.video_time.total_cmp(&b.video_time));
        self.games.insert(into.to_owned(), merged);
        self.dirty = true;
    }

    /// Move timed notes onto a new timeline (after condensing).
    pub fn remap_notes(&mut self, game_key: &str, map: impl Fn(f64) -> Option<f64>) {
        if let Some(g) = self.games.get_mut(game_key) {
            for n in &mut g.notes {
                if let Some(t) = map(n.video_time) {
                    n.video_time = t;
                }
            }
            self.dirty = true;
        }
    }

    /// A game is gone for good: drop its notes.
    pub fn forget_game(&mut self, key: &str) {
        if self.games.remove(key).is_some() {
            self.dirty = true;
        }
    }

    pub fn delete_note(&mut self, game_key: &str, id: u64) {
        self.game_mut(game_key).notes.retain(|n| n.id != id);
    }

    pub fn toggle_saved(&mut self, game_key: &str, moment: &Moment) {
        let g = self.game_mut(game_key);
        match moment.note_id {
            Some(id) => {
                if let Some(n) = g.notes.iter_mut().find(|n| n.id == id) {
                    n.saved = !n.saved;
                }
            }
            None => {
                if !g.saved.remove(&moment.key) {
                    g.saved.insert(moment.key.clone());
                }
            }
        }
    }
}

pub fn game_key(game: &Game) -> String {
    game.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// "JUNGLE|LeeSin|Graves": your role, your champion, and the enemy in that
/// same role (jungle vs jungle, mid vs mid...).
pub fn matchup_key(meta: &SessionMeta) -> Option<String> {
    let (me, vs) = (meta.player.as_ref()?, meta.opponent.as_ref()?);
    Some(format!("{}|{}|{}", me.position, me.champion_key, vs.champion_key))
}

/// "Lee Sin vs Graves".
pub fn matchup_label(meta: &SessionMeta) -> Option<String> {
    let (me, vs) = (meta.player.as_ref()?, meta.opponent.as_ref()?);
    Some(format!("{} vs {}", me.champion, vs.champion))
}

// ---------------------------------------------------------------------------
// Unified moments: auto-tagged events + your notes, in video order.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MomentKind {
    Event(MarkerKind),
    Note,
}

#[derive(Clone)]
pub struct Moment {
    /// Stable identity within a game, used for "saved".
    pub key: String,
    pub video_time: f64,
    /// In-game clock when known (auto events); notes show video time.
    pub game_time: Option<f64>,
    pub kind: MomentKind,
    pub label: String,
    pub saved: bool,
    pub note_id: Option<u64>,
    /// Emphasised on timelines (multikills, aces).
    pub big: bool,
}

impl Moment {
    /// Clock shown next to the moment.
    pub fn clock(&self) -> f64 {
        self.game_time.unwrap_or(self.video_time)
    }
}

pub fn moments(meta: &SessionMeta, notes: Option<&GameNotes>) -> Vec<Moment> {
    let mut out: Vec<Moment> = meta
        .markers
        .iter()
        .filter(|m| m.marker.kind != MarkerKind::GameEnd)
        .map(|m| {
            let key = format!("{:?}@{:.1}", m.marker.kind, m.marker.game_time);
            Moment {
                saved: notes.is_some_and(|n| n.saved.contains(&key)),
                key,
                video_time: m.video_time,
                game_time: Some(m.marker.game_time),
                kind: MomentKind::Event(m.marker.kind),
                label: m.marker.label.clone(),
                note_id: None,
                big: matches!(m.marker.kind, MarkerKind::Multikill | MarkerKind::Ace),
            }
        })
        .collect();
    if let Some(n) = notes {
        let offset = meta.game_time_offset;
        out.extend(n.notes.iter().map(|note| Moment {
            key: format!("note-{}", note.id),
            video_time: note.video_time,
            game_time: offset.map(|o| note.video_time - o).filter(|t| *t >= 0.0),
            kind: MomentKind::Note,
            label: note.text.clone(),
            saved: note.saved,
            note_id: Some(note.id),
            big: false,
        }));
    }
    out.sort_by(|a, b| a.video_time.total_cmp(&b.video_time));
    fold_duplicates(out)
}

/// Fewer, clearer moments: a multikill folds into the kill it came with
/// ("Killed Veigar Bot (double kill)", emphasised), and the same objective
/// repeated within seconds becomes one ("Horde ×3").
fn fold_duplicates(moments: Vec<Moment>) -> Vec<Moment> {
    let mut out: Vec<Moment> = Vec::with_capacity(moments.len());
    for m in moments {
        if m.kind == MomentKind::Event(MarkerKind::Multikill) {
            if let Some(kill) = out
                .iter_mut()
                .rev()
                .take_while(|k| m.video_time - k.video_time <= 1.5)
                .find(|k| k.kind == MomentKind::Event(MarkerKind::Kill))
            {
                kill.label = format!("{} ({})", kill.label, m.label.to_lowercase());
                kill.big = true;
                kill.saved |= m.saved;
                continue;
            }
        }
        if matches!(m.kind, MomentKind::Event(MarkerKind::Objective | MarkerKind::Structure)) {
            if let Some(prev) = out.iter_mut().rev().find(|p| p.kind == m.kind && m.video_time - p.video_time <= 20.0) {
                let base = prev.label.split(" ×").next().unwrap_or(&prev.label).to_string();
                if base == m.label {
                    let n = prev.label.split(" ×").nth(1).and_then(|n| n.parse::<u32>().ok()).unwrap_or(1) + 1;
                    prev.label = format!("{base} ×{n}");
                    prev.saved |= m.saved;
                    continue;
                }
            }
        }
        out.push(m);
    }
    out
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use timewarp_engine::VideoMarker;
    use timewarp_league::Marker;

    fn meta(events: &[(MarkerKind, &str, f64)]) -> SessionMeta {
        SessionMeta {
            markers: events
                .iter()
                .map(|&(kind, label, t)| VideoMarker { marker: Marker { kind, label: label.into(), game_time: t }, video_time: t })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn folds_multikills_and_repeat_objectives() {
        let m = meta(&[
            (MarkerKind::Kill, "Killed Milio Bot", 282.0),
            (MarkerKind::Kill, "Killed Veigar Bot", 283.0),
            (MarkerKind::Multikill, "Double kill", 283.0),
            (MarkerKind::Objective, "Horde", 487.0),
            (MarkerKind::Objective, "Horde", 488.0),
            (MarkerKind::Objective, "Horde", 490.0),
        ]);
        let labels: Vec<String> = moments(&m, None).into_iter().map(|m| m.label).collect();
        assert_eq!(labels, vec!["Killed Milio Bot", "Killed Veigar Bot (double kill)", "Horde ×3"]);
    }
}
