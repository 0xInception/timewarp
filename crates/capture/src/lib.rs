//! Platform capture backends.
//!
//! The public surface is deliberately tiny: start a [`Recording`] against a
//! [`CaptureTarget`], stop it, get a file. Each OS implements it with the
//! cheapest path its compositor offers:
//!
//! * macOS: ScreenCaptureKit + `SCRecordingOutput` (WindowServer hands
//!   IOSurfaces straight to the hardware media engine; zero copies in our
//!   process, no overlay windows, nothing injected into the game).
//! * Windows (planned): Windows.Graphics.Capture + Media Foundation / NVENC /
//!   AMF / QSV hardware encoders.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod mac;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    /// Smaller files, hardware-encoded on every Apple Silicon Mac.
    Hevc,
    /// Larger files, maximum compatibility (Discord previews, older editors).
    H264,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub fps: u32,
    /// Downscale so the output is at most this many pixels tall. `None` keeps
    /// native pixels (expensive on 5K/6K Retina panels for no visible gain).
    pub max_height: Option<u32>,
    pub codec: Codec,
    pub game_audio: bool,
    pub microphone: bool,
    pub show_cursor: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            fps: 60,
            max_height: Some(1080),
            codec: Codec::Hevc,
            game_audio: true,
            microphone: false,
            show_cursor: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CaptureTarget {
    /// Only this application's windows (and audio) on whichever display its
    /// main window is on. Notifications, Discord, etc. are never recorded.
    App { bundle_id: String },
    /// The whole primary display. Mostly for testing without the game.
    MainDisplay,
}

#[derive(Debug, Clone)]
pub struct FinishedRecording {
    pub path: PathBuf,
    pub duration: Duration,
    pub bytes: u64,
}

/// An in-flight recording. Dropping it without calling [`Recording::stop`]
/// still stops the stream, but the file may not be finalized cleanly.
pub struct Recording {
    #[cfg(target_os = "macos")]
    inner: mac::MacRecording,
}

impl Recording {
    pub fn start(target: &CaptureTarget, settings: &CaptureSettings, path: &Path) -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self { inner: mac::MacRecording::start(target, settings, path)? })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (target, settings, path);
            anyhow::bail!("capture backend not implemented for this platform yet")
        }
    }

    /// True once the OS reports the stream died (display change, game window
    /// recreated on another display, permission revoked...). The engine uses
    /// this to roll over into a new segment.
    pub fn has_failed(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        return self.inner.failure();
        #[cfg(not(target_os = "macos"))]
        None
    }

    pub fn elapsed(&self) -> Duration {
        #[cfg(target_os = "macos")]
        return self.inner.elapsed();
        #[cfg(not(target_os = "macos"))]
        Duration::ZERO
    }

    pub fn stop(self) -> anyhow::Result<FinishedRecording> {
        #[cfg(target_os = "macos")]
        return self.inner.stop();
        #[cfg(not(target_os = "macos"))]
        unreachable!()
    }
}

/// Whether the target app currently has a capturable on-screen window.
pub fn target_is_visible(target: &CaptureTarget) -> bool {
    #[cfg(target_os = "macos")]
    return mac::target_is_visible(target);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        false
    }
}

/// Fit `(w, h)` under `max_height`, keeping aspect ratio and even dimensions
/// (hardware encoders require 4:2:0 chroma-aligned sizes).
pub fn fit_dimensions(w: u32, h: u32, max_height: Option<u32>) -> (u32, u32) {
    let (w, h) = match max_height {
        Some(max) if h > max => {
            let scale = max as f64 / h as f64;
            ((w as f64 * scale).round() as u32, max)
        }
        _ => (w, h),
    };
    (w & !1, h & !1)
}

#[cfg(test)]
mod tests {
    use super::fit_dimensions;

    #[test]
    fn fits() {
        assert_eq!(fit_dimensions(3456, 2234, Some(1080)), (1670, 1080));
        assert_eq!(fit_dimensions(1920, 1080, Some(1080)), (1920, 1080));
        assert_eq!(fit_dimensions(1281, 721, None), (1280, 720));
    }
}
