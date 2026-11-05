//! "Moments only": rebuild a recording from just its valuable moments and
//! replace the full VOD. Segments are stitched with AVMutableComposition and
//! exported as passthrough, so there is no re-encode and no quality loss —
//! a 30-minute game typically shrinks by 90%+.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use block2::RcBlock;
use objc2_av_foundation::{
    AVAssetExportPresetPassthrough, AVAssetExportSession, AVAssetExportSessionStatus, AVFileTypeMPEG4,
    AVMutableComposition, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSString, NSURL};
use timewarp_engine::{Condensed, KeepKinds, Segment, SessionMeta, VideoMarker};

use crate::review::{LEAD_IN, TAIL};
use crate::store::{GameNotes, MomentKind};

/// Ranges of the original video to keep, merged and sorted. Your notes,
/// bookmarks and saved moments are always included.
pub fn plan(meta: &SessionMeta, notes: Option<&GameNotes>, keep: &KeepKinds) -> Vec<(f64, f64)> {
    let duration = meta.duration_secs.max(1.0);
    let mut times: Vec<f64> = crate::store::moments(meta, notes)
        .into_iter()
        .filter(|m| {
            m.saved
                || match m.kind {
                    MomentKind::Note => true,
                    MomentKind::Event(k) => keep.keeps(k),
                }
        })
        .map(|m| m.video_time)
        .collect();
    times.sort_by(f64::total_cmp);
    let mut ranges: Vec<(f64, f64)> = vec![];
    for t in times {
        let (a, b) = ((t - LEAD_IN).max(0.0), (t + TAIL).min(duration));
        match ranges.last_mut() {
            // Merge overlapping or nearly touching ranges into one continuous cut.
            Some(last) if a <= last.1 + 2.0 => last.1 = last.1.max(b),
            _ => ranges.push((a, b)),
        }
    }
    ranges
}

fn url(path: &Path) -> objc2::rc::Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn cm(t: f64) -> CMTime {
    unsafe { CMTime::with_seconds(t, 600) }
}

/// Blocking: stitch `ranges` of `src` into `dst`. Run off the main thread.
pub fn export(src: &Path, dst: &Path, ranges: &[(f64, f64)]) -> Result<(), String> {
    let _ = std::fs::remove_file(dst);
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&url(src), None);
        let comp = AVMutableComposition::composition();
        let mut cursor = 0.0;
        for &(a, b) in ranges {
            // The synchronous variant is deprecated but fine here: we're on a
            // background thread and the asset is local.
            #[allow(deprecated)]
            comp.insertTimeRange_ofAsset_atTime_error(
                CMTimeRange { start: cm(a), duration: cm(b - a) },
                &asset,
                cm(cursor),
            )
            .map_err(|e| e.localizedDescription().to_string())?;
            cursor += b - a;
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
        rx.recv().map_err(|_| "export was interrupted".to_string())?;
        if session.status() == AVAssetExportSessionStatus::Completed {
            Ok(())
        } else {
            Err(session.error().map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "export failed".into()))
        }
    }
}

/// Where the condensed file is built before it replaces the original.
/// A hidden subfolder, so the library never lists half-written files.
pub fn temp_path(video: &Path) -> PathBuf {
    let dir = video.parent().unwrap_or(Path::new(".")).join(".timewarp-work");
    dir.join(video.file_name().unwrap_or_default())
}

/// The condensed timeline for `ranges`.
pub fn condensed_info(meta: &SessionMeta, bytes: u64, ranges: &[(f64, f64)]) -> Condensed {
    let mut at = 0.0;
    let segments = ranges
        .iter()
        .map(|&(start, end)| {
            let s = Segment { start, end, at };
            at += end - start;
            s
        })
        .collect();
    Condensed { original_duration_secs: meta.duration_secs, original_bytes: bytes, segments }
}

/// Remap markers onto the condensed timeline; markers that weren't kept are dropped.
pub fn remap_meta(meta: &SessionMeta, info: Condensed) -> SessionMeta {
    let mut out = meta.clone();
    out.markers = meta
        .markers
        .iter()
        .filter_map(|m| info.map(m.video_time).map(|video_time| VideoMarker { marker: m.marker.clone(), video_time }))
        .collect();
    // One offset no longer maps game clock to video time across cuts.
    out.game_time_offset = None;
    out.duration_secs = info.segments.last().map(|s| s.at + (s.end - s.start)).unwrap_or(0.0);
    out.condensed = Some(info);
    out
}

