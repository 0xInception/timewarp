//! ScreenCaptureKit backend.
//!
//! Why this is cheap for the game:
//! * No overlay window is ever placed above the game, so focus, mouse capture
//!   and edge-pan camera behave exactly as without a recorder.
//! * Nothing is injected into or read from the game process.
//! * Frames go WindowServer -> IOSurface -> media engine (hardware HEVC/H.264)
//!   -> file entirely inside `SCRecordingOutput`. We register no frame
//!   handlers, so no pixel ever crosses into this process.
//! * NV12 (`420v`) is requested so the encoder needs no colour conversion pass.
//! * A small queue depth bounds the surfaces WindowServer keeps in flight.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    RecordingCallbacks, SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};

use crate::{CaptureSettings, CaptureTarget, Codec, FinishedRecording, fit_dimensions};

pub(crate) struct MacRecording {
    stream: SCStream,
    output: SCRecordingOutput,
    path: std::path::PathBuf,
    started: Instant,
    failure: Arc<Mutex<Option<String>>>,
}

impl MacRecording {
    pub fn start(target: &CaptureTarget, settings: &CaptureSettings, path: &Path) -> anyhow::Result<Self> {
        if !SCRecordingOutput::is_available() {
            bail!("SCRecordingOutput requires macOS 15 or newer");
        }

        let content = SCShareableContent::get().context("screen recording permission not granted?")?;
        let filter = build_filter(&content, target)?;

        let scale = f64::from(filter.point_pixel_scale().max(1.0));
        let rect = filter.content_rect();
        let (w, h) = fit_dimensions(
            (rect.size.width * scale).round() as u32,
            (rect.size.height * scale).round() as u32,
            settings.max_height,
        );

        let mut config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_fps(settings.fps)
            .with_pixel_format(PixelFormat::YCbCr_420v)
            .with_queue_depth(4)
            .with_shows_cursor(settings.show_cursor)
            .with_captures_audio(settings.game_audio)
            .with_excludes_current_process_audio(true)
            .with_sample_rate(48_000)
            .with_channel_count(2);
        if settings.microphone {
            config = config.with_captures_microphone(true)?;
        }

        let failure = Arc::new(Mutex::new(None::<String>));

        let rec_config = SCRecordingOutputConfiguration::new()?
            .with_output_url(path)
            .map_err(|e| anyhow!("invalid output path {}: {e:?}", path.display()))?
            .with_video_codec(match settings.codec {
                Codec::Hevc => SCRecordingOutputCodec::HEVC,
                Codec::H264 => SCRecordingOutputCodec::H264,
            })
            .with_output_file_type(SCRecordingOutputFileType::MP4);

        let fail_slot = failure.clone();
        let callbacks = RecordingCallbacks::new().on_fail(move |err| {
            log::error!("recording output failed: {err}");
            *fail_slot.lock().unwrap() = Some(err);
        });
        let output = SCRecordingOutput::new_with_delegate(&rec_config, callbacks)
            .ok_or_else(|| anyhow!("failed to create SCRecordingOutput"))?;

        let fail_slot = failure.clone();
        let stream = SCStream::new_with_delegate(
            &filter,
            &config,
            ErrorHandler::new(move |err: SCError| {
                log::error!("capture stream stopped: {err}");
                *fail_slot.lock().unwrap() = Some(err.to_string());
            }),
        )?;
        stream.add_recording_output(&output)?;
        stream.start_capture()?;

        log::info!("recording {w}x{h}@{} -> {}", settings.fps, path.display());
        Ok(Self { stream, output, path: path.to_owned(), started: Instant::now(), failure })
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn stop(self) -> anyhow::Result<FinishedRecording> {
        let duration = self.started.elapsed();
        // With no frame handlers attached, removing the last recording output
        // stops the stream and blocks until the movie file is finalized.
        if let Err(e) = self.stream.remove_recording_output(&self.output) {
            log::warn!("remove_recording_output: {e}");
            let _ = self.stream.stop_capture();
        }
        let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(FinishedRecording { path: self.path, duration, bytes })
    }
}

fn build_filter(content: &SCShareableContent, target: &CaptureTarget) -> anyhow::Result<SCContentFilter> {
    let displays = content.displays();
    match target {
        CaptureTarget::MainDisplay => {
            let display = displays.first().ok_or_else(|| anyhow!("no displays"))?;
            Ok(SCContentFilter::create().with_display(display).with_excluding_windows(&[]).build()?)
        }
        CaptureTarget::App { bundle_id } => {
            let app = content
                .applications()
                .into_iter()
                .find(|a| a.bundle_identifier() == *bundle_id)
                .ok_or_else(|| anyhow!("{bundle_id} is not running"))?;
            let window = main_window(content, bundle_id).ok_or_else(|| anyhow!("{bundle_id} has no visible window"))?;
            let f = window.frame();
            let center = CGPoint { x: f.origin.x + f.size.width / 2.0, y: f.origin.y + f.size.height / 2.0 };
            let display = displays
                .iter()
                .find(|d| d.frame().contains_point(center))
                .or(displays.first())
                .ok_or_else(|| anyhow!("no displays"))?;
            // Display + app filter rather than a single-window filter: League
            // destroys/recreates its window between loading screen and game and
            // when toggling fullscreen; an app filter survives that.
            Ok(SCContentFilter::create().with_display(display).with_including_applications(&[&app], &[]).build()?)
        }
    }
}

/// Largest real-sized window belonging to `bundle_id`. League fullscreen sits at
/// layer 1000 and reports off-screen whenever another Space is frontmost, so
/// neither layer nor on-screen state is filtered on.
fn main_window(content: &SCShareableContent, bundle_id: &str) -> Option<SCWindow> {
    content
        .windows()
        .into_iter()
        .filter(|w| w.owning_application().is_some_and(|a| a.bundle_identifier() == bundle_id))
        .max_by(|a, b| {
            let area = |w: &SCWindow| w.frame().size.width * w.frame().size.height;
            area(a).total_cmp(&area(b))
        })
        .filter(|w| w.frame().size.width >= 320.0 && w.frame().size.height >= 240.0)
}

pub(crate) fn target_is_visible(target: &CaptureTarget) -> bool {
    match target {
        CaptureTarget::MainDisplay => true,
        // Only when it's actually on screen: macOS doesn't draw a fullscreen
        // game in another Space, so recording then only captures black.
        CaptureTarget::App { bundle_id } => SCShareableContent::get()
            .ok()
            .and_then(|c| main_window(&c, bundle_id))
            .is_some_and(|w| w.is_on_screen()),
    }
}
