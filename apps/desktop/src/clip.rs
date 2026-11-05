//! Clip export: cut a range out of a recording without re-encoding
//! (AVFoundation passthrough), then put the file on the clipboard so it can
//! be pasted straight into Discord, Messages, etc.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use block2::RcBlock;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSPasteboard;
use objc2_av_foundation::{
    AVAssetExportPreset1920x1080,
    AVAssetExportPresetPassthrough, AVAssetExportSession, AVAssetExportSessionStatus, AVFileTypeMPEG4, AVURLAsset,
};
use timewarp_engine::{ClipFormat, ClipSettings};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSArray, NSString, NSURL};
use serde::{Deserialize, Serialize};
use timewarp_league::MarkerKind;

fn file_url(path: &Path) -> objc2::rc::Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

/// Blocking; run it off the main thread. Exports in the configured format and,
/// with a size limit, re-encodes at the bitrate that fits when needed.
pub fn export_with(src: &Path, dst: &Path, start: f64, end: f64, opts: &ClipSettings) -> Result<(), String> {
    let preset: &'static NSString = match opts.format {
        ClipFormat::Original => unsafe { AVAssetExportPresetPassthrough },
        ClipFormat::Shareable => unsafe { AVAssetExportPreset1920x1080 },
    };
    export_preset(src, dst, start, end, preset)?;
    let Some(limit) = opts.max_mb.map(|mb| u64::from(mb) * 1_000_000) else { return Ok(()) };
    if std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0) <= limit {
        return Ok(());
    }
    // Too big: re-encode at exactly the bitrate that fits, leaving headroom
    // for audio and container overhead. One retry if the encoder overshoots.
    let duration = (end - start).max(0.5);
    let mut video_bps = ((limit as f64 * 8.0 * 0.92) / duration - 160_000.0).max(300_000.0);
    for _ in 0..2 {
        transcode(src, dst, start, end, video_bps)?;
        let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
        if size <= limit {
            return Ok(());
        }
        video_bps *= 0.8 * limit as f64 / size as f64;
    }
    Err("couldn't fit the clip under the size limit; try a shorter clip".into())
}

