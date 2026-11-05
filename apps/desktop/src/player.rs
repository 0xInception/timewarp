//! Video playback: AVPlayer (hardware decode + audio) feeding decoded frames
//! to gpui's `surface()` element as IOSurface-backed CVPixelBuffers. No
//! copies, no software decode, no web view.

use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_av_foundation::{AVPlayer, AVPlayerItem, AVPlayerItemOutput, AVPlayerItemVideoOutput};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use objc2_quartz_core::CACurrentMediaTime;

pub type Frame = CFRetained<CVPixelBuffer>;

pub struct VideoPlayer {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    frame: Option<Frame>,
    frame_time: f64,
}

fn cf_key(key: &CFString) -> &NSString {
    // CFString and NSString are toll-free bridged.
    unsafe { &*(key as *const CFString as *const NSString) }
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Self {
        let mtm = MainThreadMarker::new().expect("player must be created on the main thread");
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);

            // gpui's Metal surface shader expects full-range bi-planar 4:2:0.
            let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
            let metal = NSNumber::new_bool(true);
            let iosurface = NSDictionary::<NSString, AnyObject>::new();
            let keys = [
                cf_key(kCVPixelBufferPixelFormatTypeKey),
                cf_key(kCVPixelBufferMetalCompatibilityKey),
                cf_key(kCVPixelBufferIOSurfacePropertiesKey),
            ];
            let values: [&AnyObject; 3] = [&format, &metal, &iosurface];
            let attrs = NSDictionary::from_slices(&keys, &values);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attrs),
            );
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            Self { player, item, output, frame: None, frame_time: 0.0 }
        }
    }

    pub fn is_playing(&self) -> bool {
        unsafe { self.player.rate() != 0.0 }
    }

    pub fn play(&self) {
        unsafe { self.player.play() }
    }

    pub fn pause(&self) {
        unsafe { self.player.pause() }
    }

    pub fn toggle(&self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    pub fn position(&self) -> f64 {
        let s = unsafe { self.player.currentTime().seconds() };
        if s.is_finite() { s } else { 0.0 }
    }

    /// `None` until the asset has loaded.
    pub fn duration(&self) -> Option<f64> {
        let s = unsafe { self.item.duration().seconds() };
        (s.is_finite() && s > 0.0).then_some(s)
    }

    /// Speed that `play()` uses from now on (also applied right away).
    pub fn set_speed(&self, speed: f32) {
        unsafe {
            self.player.setDefaultRate(speed);
            if self.is_playing() {
                self.player.setRate(speed);
            }
        }
    }

    /// Pixel size of the decoded picture (for mapping drawings onto it).
    pub fn frame_size(&self) -> Option<(f32, f32)> {
        let f = self.frame.as_ref()?;
        let (w, h) = (objc2_core_video::CVPixelBufferGetWidth(f), objc2_core_video::CVPixelBufferGetHeight(f));
        (w > 0 && h > 0).then_some((w as f32, h as f32))
    }

    pub fn rate(&self) -> f32 {
        unsafe { self.player.rate() }
    }

    /// Playback speed (negative plays backwards where the file allows it).
    pub fn set_rate(&self, rate: f32) {
        unsafe { self.player.setRate(rate) }
    }

    pub fn can_play_reverse(&self) -> bool {
        unsafe { self.item.canPlayReverse() }
    }

    /// Step whole frames (pauses playback).
    pub fn step(&self, frames: isize) {
        unsafe {
            self.player.pause();
            self.item.stepByCount(frames);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe { self.player.setMuted(muted) }
    }

    /// Fast, approximate seek (nearest keyframe within a second): for previews.
    pub fn seek_fast(&self, secs: f64) {
        unsafe {
            let t = CMTime::with_seconds(secs.max(0.0), 600);
            let slack = CMTime::with_seconds(1.0, 600);
            self.player.seekToTime_toleranceBefore_toleranceAfter(t, slack, slack);
        }
    }

    /// Video time of the frame last returned by `frame()`.
    pub fn frame_time(&self) -> f64 {
        self.frame_time
    }

    /// Frame-accurate seek.
    pub fn seek(&self, secs: f64) {
        unsafe {
            let t = CMTime::with_seconds(secs.max(0.0), 600);
            let zero = CMTime::with_seconds(0.0, 600);
            self.player.seekToTime_toleranceBefore_toleranceAfter(t, zero, zero);
        }
    }

    /// Latest decoded frame for the current display time.
    pub fn frame(&mut self) -> Option<Frame> {
        unsafe {
            let output: &AVPlayerItemOutput = &self.output;
            let t = output.itemTimeForHostTime(CACurrentMediaTime());
            if self.output.hasNewPixelBufferForItemTime(t) {
                let mut shown = t;
                if let Some(buf) = self.output.copyPixelBufferForItemTime_itemTimeForDisplay(t, &mut shown) {
                    let secs = shown.seconds();
                    self.frame_time = if secs.is_finite() { secs } else { self.position() };
                    // objc2 `Retained` -> CF `CFRetained` (same +1 reference).
                    let raw = Retained::into_raw(buf);
                    self.frame = Some(CFRetained::from_raw(NonNull::new_unchecked(raw)));
                }
            }
        }
        self.frame.clone()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.pause();
    }
}