/// Blocking end-to-end: export, swap files, rewrite the sidecar. Returns the
/// new metadata so the caller can remap notes in the store.
/// `keep_original`: move the full video to the Trash (manual, recoverable)
/// instead of deleting it (automatic cleanup, which is meant to free space).
pub fn run(video: &Path, meta: &SessionMeta, bytes: u64, ranges: &[(f64, f64)], keep_original: bool) -> Result<SessionMeta, String> {
    let tmp = temp_path(video);
    std::fs::create_dir_all(tmp.parent().unwrap()).map_err(|e| e.to_string())?;
    export(video, &tmp, ranges)?;
    let new_meta = remap_meta(meta, condensed_info(meta, bytes, ranges));
    // Write the sidecar first: if we crash between the two steps, the worst
    // case is a full video with condensed metadata, never a lost video.
    timewarp_engine::write_sidecar(video, &new_meta).map_err(|e| e.to_string())?;
    if keep_original {
        crate::deleted::trash(crate::deleted::Kind::Replaced, None, "Full video before keeping moments only", &[video.to_path_buf()]);
    }
    std::fs::rename(&tmp, video).map_err(|e| e.to_string())?;
    Ok(new_meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use timewarp_league::{Marker, MarkerKind};

    fn meta(times: &[(MarkerKind, f64)]) -> SessionMeta {
        SessionMeta {
            duration_secs: 1800.0,
            markers: times
                .iter()
                .map(|&(kind, t)| VideoMarker { marker: Marker { kind, label: String::new(), game_time: t }, video_time: t })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn plans_merged_ranges_for_kept_kinds() {
        let m = meta(&[
            (MarkerKind::Kill, 100.0),
            (MarkerKind::Death, 106.0), // overlaps the kill's range -> merged
            (MarkerKind::Assist, 500.0), // not kept by default
            (MarkerKind::Bookmark, 900.0), // always kept
        ]);
        let ranges = plan(&m, None, &KeepKinds::default());
        assert_eq!(ranges, vec![(94.0, 110.0), (894.0, 904.0)]);
    }

    #[test]
    fn remaps_markers_onto_condensed_timeline() {
        let m = meta(&[(MarkerKind::Kill, 100.0), (MarkerKind::Assist, 500.0), (MarkerKind::Bookmark, 900.0)]);
        let ranges = plan(&m, None, &KeepKinds::default());
        let out = remap_meta(&m, condensed_info(&m, 0, &ranges));
        let times: Vec<f64> = out.markers.iter().map(|m| m.video_time).collect();
        assert_eq!(times, vec![6.0, 16.0]); // 100 -> 6 (first range starts at 94); 900 -> 10 + 6
        assert_eq!(out.duration_secs, 20.0);
    }
}

#[cfg(test)]
mod real_file {
    use super::*;
    use timewarp_league::{Marker, MarkerKind};

    /// End-to-end on a copy of a real recording (TIMEWARP_TEST_VIDEO).
    #[test]
    fn condenses_a_real_recording() {
        let Some(src) = std::env::var_os("TIMEWARP_TEST_VIDEO") else { return };
        let dir = std::env::temp_dir().join("timewarp-condense-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("game.mp4");
        std::fs::copy(&src, &video).unwrap();
        let bytes = std::fs::metadata(&video).unwrap().len();
        let meta = SessionMeta {
            duration_secs: 24.0,
            markers: [(MarkerKind::Kill, 8.0), (MarkerKind::Death, 21.0)]
                .iter()
                .map(|&(kind, t)| VideoMarker { marker: Marker { kind, label: "x".into(), game_time: t }, video_time: t })
                .collect(),
            ..Default::default()
        };
        let ranges = plan(&meta, None, &KeepKinds::default());
        assert_eq!(ranges, vec![(2.0, 12.0), (15.0, 24.0)]);
        let out = run(&video, &meta, bytes, &ranges, false).expect("condense");
        assert_eq!(out.duration_secs, 19.0);
        assert!(std::fs::metadata(&video).unwrap().len() < bytes);
        assert!(!temp_path(&video).exists());
        let sidecar: SessionMeta = serde_json::from_slice(&std::fs::read(video.with_extension("json")).unwrap()).unwrap();
        assert_eq!(sidecar.markers.len(), 2);
        assert_eq!(sidecar.markers[1].video_time, 16.0); // 21 -> 10 + (21 - 15)
    }
}