/// Hardware H.264 re-encode at a fixed average bitrate, scaled down when the
/// bitrate is too low for full resolution. Audio is copied as-is.
fn transcode(src: &Path, dst: &Path, start: f64, end: f64, video_bps: f64) -> Result<(), String> {
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAssetReader, AVAssetReaderTrackOutput, AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus,
        AVMediaTypeAudio, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264,
        AVVideoCompressionPropertiesKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey,
        AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey, AVVideoScalingModeKey,
        AVVideoScalingModeResizeAspect, AVVideoWidthKey,
    };
    use objc2_core_media::CMFormatDescription;
    use objc2_core_video::{kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange};
    use objc2_foundation::{NSDictionary, NSNumber};

    let _ = std::fs::remove_file(dst);
    let err = |e: objc2::rc::Retained<objc2_foundation::NSError>| e.localizedDescription().to_string();
    let key = |k: Option<&'static NSString>| k.expect("AVFoundation key");
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&url_of(src), None);
        #[allow(deprecated)]
        let vtrack = asset.tracksWithMediaType(key(AVMediaTypeVideo)).firstObject().ok_or("no video track")?;
        #[allow(deprecated)]
        let atrack = asset.tracksWithMediaType(key(AVMediaTypeAudio)).firstObject();

        // Pick a resolution the bitrate can carry.
        let natural = vtrack.naturalSize();
        let max_h = if video_bps < 1_200_000.0 { 540.0 } else if video_bps < 2_500_000.0 { 720.0 } else { 1080.0 };
        let h = natural.height.min(max_h);
        let w = (natural.width * h / natural.height.max(1.0)).round();
        let (w, h) = ((w as i64) & !1, (h as i64) & !1);

        let reader = AVAssetReader::assetReaderWithAsset_error(&asset).map_err(err)?;
        reader.setTimeRange(CMTimeRange { start: CMTime::with_seconds(start, 600), duration: CMTime::with_seconds(end - start, 600) });
        let pix = NSDictionary::<NSString, AnyObject>::from_slices(
            &[&*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString)],
            &[&*NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)],
        );
        let vout = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&vtrack, Some(&pix));
        reader.addOutput(&vout);
        let aout = atrack.as_ref().map(|t| AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(t, None));
        if let Some(a) = &aout {
            reader.addOutput(a);
        }

        let compression = NSDictionary::<NSString, AnyObject>::from_slices(
            &[key(AVVideoAverageBitRateKey), key(AVVideoMaxKeyFrameIntervalKey), key(AVVideoProfileLevelKey)],
            &[&*NSNumber::new_f64(video_bps), &*NSNumber::new_i64(120), key(AVVideoProfileLevelH264HighAutoLevel)],
        );
        let vsettings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[key(AVVideoCodecKey), key(AVVideoWidthKey), key(AVVideoHeightKey), key(AVVideoScalingModeKey), key(AVVideoCompressionPropertiesKey)],
            &[key(AVVideoCodecTypeH264), &*NSNumber::new_i64(w), &*NSNumber::new_i64(h), key(AVVideoScalingModeResizeAspect), &*compression],
        );
        let vin = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings_sourceFormatHint(key(AVMediaTypeVideo), Some(&vsettings), None);
        vin.setExpectsMediaDataInRealTime(false);
        // Audio passthrough needs the source format as a hint.
        let ain = atrack.as_ref().map(|t| {
            let fmt = t.formatDescriptions().firstObject().map(|o| &*(objc2::rc::Retained::as_ptr(&o) as *const CMFormatDescription));
            let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings_sourceFormatHint(key(AVMediaTypeAudio), None, fmt);
            input.setExpectsMediaDataInRealTime(false);
            input
        });

        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url_of(dst), key(AVFileTypeMPEG4)).map_err(err)?;
        writer.addInput(&vin);
        if let Some(a) = &ain {
            writer.addInput(a);
        }
        if !reader.startReading() || !writer.startWriting() {
            return Err(writer.error().map(err).or_else(|| reader.error().map(err)).unwrap_or_else(|| "couldn't start re-encoding".into()));
        }
        writer.startSessionAtSourceTime(CMTime::with_seconds(start, 600));

        // Pull both tracks on this thread, feeding whichever input is ready.
        let (mut vdone, mut adone) = (false, ain.is_none());
        while !(vdone && adone) {
            let mut progressed = false;
            if !vdone && vin.isReadyForMoreMediaData() {
                match vout.copyNextSampleBuffer() {
                    Some(b) => {
                        vin.appendSampleBuffer(&b);
                    }
                    None => {
                        vin.markAsFinished();
                        vdone = true;
                    }
                }
                progressed = true;
            }
            if let (false, Some(ain), Some(aout)) = (adone, &ain, &aout) {
                if ain.isReadyForMoreMediaData() {
                    match aout.copyNextSampleBuffer() {
                        Some(b) => {
                            ain.appendSampleBuffer(&b);
                        }
                        None => {
                            ain.markAsFinished();
                            adone = true;
                        }
                    }
                    progressed = true;
                }
            }
            if writer.status() == AVAssetWriterStatus::Failed {
                return Err(writer.error().map(err).unwrap_or_else(|| "re-encoding failed".into()));
            }
            if !progressed {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        writer.finishWritingWithCompletionHandler(&done);
        rx.recv().map_err(|_| "re-encoding was interrupted".to_string())?;
        if writer.status() == AVAssetWriterStatus::Completed {
            Ok(())
        } else {
            Err(writer.error().map(err).unwrap_or_else(|| "re-encoding failed".into()))
        }
    }
}

fn url_of(path: &Path) -> objc2::rc::Retained<NSURL> {
    file_url(path)
}

#[cfg_attr(not(test), allow(dead_code))]
/// Passthrough export (no re-encode). Starts on the keyframe at or before
/// `start`, so clips may begin a fraction early.
pub fn export(src: &Path, dst: &Path, start: f64, end: f64) -> Result<(), String> {
    export_preset(src, dst, start, end, unsafe { AVAssetExportPresetPassthrough })
}

