//! Export and import: a plain folder you can back up, move to another Mac or
//! share. Pick what goes in (notes, matchup notes, clips, recordings).
//!
//! ```text
//! Timewarp Export 2026-11-05 17-30/
//!   manifest.json     what's inside
//!   library.json      notes, saved moments, matchup notes
//!   recordings/       full games (.mp4 + .json)
//!   clips/            clips (.mp4 + .json)
//! ```
//!
//! Importing merges: nothing you already have is overwritten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{GameNotes, MatchupNotes, Store};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    /// Game notes, timed notes, saved moments, Keep.
    pub notes: bool,
    pub matchups: bool,
    pub clips: bool,
    pub recordings: bool,
    /// All settings except the save location (a path on this Mac).
    pub settings: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self { notes: true, matchups: true, clips: true, recordings: false, settings: true }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub app: String,
    pub version: u32,
    pub exported_at: Option<chrono::DateTime<chrono::Local>>,
    pub notes: usize,
    pub matchups: usize,
    pub clips: usize,
    pub recordings: usize,
    pub bytes: u64,
    pub settings: bool,
}

/// What lives in library.json of an export (same shape as the app's store).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Library {
    games: BTreeMap<String, GameNotes>,
    matchups: BTreeMap<String, MatchupNotes>,
}

fn copy_with_sidecar(src: &Path, dir: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(dir)?;
    let name = src.file_name().unwrap_or_default();
    // std::fs::copy clones on APFS: instant and no extra space on the same drive.
    let bytes = std::fs::copy(src, dir.join(name))?;
    let side = src.with_extension("json");
    if side.exists() {
        std::fs::copy(&side, dir.join(name).with_extension("json"))?;
    }
    Ok(bytes)
}

