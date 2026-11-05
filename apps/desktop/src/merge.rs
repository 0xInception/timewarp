//! Joining split recordings: one match can end up in several files (app
//! restarted, recording stopped and started). Pieces of the same match are
//! stitched into one video without re-encoding, and every moment, note, star
//! and clip is moved onto the joined timeline.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use block2::RcBlock;
use objc2_av_foundation::{
    AVAssetExportPresetPassthrough, AVAssetExportSession, AVAssetExportSessionStatus, AVFileTypeMPEG4,
    AVMutableComposition, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSString, NSURL};
use timewarp_engine::{SessionMeta, VideoMarker};

use crate::Game;

/// For recordings without match ids (made before ids existed): same mode,
/// compatible champion, close in time, and the in-game clock carries on.
fn looks_continuous(prev: &SessionMeta, next: &SessionMeta) -> bool {
    if prev.condensed.is_some() || next.condensed.is_some() {
        return false;
    }
    if prev.game_mode.is_none() || prev.game_mode != next.game_mode {
        return false;
    }
    if let (Some(a), Some(b)) = (&prev.champion, &next.champion) {
        if a != b {
            return false;
        }
    }
    let prev_end = prev
        .ended_at
        .unwrap_or_else(|| prev.started_at + chrono::Duration::milliseconds((prev.duration_secs * 1000.0) as i64));
    let gap = (next.started_at - prev_end).num_milliseconds() as f64 / 1000.0;
    if !(-5.0..=900.0).contains(&gap) {
        return false;
    }
    let prev_end_clock = prev.game_clock_end.or(prev.game_time_offset.map(|po| prev.duration_secs - po));
    match (prev_end_clock, next.game_time_offset) {
        (Some(prev_end_clock), Some(no)) => {
            // Game clock where the first piece ended and the next one started.
            let next_start_clock = -no;
            next_start_clock >= prev_end_clock - 10.0 && next_start_clock - prev_end_clock <= gap + 90.0
        }
        _ => gap <= 120.0,
    }
}

/// Indices (into `games`) of recordings that belong together, oldest first.
/// Only groups with two or more pieces are returned.
pub fn groups(games: &[Game]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..games.len()).collect();
    order.sort_by_key(|&i| games[i].meta.started_at);
    let mut out: Vec<Vec<usize>> = vec![];
    let mut current: Vec<usize> = vec![];
    for i in order {
        let joins = current.last().is_some_and(|&p| {
            let (a, b) = (&games[p].meta, &games[i].meta);
            a.condensed.is_none() && b.condensed.is_none() && a.same_match(b).unwrap_or_else(|| looks_continuous(a, b))
        });
        if !joins {
            if current.len() > 1 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        current.push(i);
    }
    if current.len() > 1 {
        out.push(current);
    }
    out
}

fn url(path: &Path) -> objc2::rc::Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn video_tracks(asset: &AVURLAsset) -> Vec<objc2::rc::Retained<objc2_av_foundation::AVAssetTrack>> {
    unsafe {
        #[allow(deprecated)]
        let tracks = asset.tracksWithMediaType(objc2_av_foundation::AVMediaTypeVideo.expect("AVMediaTypeVideo"));
        let mut v: Vec<_> = tracks.iter().collect();
        v.sort_by(|a, b| a.timeRange().start.seconds().total_cmp(&b.timeRange().start.seconds()));
        v
    }
}

/// Number of video tracks (a joined file with more than one shows black
/// after the first track ends; see `render_single_track`).
pub fn video_track_count(path: &Path) -> usize {
    unsafe { video_tracks(&AVURLAsset::URLAssetWithURL_options(&url(path), None)).len() }
}