fn export_preset(src: &Path, dst: &Path, start: f64, end: f64, preset: &NSString) -> Result<(), String> {
    let _ = std::fs::remove_file(dst);
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&file_url(src), None);
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(&asset, preset)
            .ok_or("could not create export session")?;
        session.setOutputURL(Some(&file_url(dst)));
        session.setOutputFileType(AVFileTypeMPEG4);
        session.setTimeRange(CMTimeRange {
            start: CMTime::with_seconds(start.max(0.0), 600),
            duration: CMTime::with_seconds((end - start).max(0.5), 600),
        });
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        session.exportAsynchronouslyWithCompletionHandler(&done);
        rx.recv().map_err(|_| "export was interrupted".to_string())?;
        if session.status() == AVAssetExportSessionStatus::Completed {
            Ok(())
        } else {
            Err(session
                .error()
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "export failed".into()))
        }
    }
}

/// Put the file itself on the clipboard (paste into Discord uploads it).
pub fn copy_file_to_clipboard(path: &Path) {
    let url = file_url(path);
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let items = NSArray::from_retained_slice(&[ProtocolObject::from_retained(url)]);
    pb.writeObjects(&items);
}

/// `<output>/Clips/<game>_<label>_<mm-ss>.mp4`, filesystem-safe.
pub fn clip_path(output_dir: &Path, game_file: &Path, label: &str, at: f64) -> PathBuf {
    let game = game_file.file_stem().unwrap_or_default().to_string_lossy();
    let label: String = label
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let t = at.max(0.0) as u64;
    output_dir.join("Clips").join(format!("{game}_{label}_{:02}-{:02}.mp4", t / 60, t % 60))
}

/// What a clip is, stored next to it as `<clip>.json` so it stays meaningful
/// after the full game is condensed or deleted.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipMeta {
    pub title: String,
    /// The tagged moment this clip was cut around (None for custom ranges).
    pub kind: Option<MarkerKind>,
    pub note: bool,
    /// File name of the source recording.
    pub source: String,
    /// Range in the source video (seconds).
    pub start: f64,
    pub end: f64,
    /// Where the moment itself sits in the source video.
    pub moment_time: Option<f64>,
    pub game_time: Option<f64>,
    pub champion: Option<String>,
    pub champion_key: Option<String>,
    pub opponent: Option<String>,
    pub result: Option<String>,
    pub queue: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
}

pub struct Clip {
    pub path: PathBuf,
    pub meta: ClipMeta,
    pub bytes: u64,
    pub thumb: Option<PathBuf>,
}

impl Clip {
    pub fn duration(&self) -> f64 {
        (self.meta.end - self.meta.start).max(0.0)
    }
    pub fn title(&self) -> String {
        if self.meta.title.is_empty() {
            self.path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
        } else {
            self.meta.title.clone()
        }
    }
}

pub fn clips_dir(output_dir: &Path) -> PathBuf {
    output_dir.join("Clips")
}

pub fn thumb_path(clip: &Path) -> PathBuf {
    let dir = clip.parent().unwrap_or(Path::new(".")).join(".thumbs");
    dir.join(format!("{}.png", clip.file_name().unwrap_or_default().to_string_lossy()))
}