/// Blocking. Creates `<parent>/Timewarp Export <date>/` and returns it.
pub fn export(
    parent: &Path,
    parts: Parts,
    config: &timewarp_engine::Config,
    store_json: &[u8],
    recordings: &[PathBuf],
    clips: &[PathBuf],
) -> Result<(PathBuf, Manifest), String> {
    let stamp = chrono::Local::now();
    let mut dir = parent.join(format!("Timewarp Export {}", stamp.format("%Y-%m-%d %H-%M")));
    let mut n = 2;
    while dir.exists() {
        dir = parent.join(format!("Timewarp Export {} ({n})", stamp.format("%Y-%m-%d %H-%M")));
        n += 1;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut m = Manifest { app: "Timewarp".into(), version: 1, exported_at: Some(stamp), ..Default::default() };

    if parts.notes || parts.matchups {
        let full: Library = serde_json::from_slice(store_json).unwrap_or_default();
        let lib = Library {
            games: if parts.notes { full.games } else { Default::default() },
            matchups: if parts.matchups { full.matchups } else { Default::default() },
        };
        m.notes = lib.games.len();
        m.matchups = lib.matchups.values().filter(|n| !n.notes.trim().is_empty()).count();
        std::fs::write(dir.join("library.json"), serde_json::to_vec_pretty(&lib).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    if parts.settings {
        let mut v = serde_json::to_value(config).map_err(|e| e.to_string())?;
        // The save location is a path on this Mac; it isn't carried over.
        if let Some(o) = v.as_object_mut() {
            o.remove("output_dir");
        }
        std::fs::write(dir.join("settings.json"), serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        m.settings = true;
    }
    if parts.recordings {
        for r in recordings {
            m.bytes += copy_with_sidecar(r, &dir.join("recordings")).map_err(|e| format!("{}: {e}", r.display()))?;
            m.recordings += 1;
        }
    }
    if parts.clips {
        for c in clips {
            m.bytes += copy_with_sidecar(c, &dir.join("clips")).map_err(|e| format!("{}: {e}", c.display()))?;
            m.clips += 1;
        }
    }
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok((dir, m))
}

/// Settings from an export, applied on top of `current` but keeping this
/// Mac's save location. Unknown or missing fields fall back to defaults.
pub fn read_settings(dir: &Path, current: &timewarp_engine::Config) -> Result<timewarp_engine::Config, String> {
    let bytes = std::fs::read(dir.join("settings.json")).map_err(|_| "This export has no settings.".to_string())?;
    let mut cfg: timewarp_engine::Config =
        serde_json::from_slice(&bytes).map_err(|e| format!("The exported settings are damaged: {e}"))?;
    cfg.output_dir = current.output_dir.clone();
    Ok(cfg)
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(dir.join("manifest.json"))
        .map_err(|_| "That folder isn't a Timewarp export (no manifest.json).".to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("The export's manifest is damaged: {e}"))
}

#[derive(Default, Clone, Copy)]
pub struct Imported {
    pub recordings: usize,
    pub clips: usize,
    pub skipped: usize,
}

/// Blocking: copy recordings and clips that aren't already in the library.
pub fn import_files(dir: &Path, parts: Parts, output_dir: &Path) -> Result<Imported, String> {
    let mut out = Imported::default();
    let copy_all = |from: PathBuf, to: PathBuf, count: &mut usize, skipped: &mut usize| -> Result<(), String> {
        let Ok(entries) = std::fs::read_dir(&from) else { return Ok(()) };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "mp4") {
                continue;
            }
            if to.join(p.file_name().unwrap()).exists() {
                *skipped += 1;
                continue;
            }
            copy_with_sidecar(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
            *count += 1;
        }
        Ok(())
    };
    let (mut rec, mut clips, mut skipped) = (0, 0, 0);
    if parts.recordings {
        copy_all(dir.join("recordings"), output_dir.to_path_buf(), &mut rec, &mut skipped)?;
    }
    if parts.clips {
        copy_all(dir.join("clips"), output_dir.join("Clips"), &mut clips, &mut skipped)?;
    }
    out.recordings = rec;
    out.clips = clips;
    out.skipped = skipped;
    Ok(out)
}

fn merge_text(mine: &mut String, theirs: &str) {
    let theirs = theirs.trim();
    if theirs.is_empty() || mine.contains(theirs) {
        return;
    }
    if mine.trim().is_empty() {
        *mine = theirs.to_string();
    } else {
        mine.push_str("\n\n---\n\n");
        mine.push_str(theirs);
    }
}

impl Store {
    /// Merge an export's library.json: nothing existing is overwritten; notes
    /// are unioned by id, stars and Keep are combined, differing text is appended.
    pub fn merge_import(&mut self, dir: &Path, parts: Parts) -> (usize, usize) {
        let Ok(bytes) = std::fs::read(dir.join("library.json")) else { return (0, 0) };
        let lib: Library = serde_json::from_slice(&bytes).unwrap_or_default();
        let (mut games, mut matchups) = (0, 0);
        if parts.notes {
            for (key, theirs) in lib.games {
                let mine = self.game_mut(&key);
                merge_text(&mut mine.note, &theirs.note);
                for n in theirs.notes {
                    if !mine.notes.iter().any(|m| m.id == n.id) {
                        mine.notes.push(n);
                    }
                }
                mine.notes.sort_by(|a, b| a.video_time.total_cmp(&b.video_time));
                mine.saved.extend(theirs.saved);
                mine.pinned |= theirs.pinned;
                games += 1;
            }
        }
        if parts.matchups {
            for (key, theirs) in lib.matchups {
                let mut text = self.matchup(&key).map(|m| m.notes.clone()).unwrap_or_default();
                merge_text(&mut text, &theirs.notes);
                self.set_matchup_notes(&key, text);
                matchups += 1;
            }
        }
        (games, matchups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_never_loses_text() {
        let mut a = "Track his first clear.".to_string();
        merge_text(&mut a, "Ward raptors at 2:45.");
        assert_eq!(a, "Track his first clear.\n\n---\n\nWard raptors at 2:45.");
        // Importing the same thing again is a no-op.
        merge_text(&mut a, "Ward raptors at 2:45.");
        assert_eq!(a.matches("Ward raptors").count(), 1);
        let mut empty = String::new();
        merge_text(&mut empty, "Hello");
        assert_eq!(empty, "Hello");
    }

    #[test]
    fn export_then_import_round_trip() {
        let root = std::env::temp_dir().join("timewarp-transfer-test");
        let _ = std::fs::remove_dir_all(&root);
        let (lib_src, out_parent, target) = (root.join("lib"), root.join("exports"), root.join("target"));
        for d in [&lib_src, &out_parent, &target] {
            std::fs::create_dir_all(d).unwrap();
        }
        let game = lib_src.join("2026-11-05_15-00-00_Jinx_Win.mp4");
        std::fs::write(&game, b"video").unwrap();
        std::fs::write(game.with_extension("json"), b"{}").unwrap();
        let store_json = br#"{"games":{"2026-11-05_15-00-00_Jinx_Win.mp4":{"note":"good game","notes":[],"saved":[],"pinned":true}},"matchups":{"BOTTOM|Jinx|Caitlyn":{"notes":"respect her range"}}}"#;
        let parts = Parts { notes: true, matchups: true, clips: false, recordings: true, settings: true };
        let mut cfg = timewarp_engine::Config::default();
        cfg.capture.fps = 30;
        cfg.bookmark_key = Some("F9".into());
        cfg.output_dir = "/Users/someone-else/Movies/Timewarp".into();
        let (dir, m) = export(&out_parent, parts, &cfg, store_json, &[game], &[]).unwrap();
        assert!(m.settings);
        // Settings come back, but the save location stays this Mac's.
        let here = timewarp_engine::Config { output_dir: target.clone(), ..Default::default() };
        let got_cfg = read_settings(&dir, &here).unwrap();
        assert_eq!((got_cfg.capture.fps, got_cfg.bookmark_key.as_deref()), (30, Some("F9")));
        assert_eq!(got_cfg.output_dir, target);
        assert_eq!((m.recordings, m.notes, m.matchups), (1, 1, 1));
        assert!(read_manifest(&dir).is_ok());
        let got = import_files(&dir, parts, &target).unwrap();
        assert_eq!(got.recordings, 1);
        assert!(target.join("2026-11-05_15-00-00_Jinx_Win.json").exists());
        // A second import skips what's already there.
        assert_eq!(import_files(&dir, parts, &target).unwrap().skipped, 1);
    }
}