/// Whether every source is one video track of the same size, so the pieces
/// can be stitched without re-encoding.
fn same_format(sources: &[PathBuf]) -> bool {
    let sizes: Vec<(i64, i64)> = sources
        .iter()
        .flat_map(|p| unsafe {
            video_tracks(&AVURLAsset::URLAssetWithURL_options(&url(p), None))
                .iter()
                .map(|t| {
                    let s = t.naturalSize();
                    (s.width as i64, s.height as i64)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    sizes.len() == sources.len() && sizes.windows(2).all(|w| w[0] == w[1])
}

/// Blocking: render `sources` (oldest first) into one video track at the
/// largest source size, scaling smaller pieces to fit (hardware HEVC
/// re-encode). Used when pieces were recorded at different resolutions,
/// and to repair a joined file that ended up with several video tracks.
fn render_single_track(sources: &[PathBuf], dst: &Path) -> Result<Vec<f64>, String> {
    use objc2::runtime::ProtocolObject;
    use objc2_av_foundation::{
        AVAssetExportPresetHEVCHighestQuality, AVMediaTypeAudio, AVMediaTypeVideo, AVMutableVideoComposition,
        AVMutableVideoCompositionInstruction, AVMutableVideoCompositionLayerInstruction,
    };
    use objc2_core_foundation::{CGAffineTransform, CGSize};
    use objc2_foundation::NSArray;

    let _ = std::fs::remove_file(dst);
    let err = |e: objc2::rc::Retained<objc2_foundation::NSError>| e.localizedDescription().to_string();
    unsafe {
        let comp = AVMutableComposition::composition();
        let vtrack = comp
            .addMutableTrackWithMediaType_preferredTrackID(AVMediaTypeVideo.expect("video"), 0)
            .ok_or("couldn't create a video track")?;
        let atrack = comp.addMutableTrackWithMediaType_preferredTrackID(AVMediaTypeAudio.expect("audio"), 0);

        // Render at the largest source size.
        let assets: Vec<_> = sources.iter().map(|p| AVURLAsset::URLAssetWithURL_options(&url(p), None)).collect();
        let mut render = CGSize { width: 0.0, height: 0.0 };
        for a in &assets {
            for t in video_tracks(a) {
                let s = t.naturalSize();
                if s.height > render.height {
                    render = s;
                }
            }
        }
        if render.height <= 0.0 {
            return Err("no video in these recordings".into());
        }

        let layer = AVMutableVideoCompositionLayerInstruction::videoCompositionLayerInstructionWithAssetTrack(&vtrack);
        let mut offsets = vec![];
        let mut cursor = 0.0;
        for asset in &assets {
            offsets.push(cursor);
            #[allow(deprecated)]
            let dur = asset.duration().seconds();
            // Video: tracks in order; overlapping ranges (from an earlier
            // multi-track join) are trimmed so the timeline stays the same.
            let mut covered = 0.0_f64;
            for t in video_tracks(asset) {
                let r = t.timeRange();
                let (start, end) = (r.start.seconds().max(covered), r.start.seconds() + r.duration.seconds());
                if end <= start {
                    continue;
                }
                vtrack
                    .insertTimeRange_ofTrack_atTime_error(
                        CMTimeRange { start: CMTime::with_seconds(start, 600), duration: CMTime::with_seconds(end - start, 600) },
                        &t,
                        CMTime::with_seconds(cursor + start, 600),
                    )
                    .map_err(err)?;
                let size = t.naturalSize();
                let k = render.height / size.height.max(1.0);
                let transform = CGAffineTransform { a: k, b: 0.0, c: 0.0, d: k, tx: (render.width - size.width * k) / 2.0, ty: 0.0 };
                layer.setTransform_atTime(transform, CMTime::with_seconds(cursor + start, 600));
                covered = end;
            }
            if let Some(at) = &atrack {
                #[allow(deprecated)]
                let audio = asset.tracksWithMediaType(AVMediaTypeAudio.expect("audio"));
                for t in audio.iter() {
                    let r = t.timeRange();
                    let _ = at.insertTimeRange_ofTrack_atTime_error(r, &t, CMTime::with_seconds(cursor + r.start.seconds(), 600));
                }
            }
            cursor += dur;
        }

        let instruction = AVMutableVideoCompositionInstruction::videoCompositionInstruction();
        #[allow(deprecated)]
        instruction.setTimeRange(CMTimeRange { start: CMTime::with_seconds(0.0, 600), duration: CMTime::with_seconds(cursor, 600) });
        #[allow(deprecated)]
        instruction.setLayerInstructions(&NSArray::from_retained_slice(&[objc2::rc::Retained::into_super(layer)]));
        let video_comp = AVMutableVideoComposition::videoComposition();
        #[allow(deprecated)]
        video_comp.setRenderSize(render);
        #[allow(deprecated)]
        video_comp.setFrameDuration(CMTime::with_seconds(1.0 / 60.0, 600));
        #[allow(deprecated)]
        video_comp.setInstructions(&NSArray::from_retained_slice(&[ProtocolObject::from_retained(instruction)]));

        let session = AVAssetExportSession::exportSessionWithAsset_presetName(&comp, AVAssetExportPresetHEVCHighestQuality)
            .ok_or("could not create export session")?;
        session.setOutputURL(Some(&url(dst)));
        session.setOutputFileType(AVFileTypeMPEG4);
        session.setVideoComposition(Some(&video_comp));
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        session.exportAsynchronouslyWithCompletionHandler(&done);
        rx.recv().map_err(|_| "join was interrupted".to_string())?;
        if session.status() == AVAssetExportSessionStatus::Completed {
            Ok(offsets)
        } else {
            Err(session.error().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "join failed".into()))
        }
    }
}

/// Blocking: put a joined file that has several video tracks back into one.
pub fn repair(video: &Path) -> Result<(), String> {
    let tmp = crate::condense::temp_path(video).with_file_name(format!(
        "repaired-{}",
        video.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::create_dir_all(tmp.parent().unwrap()).map_err(|e| e.to_string())?;
    render_single_track(&[video.to_path_buf()], &tmp)?;
    crate::deleted::trash(crate::deleted::Kind::Replaced, None, "Original before a repair", &[video.to_path_buf()]);
    std::fs::rename(&tmp, video).map_err(|e| e.to_string())
}

/// Blocking: stitch `parts` (oldest first) into `dst`. Returns where each
/// part starts in the joined video. Same-format pieces are stitched without
/// re-encoding; mixed resolutions are rendered into one track.
fn stitch(parts: &[PathBuf], dst: &Path) -> Result<Vec<f64>, String> {
    if !same_format(parts) {
        return render_single_track(parts, dst);
    }
    let _ = std::fs::remove_file(dst);
    unsafe {
        let comp = AVMutableComposition::composition();
        let mut offsets = vec![];
        let mut cursor = 0.0;
        for p in parts {
            let asset = AVURLAsset::URLAssetWithURL_options(&url(p), None);
            #[allow(deprecated)]
            let dur = asset.duration();
            let secs = dur.seconds();
            if !secs.is_finite() || secs <= 0.0 {
                continue;
            }
            #[allow(deprecated)]
            comp.insertTimeRange_ofAsset_atTime_error(
                CMTimeRange { start: CMTime::with_seconds(0.0, 600), duration: dur },
                &asset,
                CMTime::with_seconds(cursor, 600),
            )
            .map_err(|e| e.localizedDescription().to_string())?;
            offsets.push(cursor);
            cursor += secs;
        }
        if offsets.len() != parts.len() {
            return Err("one of the recordings couldn't be read".into());
        }
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(&comp, AVAssetExportPresetPassthrough)
            .ok_or("could not create export session")?;
        session.setOutputURL(Some(&url(dst)));
        session.setOutputFileType(AVFileTypeMPEG4);
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        session.exportAsynchronouslyWithCompletionHandler(&done);
        rx.recv().map_err(|_| "join was interrupted".to_string())?;
        if session.status() == AVAssetExportSessionStatus::Completed {
            Ok(offsets)
        } else {
            Err(session.error().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "join failed".into()))
        }
    }
}

/// One sidecar for the joined video: moments shifted by each part's offset,
/// match details from the latest piece that has them.
pub fn merged_meta(parts: &[SessionMeta], offsets: &[f64], total: f64) -> SessionMeta {
    let latest = |f: &dyn Fn(&SessionMeta) -> bool| parts.iter().rev().find(|m| f(m)).cloned();
    let mut out = latest(&|m| m.player.is_some()).unwrap_or_else(|| parts[parts.len() - 1].clone());
    out.started_at = parts[0].started_at;
    out.duration_secs = total;
    out.markers = parts
        .iter()
        .zip(offsets)
        .flat_map(|(m, &off)| {
            m.markers
                .iter()
                // A moment whose game time doesn't fit this piece's clock is
                // left over from an old bug (events before the piece began).
                .filter(|v| {
                    m.game_time_offset
                        .is_none_or(|o| v.marker.kind == timewarp_league::MarkerKind::Bookmark || (v.video_time - (v.marker.game_time + o)).abs() < 3.0)
                })
                .map(move |v| VideoMarker { marker: v.marker.clone(), video_time: v.video_time + off })
        })
        .collect();
    out.result = parts.iter().rev().find_map(|m| m.result.clone());
    out.queue = parts.iter().rev().find_map(|m| m.queue.clone());
    out.patch = parts.iter().rev().find_map(|m| m.patch.clone());
    out.game_mode = parts.iter().rev().find_map(|m| m.game_mode.clone());
    out.champion = parts.iter().rev().find_map(|m| m.champion.clone());
    out.role_override = parts.iter().rev().find_map(|m| m.role_override.clone());
    out.game_id = parts.iter().find_map(|m| m.game_id);
    out.process_id = parts.iter().find_map(|m| m.process_id.clone());
    // Game clock no longer maps to video time with one offset across pieces;
    // markers keep their own game time.
    out.game_time_offset = if parts.len() == 1 { parts[0].game_time_offset } else { None };
    out.condensed = None;
    let last = &parts[parts.len() - 1];
    out.ended_at = Some(last.ended_at.unwrap_or_else(|| {
        last.started_at + chrono::Duration::milliseconds((last.duration_secs * 1000.0) as i64)
    }));
    out.game_clock_end = last.game_clock_end.or(last.game_time_offset.map(|o| last.duration_secs - o));
    out
}

/// `2026-11-05_15-12-10_Amumu_Win.mp4` style name for the joined video.
fn merged_path(first: &Path, meta: &SessionMeta) -> PathBuf {
    let stem = first.file_stem().unwrap_or_default().to_string_lossy();
    let date: String = stem.chars().take(19).collect();
    let mut name = date;
    if let Some(c) = &meta.champion {
        name.push('_');
        name.push_str(c);
    }
    if let Some(r) = &meta.result {
        name.push('_');
        name.push_str(r);
    }
    first.with_file_name(format!("{name}.mp4"))
}

pub struct Joined {
    pub path: PathBuf,
    /// (old file name, offset in the joined video) for each part.
    pub parts: Vec<(String, f64)>,
}

/// Blocking end-to-end: stitch, write the sidecar, move the pieces to the
/// Trash, put the joined video in place.
pub fn run(parts: &[(PathBuf, SessionMeta)]) -> Result<Joined, String> {
    let paths: Vec<PathBuf> = parts.iter().map(|(p, _)| p.clone()).collect();
    let tmp = crate::condense::temp_path(&paths[0]).with_file_name(format!(
        "joined-{}",
        paths[0].file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::create_dir_all(tmp.parent().unwrap()).map_err(|e| e.to_string())?;
    let offsets = stitch(&paths, &tmp)?;
    let total = unsafe {
        #[allow(deprecated)]
        AVURLAsset::URLAssetWithURL_options(&url(&tmp), None).duration().seconds()
    };
    let metas: Vec<SessionMeta> = parts.iter().map(|(_, m)| m.clone()).collect();
    let meta = merged_meta(&metas, &offsets, total);
    let dst = merged_path(&paths[0], &meta);
    // Originals go to the Trash (recoverable) only after the join succeeded.
    for p in &paths {
        crate::deleted::trash(crate::deleted::Kind::Replaced, None, "Piece of a recording that was joined", &[p.clone(), p.with_extension("json")]);
    }
    std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
    timewarp_engine::write_sidecar(&dst, &meta).map_err(|e| e.to_string())?;
    Ok(Joined {
        path: dst,
        parts: paths
            .iter()
            .zip(&offsets)
            .map(|(p, &o)| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), o))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use timewarp_league::{Marker, MarkerKind};

    fn meta(start: &str, dur: f64, offset: Option<f64>, champ: Option<&str>) -> SessionMeta {
        SessionMeta {
            started_at: chrono::DateTime::parse_from_rfc3339(start).unwrap().into(),
            duration_secs: dur,
            game_time_offset: offset,
            champion: champ.map(Into::into),
            game_mode: Some("PRACTICETOOL".into()),
            ..Default::default()
        }
    }

    fn game(m: SessionMeta) -> Game {
        Game { path: PathBuf::from("x.mp4"), meta: m, bytes: 0 }
    }

    #[test]
    fn groups_your_real_practice_tool_pieces() {
        // From the actual sidecars: 15:12:10 (78s, offset -117) then
        // 15:23:55 (offset -822): the game clock carried on across the gap.
        let games = vec![
            game(meta("2026-11-05T15:12:10+02:00", 78.4, Some(-117.4), None)),
            game(meta("2026-11-05T15:23:55+02:00", 4.3, Some(-822.1), None)),
            game(meta("2026-11-05T15:28:57+02:00", 24.0, Some(-1120.0), Some("Amumu"))),
            // A different champion an hour later is a different game.
            game(meta("2026-11-05T16:30:00+02:00", 60.0, Some(-30.0), Some("Rammus"))),
        ];
        assert_eq!(groups(&games), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn match_ids_win_over_heuristics() {
        let mut a = meta("2026-11-05T15:00:00+02:00", 60.0, None, Some("Jinx"));
        let mut b = meta("2026-11-05T15:01:30+02:00", 60.0, None, Some("Jinx"));
        a.game_id = Some(1);
        b.game_id = Some(2);
        assert!(groups(&[game(a.clone()), game(b.clone())]).is_empty());
        b.game_id = Some(1);
        assert_eq!(groups(&[game(a), game(b)]), vec![vec![0, 1]]);
    }

    #[test]
    fn shifts_markers_by_part_offset() {
        let mut a = meta("2026-11-05T15:00:00+02:00", 60.0, Some(-10.0), None);
        a.markers = vec![VideoMarker { marker: Marker { kind: MarkerKind::Kill, label: String::new(), game_time: 40.0 }, video_time: 30.0 }];
        let mut b = meta("2026-11-05T15:02:00+02:00", 60.0, Some(-130.0), Some("Jinx"));
        b.markers = vec![VideoMarker { marker: Marker { kind: MarkerKind::Death, label: String::new(), game_time: 140.0 }, video_time: 10.0 }];
        let m = merged_meta(&[a, b], &[0.0, 60.0], 120.0);
        let times: Vec<f64> = m.markers.iter().map(|v| v.video_time).collect();
        assert_eq!(times, vec![30.0, 70.0]);
        assert_eq!(m.champion.as_deref(), Some("Jinx"));
        assert_eq!(m.duration_secs, 120.0);
    }
}

#[cfg(test)]
mod real_file {
    use super::*;

    /// Stitches two copies of a real recording (TIMEWARP_TEST_VIDEO).
    #[test]
    fn stitches_real_recordings() {
        let Some(src) = std::env::var_os("TIMEWARP_TEST_VIDEO") else { return };
        let dir = std::env::temp_dir().join("timewarp-join-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, out) = (dir.join("a.mp4"), dir.join("b.mp4"), dir.join("joined.mp4"));
        std::fs::copy(&src, &a).unwrap();
        std::fs::copy(&src, &b).unwrap();
        let offsets = stitch(&[a, b], &out).expect("stitch");
        assert_eq!(offsets.len(), 2);
        assert!(offsets[1] > 20.0, "second part starts after the first: {offsets:?}");
        let total = unsafe {
            #[allow(deprecated)]
            AVURLAsset::URLAssetWithURL_options(&url(&out), None).duration().seconds()
        };
        assert!((total - 2.0 * offsets[1]).abs() < 0.5, "joined length {total}");
    }
}

#[cfg(test)]
mod repair_test {
    use super::*;

    /// TIMEWARP_TEST_MULTITRACK=<joined file with several video tracks>
    #[test]
    fn renders_multitrack_file_into_one_track() {
        let Some(src) = std::env::var_os("TIMEWARP_TEST_MULTITRACK") else { return };
        let out = std::env::temp_dir().join("timewarp-repaired.mp4");
        let before = unsafe {
            #[allow(deprecated)]
            AVURLAsset::URLAssetWithURL_options(&url(Path::new(&src)), None).duration().seconds()
        };
        render_single_track(&[PathBuf::from(&src)], &out).expect("render");
        assert_eq!(video_track_count(&out), 1);
        let after = unsafe {
            #[allow(deprecated)]
            AVURLAsset::URLAssetWithURL_options(&url(&out), None).duration().seconds()
        };
        assert!((after - before).abs() < 1.5, "{before} vs {after}");
    }
}