/// Quick Look renders a frame (HEVC included) without us decoding anything.
pub fn make_thumbnail(clip: &Path) {
    let out = thumb_path(clip);
    let _ = std::fs::create_dir_all(out.parent().unwrap());
    let _ = std::process::Command::new("qlmanage")
        .args(["-t", "-s", "640", "-o"])
        .arg(out.parent().unwrap())
        .arg(clip)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn write_meta(clip: &Path, meta: &ClipMeta) {
    if let Ok(bytes) = serde_json::to_vec_pretty(meta) {
        let _ = std::fs::write(clip.with_extension("json"), bytes);
    }
}

/// Clips in `<output>/Clips`, newest first.
pub fn list(output_dir: &Path) -> Vec<Clip> {
    let mut out: Vec<Clip> = std::fs::read_dir(clips_dir(output_dir))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp4"))
        .map(|path| {
            let fs = std::fs::metadata(&path).ok();
            let mut meta: ClipMeta = std::fs::read(path.with_extension("json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            if meta.created_at.is_none() {
                meta.created_at = fs.as_ref().and_then(|m| m.modified().ok()).map(chrono::DateTime::from);
            }
            let thumb = Some(thumb_path(&path)).filter(|t| t.exists());
            Clip { bytes: fs.map(|m| m.len()).unwrap_or(0), path, meta, thumb }
        })
        .collect();
    out.sort_by(|a, b| b.meta.created_at.cmp(&a.meta.created_at));
    out
}

/// Rename the clip's title (and file, so it reads well in Finder/Discord).
pub fn rename(clip: &Clip, title: &str) -> Result<PathBuf, String> {
    let safe: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if safe.is_empty() {
        return Err("Name can't be empty".into());
    }
    let mut new_path = clip.path.with_file_name(format!("{safe}.mp4"));
    let mut n = 2;
    while new_path.exists() && new_path != clip.path {
        new_path = clip.path.with_file_name(format!("{safe} {n}.mp4"));
        n += 1;
    }
    if new_path != clip.path {
        std::fs::rename(&clip.path, &new_path).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(clip.path.with_extension("json"));
        let _ = std::fs::rename(thumb_path(&clip.path), thumb_path(&new_path));
    }
    let mut meta = clip.meta.clone();
    meta.title = title.trim().to_string();
    write_meta(&new_path, &meta);
    Ok(new_path)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Exports a real clip when TIMEWARP_TEST_VIDEO points at a recording.
    #[test]
    fn exports_a_clip() {
        let Some(src) = std::env::var_os("TIMEWARP_TEST_VIDEO") else { return };
        let dst = std::env::temp_dir().join("timewarp-clip-test.mp4");
        export(Path::new(&src), &dst, 5.0, 12.0).expect("export");
        assert!(std::fs::metadata(&dst).unwrap().len() > 10_000);
    }

    /// Shareable + 10 MB: H.264 that fits Discord's free limit.
    #[test]
    fn exports_a_discord_sized_clip() {
        let Some(src) = std::env::var_os("TIMEWARP_TEST_VIDEO") else { return };
        let dst = std::env::temp_dir().join("timewarp-clip-discord.mp4");
        let opts = ClipSettings { format: ClipFormat::Shareable, max_mb: Some(10) };
        export_with(Path::new(&src), &dst, 2.0, 17.0, &opts).expect("export");
        assert!(std::fs::metadata(&dst).unwrap().len() <= 10_000_000);
    }

    #[test]
    fn clip_names_are_filesystem_safe() {
        let p = clip_path(Path::new("/out"), Path::new("/out/2026_LeeSin_Win.mp4"), "Killed by Kha'Zix!", 125.0);
        assert_eq!(p, Path::new("/out/Clips/2026_LeeSin_Win_Killed-by-Kha-Zix_02-05.mp4"));
    }
}

#[cfg(test)]
mod demo {
    use super::*;

    /// `TIMEWARP_TEST_VIDEO=… TIMEWARP_DEMO_DIR=… cargo test -- --ignored demo_clips`
    #[test]
    #[ignore]
    fn demo_clips() {
        let (Some(src), Some(dir)) = (std::env::var_os("TIMEWARP_TEST_VIDEO"), std::env::var_os("TIMEWARP_DEMO_DIR")) else {
            return;
        };
        let out = PathBuf::from(dir);
        let demos = [
            ("Lee Sin: Killed Graves", Some(MarkerKind::Kill), "Graves", 2.0),
            ("Lee Sin: Killed by Darius", Some(MarkerKind::Death), "Graves", 8.0),
            ("Ahri: Dragon", Some(MarkerKind::Objective), "Syndra", 12.0),
            ("Jinx: Killed Caitlyn", Some(MarkerKind::Kill), "Caitlyn", 15.0),
        ];
        for (i, (title, kind, vs, at)) in demos.into_iter().enumerate() {
            let dst = clip_path(&out, Path::new("demo.mp4"), title, at);
            export(Path::new(&src), &dst, at, at + 6.0).unwrap();
            let champ = title.split(':').next().unwrap().to_string();
            write_meta(&dst, &ClipMeta {
                title: title.into(),
                kind,
                source: "2026-11-05_14-00-00_LeeSin_Win.mp4".into(),
                start: at,
                end: at + 6.0,
                champion_key: Some(champ.replace(' ', "")),
                champion: Some(champ),
                opponent: Some(vs.into()),
                result: Some("Win".into()),
                queue: Some("Ranked Solo/Duo".into()),
                game_time: Some(300.0 + i as f64 * 200.0),
                created_at: Some(chrono::Local::now() - chrono::Duration::hours(i as i64 * 5)),
                ..Default::default()
            });
            make_thumbnail(&dst);
            assert!(thumb_path(&dst).exists(), "thumbnail for {title}");
        }
    }
}
