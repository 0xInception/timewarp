//! Headless recorder. Useful for measuring FPS impact with no UI running.
//!
//!   timewarp record [SECONDS] [--display]   record League (or the display) now
//!   timewarp watch                          auto-record every match until Ctrl-C

use std::path::PathBuf;
use std::time::Duration;

use timewarp_capture::{CaptureTarget, Recording};
use timewarp_engine::{Config, Phase};

fn main() -> anyhow::Result<()> {

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = Config::load();

    match args.first().map(String::as_str) {
        Some("record") => {
            let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
            let target = if args.iter().any(|a| a == "--display") {
                CaptureTarget::MainDisplay
            } else {
                CaptureTarget::App { bundle_id: timewarp_league::GAME_BUNDLE_ID.into() }
            };
            std::fs::create_dir_all(&config.output_dir)?;
            let path: PathBuf = config.output_dir.join("test-recording.mp4");
            let _ = std::fs::remove_file(&path);
            let rec = Recording::start(&target, &config.capture, &path)?;
            std::thread::sleep(Duration::from_secs(secs));
            if let Some(err) = rec.has_failed() {
                log::error!("stream failed during recording: {err}");
            }
            let done = rec.stop()?;
            println!("{} ({:.1}s, {:.1} MB)", done.path.display(), done.duration.as_secs_f64(), done.bytes as f64 / 1e6);
        }
        Some("watch") => {
            let engine = timewarp_engine::spawn(config);
            let mut last = Phase::Idle;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let s = engine.status();
                if s.phase != last {
                    log::info!("{:?}", s.phase);
                    last = s.phase.clone();
                }
                if let Some(e) = &s.last_error {
                    log::warn!("{e}");
                }
            }
        }
        _ => {
            eprintln!("usage: timewarp record [SECONDS] [--display] | timewarp watch");
            std::process::exit(2);
        }
    }
    Ok(())
}
