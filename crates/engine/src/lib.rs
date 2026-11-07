//! The recorder brain: watches for a League match, records it, tags
//! highlights, and writes a sidecar JSON next to every video.
//!
//! Runs on its own low-priority thread; the UI only reads [`Status`] snapshots
//! and sends [`Command`]s, so a busy/frozen UI can never stall recording.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use timewarp_capture::{CaptureSettings, CaptureTarget, Recording};
use timewarp_league::{GAME_BUNDLE_ID, LiveClient, Marker, Me, ProcessWatcher, classify};

pub use timewarp_capture::Codec;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    pub auto_record: bool,
    pub capture: CaptureSettings,
    pub storage: StoragePolicy,
    /// In-game bookmark hotkey, e.g. "F8". `None` disables it.
    pub bookmark_key: Option<String>,
    /// Which matches auto-record picks up.
    pub record: RecordFilter,
    /// How exported clips are encoded (separate from recordings).
    pub clips: ClipSettings,
    /// Remembered layout of the desktop app.
    pub layout: Layout,
    /// Look for new versions once a day and say so. Off until the user turns
    /// it on; never installs by itself.
    pub check_updates: bool,
    /// A version the user chose to skip: no reminders for it.
    pub skip_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Width of the review's side panel (moments, clips, notes), in points.
    pub review_panel_width: f32,
    pub review_panel_open: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self { review_panel_width: 380.0, review_panel_open: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipFormat {
    /// HEVC: the same picture as H.264 in about 30% fewer bytes. Plays inline
    /// in Discord on Macs, iPhones and most recent PCs and phones.
    #[default]
    Compact,
    /// H.264: plays everywhere, including old PCs and browsers.
    Shareable,
    /// Cut straight from the recording, instant, same codec as recordings.
    Original,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipSettings {
    pub format: ClipFormat,
    /// Shrink clips until they fit (Discord free: 10 MB).
    pub max_mb: Option<u32>,
    /// 30 fps halves the frames: sharper clips at the same file size.
    pub fps: ClipFps,
    /// Highest resolution for clips (smaller files, faster uploads).
    pub resolution: ClipResolution,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipFps {
    /// Same as the recording (usually 60).
    #[default]
    Original,
    Fps30,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipResolution {
    #[default]
    Original,
    P1080,
    P720,
    P480,
}

impl ClipResolution {
    pub fn max_height(self) -> Option<f64> {
        match self {
            Self::Original => None,
            Self::P1080 => Some(1080.0),
            Self::P720 => Some(720.0),
            Self::P480 => Some(480.0),
        }
    }
}

impl ClipSettings {
    /// Smaller than the source in frame rate or size: needs a re-encode.
    pub fn reduces(&self) -> bool {
        self.fps == ClipFps::Fps30 || self.resolution != ClipResolution::Original
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordFilter {
    #[default]
    All,
    /// Skip Practice Tool and custom games.
    SkipPractice,
    RankedOnly,
}

impl RecordFilter {
    /// Unknown queue (client API unavailable) records, rather than silently missing a game.
    pub fn allows(self, info: Option<&timewarp_league::lcu::MatchInfo>) -> bool {
        let Some(i) = info else { return true };
        match self {
            RecordFilter::All => true,
            RecordFilter::SkipPractice => !i.custom && i.game_mode.as_deref() != Some("PRACTICETOOL"),
            RecordFilter::RankedOnly => i.ranked,
        }
    }
}

/// Automatic cleanup of old recordings. Both limits off by default: nothing
/// is ever deleted unless the user opts in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoragePolicy {
    /// Keep the library under this many gigabytes (oldest go first).
    pub max_gb: Option<u32>,
    /// Delete recordings older than this many days.
    pub max_days: Option<u32>,
    /// Cut games down to their moments once they're this many days old
    /// (0 = right after the game).
    pub condense_after_days: Option<u32>,
    /// Which auto-tagged moments survive condensing. Your notes, bookmarks
    /// and saved moments are always kept.
    pub keep: KeepKinds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeepKinds {
    pub kills: bool,
    pub deaths: bool,
    pub assists: bool,
    pub objectives: bool,
}

impl Default for KeepKinds {
    fn default() -> Self {
        Self { kills: true, deaths: true, assists: false, objectives: true }
    }
}

impl KeepKinds {
    pub fn keeps(&self, kind: timewarp_league::MarkerKind) -> bool {
        use timewarp_league::MarkerKind::*;
        match kind {
            Kill | Multikill | Ace => self.kills,
            Death => self.deaths,
            Assist => self.assists,
            Objective | Structure => self.objectives,
            Bookmark => true,
            GameEnd => false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: dirs::video_dir().unwrap_or_else(|| PathBuf::from(".")).join("Timewarp"),
            auto_record: true,
            capture: CaptureSettings::default(),
            storage: StoragePolicy::default(),
            bookmark_key: Some("F8".into()),
            record: RecordFilter::All,
            clips: ClipSettings::default(),
            layout: Layout::default(),
            check_updates: false,
            skip_version: None,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Timewarp").join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let p = Self::path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(p, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public status / commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for a match to start.
    Idle,
    /// Game process exists but its window is not capturable yet.
    WaitingForWindow,
    /// Record was pressed with no game running: recording starts as soon as
    /// League's game window appears. Only League is ever recorded.
    Armed,
    Recording,
    Finalizing,
    /// A match is running but the record filter excludes it.
    Skipped,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub phase: Phase,
    pub current_file: Option<PathBuf>,
    pub elapsed: Duration,
    pub markers: usize,
    pub champion: Option<String>,
    pub last_error: Option<String>,
    pub last_finished: Option<PathBuf>,
    /// Bookmarks placed this session (UI confirms each one).
    pub bookmarks: usize,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            bookmarks: 0,
            phase: Phase::Idle,
            current_file: None,
            elapsed: Duration::ZERO,
            markers: 0,
            champion: None,
            last_error: None,
            last_finished: None,
        }
    }
}

pub enum Command {
    /// Start recording right now (main display if no game is running).
    StartManual,
    Stop,
    /// Mark "this moment" in the current recording (bookmark hotkey).
    Bookmark,
    SetConfig(Config),
    Shutdown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMeta {
    pub started_at: chrono::DateTime<chrono::Local>,
    pub duration_secs: f64,
    pub champion: Option<String>,
    pub game_mode: Option<String>,
    /// Client queue name, e.g. "Ranked Solo/Duo".
    pub queue: Option<String>,
    pub patch: Option<String>,
    /// "Win" / "Lose" from the GameEnd event.
    pub result: Option<String>,
    /// Final (last seen) line for the local player.
    pub player: Option<PlayerLine>,
    /// Enemy in the same role (jungle vs jungle, mid vs mid...).
    pub opponent: Option<PlayerLine>,
    /// Everyone in the match, for "ally top / enemy jungle" filtering.
    pub roster: Vec<RosterEntry>,
    /// Video timestamp (seconds) = game_time + this offset.
    pub game_time_offset: Option<f64>,
    pub markers: Vec<VideoMarker>,
    /// Set once the video was cut down to its moments.
    pub condensed: Option<Condensed>,
    /// Your role as set by hand (wins over what League reported or we inferred).
    pub role_override: Option<String>,
    /// Riot match id, when the League client reported one.
    pub game_id: Option<i64>,
    /// "pid-starttime" of the game process: same match, even without the client.
    pub process_id: Option<String>,
    /// For joined videos: when the last piece really ended, and the game
    /// clock at that point (the joined length skips the gaps between pieces).
    pub ended_at: Option<chrono::DateTime<chrono::Local>>,
    pub game_clock_end: Option<f64>,
}

impl SessionMeta {
    /// Whether two recordings are pieces of the same match.
    pub fn same_match(&self, other: &SessionMeta) -> Option<bool> {
        match (self.game_id, other.game_id) {
            (Some(a), Some(b)) => return Some(a == b),
            _ => {}
        }
        match (&self.process_id, &other.process_id) {
            (Some(a), Some(b)) => Some(a == b),
            _ => None,
        }
    }
}

pub const ROLES: [&str; 5] = ["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];

impl SessionMeta {
    /// Fill in your role and lane opponent when League didn't assign you a
    /// position (Practice Tool, customs): the one role missing from your
    /// team is yours. A manual override always wins.
    pub fn infer_role(&mut self) {
        let Some(p) = self.player.as_mut() else { return };
        let role = self.role_override.clone().or_else(|| {
            if ROLES.contains(&p.position.as_str()) {
                return Some(p.position.clone());
            }
            let taken: Vec<&str> = self.roster.iter().filter(|r| r.ally && !r.me).map(|r| r.position.as_str()).collect();
            let missing: Vec<&str> = ROLES.iter().copied().filter(|r| !taken.contains(r)).collect();
            (missing.len() == 1 && !taken.is_empty()).then(|| missing[0].to_string())
        });
        let Some(role) = role else { return };
        p.position = role.clone();
        if let Some(me) = self.roster.iter_mut().find(|r| r.me) {
            me.position = role.clone();
        }
        if self.opponent.as_ref().is_none_or(|o| o.position != role) {
            self.opponent = self
                .roster
                .iter()
                .find(|r| !r.ally && r.position == role && !r.champion.is_empty())
                .map(|r| PlayerLine {
                    champion: r.champion.clone(),
                    champion_key: r.champion_key.clone(),
                    position: role.clone(),
                    ..Default::default()
                });
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Condensed {
    /// Length of the full recording before condensing.
    pub original_duration_secs: f64,
    pub original_bytes: u64,
    /// Kept ranges of the original, in order, and where each starts now.
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    /// Start of this range in the condensed video.
    pub at: f64,
}

impl Condensed {
    /// Original video time -> condensed video time, if it was kept.
    pub fn map(&self, t: f64) -> Option<f64> {
        self.segments.iter().find(|s| t >= s.start && t <= s.end).map(|s| s.at + (t - s.start))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerLine {
    pub riot_id: String,
    pub champion: String,
    /// Data Dragon key for icons.
    pub champion_key: String,
    pub position: String,
    pub level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub cs: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RosterEntry {
    pub champion: String,
    pub champion_key: String,
    /// TOP / JUNGLE / MIDDLE / BOTTOM / UTILITY, or NONE/empty when unassigned.
    pub position: String,
    pub ally: bool,
    pub me: bool,
}

impl PlayerLine {
    fn from(p: &timewarp_league::Player) -> Self {
        Self {
            riot_id: p.riot_id.clone(),
            champion: p.champion_name.clone(),
            champion_key: p.champion_key(),
            position: p.position.clone(),
            level: p.level,
            kills: p.scores.kills,
            deaths: p.scores.deaths,
            assists: p.scores.assists,
            cs: p.scores.creep_score,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMarker {
    #[serde(flatten)]
    pub marker: Marker,
    pub video_time: f64,
}

#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Command>,
    status: Arc<Mutex<Status>>,
    thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl EngineHandle {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Stop any recording, finalize its file, and wait for the engine to exit.
    /// Call before the process quits, or an in-progress MP4 is left unplayable.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}

pub fn spawn(config: Config) -> EngineHandle {
    let (tx, rx) = mpsc::channel();
    let status = Arc::new(Mutex::new(Status::default()));
    let st = status.clone();
    let thread = thread::Builder::new()
        .name("timewarp-engine".into())
        .spawn(move || Engine::new(config, rx, st).run())
        .expect("spawn engine thread");
    EngineHandle { tx, status, thread: Arc::new(Mutex::new(Some(thread))) }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct Session {
    rec: Recording,
    path: PathBuf,
    manual: bool,
    meta: SessionMeta,
    me: Option<Me>,
    last_event_id: Option<u64>,
    polls: u64,
    lcu_checked: bool,
}

struct Engine {
    config: Config,
    rx: Receiver<Command>,
    status: Arc<Mutex<Status>>,
    procs: ProcessWatcher,
    live: LiveClient,
    session: Option<Session>,
    /// Suppresses auto-record for the current match after a manual stop.
    suppressed_pid: Option<u32>,
    /// Record pressed before the game was up: start when it appears.
    armed: bool,
    /// Record-filter decision for this match (asked once per game process).
    allowed_pid: Option<(u32, bool)>,
}

impl Engine {
    fn new(config: Config, rx: Receiver<Command>, status: Arc<Mutex<Status>>) -> Self {
        Self {
            config,
            rx,
            status,
            procs: ProcessWatcher::new(),
            live: LiveClient::new(),
            session: None,
            suppressed_pid: None,
            armed: false,
            allowed_pid: None,
        }
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }

    fn run(mut self) {
        loop {
            // Poll faster while recording (event markers), slower when idle.
            let tick = if self.session.is_some() { Duration::from_secs(1) } else { Duration::from_secs(2) };
            match self.rx.recv_timeout(tick) {
                Ok(Command::StartManual) => {
                    if self.session.is_none() {
                        // Never the whole screen: record League's game now if
                        // it's up, otherwise as soon as it appears.
                        let target = CaptureTarget::App { bundle_id: GAME_BUNDLE_ID.into() };
                        if self.procs.game_pid().is_some() && timewarp_capture::target_is_visible(&target) {
                            self.start(target, true);
                        } else {
                            self.armed = true;
                            self.suppressed_pid = None;
                            self.set(|s| s.phase = Phase::Armed);
                        }
                    }
                }
                Ok(Command::Stop) => {
                    if self.armed && self.session.is_none() {
                        self.armed = false;
                        self.set(|s| s.phase = Phase::Idle);
                        continue;
                    }
                    self.suppressed_pid = self.procs.game_pid();
                    self.finish();
                }
                Ok(Command::Bookmark) => self.bookmark(),
                Ok(Command::SetConfig(c)) => self.config = c,
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    self.finish();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
        }
    }

    fn tick(&mut self) {
        let pid = self.procs.game_pid();
        if pid.is_none() {
            self.suppressed_pid = None;
        }

        match &mut self.session {
            None => {
                let Some(pid) = pid else {
                    let phase = if self.armed { Phase::Armed } else { Phase::Idle };
                    self.set(|s| s.phase = phase);
                    return;
                };
                let target = CaptureTarget::App { bundle_id: GAME_BUNDLE_ID.into() };
                // Armed by the record button: this match records regardless
                // of auto-record and the match filter.
                if self.armed {
                    if timewarp_capture::target_is_visible(&target) {
                        self.armed = false;
                        self.start(target, false);
                    } else {
                        self.set(|s| s.phase = Phase::WaitingForWindow);
                    }
                    return;
                }
                if !self.config.auto_record || self.suppressed_pid == Some(pid) {
                    return;
                }
                let allowed = match self.allowed_pid {
                    Some((p, ok)) if p == pid => ok,
                    _ => {
                        let ok = self.config.record.allows(timewarp_league::lcu::match_info().as_ref());
                        self.allowed_pid = Some((pid, ok));
                        if !ok {
                            log::info!("skipping this match ({:?} filter)", self.config.record);
                        }
                        ok
                    }
                };
                if !allowed {
                    self.set(|s| s.phase = Phase::Skipped);
                    return;
                }
                if timewarp_capture::target_is_visible(&target) {
                    self.start(target, false);
                } else {
                    self.set(|s| s.phase = Phase::WaitingForWindow);
                }
            }
            Some(sess) => {
                if !sess.manual && pid.is_none() {
                    self.finish();
                    return;
                }
                if let Some(err) = sess.rec.has_failed() {
                    // Stream died (display change, fullscreen toggle onto another
                    // monitor...). Close this file; the next tick starts a new one.
                    self.set(|s| s.last_error = Some(err));
                    self.finish();
                    return;
                }
                poll_game_data(&self.live, sess, false);
                let (elapsed, markers, champ) =
                    (sess.rec.elapsed(), sess.meta.markers.len(), sess.meta.champion.clone());
                self.set(|s| {
                    s.elapsed = elapsed;
                    s.markers = markers;
                    s.champion = champ;
                });
            }
        }
    }

    fn start(&mut self, target: CaptureTarget, manual: bool) {
        let started_at = chrono::Local::now();
        if let Err(e) = std::fs::create_dir_all(&self.config.output_dir) {
            self.set(|s| s.last_error = Some(format!("cannot create output dir: {e}")));
            return;
        }
        let path = self.config.output_dir.join(format!("{}.mp4", started_at.format("%Y-%m-%d_%H-%M-%S")));
        match Recording::start(&target, &self.config.capture, &path) {
            Ok(rec) => {
                self.set(|s| {
                    s.phase = Phase::Recording;
                    s.current_file = Some(path.clone());
                    s.markers = 0;
                    s.champion = None;
                    s.last_error = None;
                });
                self.session = Some(Session {
                    rec,
                    path,
                    manual,
                    meta: SessionMeta {
                        started_at,
                        process_id: self.procs.game().map(|(pid, start)| format!("{pid}-{start}")),
                        ..Default::default()
                    },
                    me: None,
                    last_event_id: None,
                    polls: 0,
                    lcu_checked: false,
                });
            }
            Err(e) => {
                log::error!("failed to start recording: {e:#}");
                self.set(|s| s.last_error = Some(format!("{e:#}")));
            }
        }
    }

    fn bookmark(&mut self) {
        let Some(sess) = &mut self.session else { return };
        let video_time = sess.rec.elapsed().as_secs_f64();
        let offset = sess.meta.game_time_offset.unwrap_or(0.0);
        sess.meta.markers.push(VideoMarker {
            marker: Marker {
                kind: timewarp_league::MarkerKind::Bookmark,
                label: "Marked during the game".into(),
                game_time: (video_time - offset).max(0.0),
            },
            video_time,
        });
        let n = sess.meta.markers.len();
        self.set(|s| {
            s.markers = n;
            s.bookmarks += 1;
        });
    }

    fn finish(&mut self) {
        let Some(mut sess) = self.session.take() else { return };
        self.set(|s| s.phase = Phase::Finalizing);
        // One last event pull so the GameEnd marker / result make it in.
        poll_game_data(&self.live, &mut sess, true);
        match sess.rec.stop() {
            Ok(done) => {
                sess.meta.duration_secs = done.duration.as_secs_f64();
                let final_path = rename_with_champion(&sess.path, &sess.meta);
                if let Err(e) = write_sidecar(&final_path, &sess.meta) {
                    log::warn!("sidecar: {e}");
                }
                log::info!("saved {} ({:.1} MB)", final_path.display(), done.bytes as f64 / 1e6);
                self.set(|s| s.last_finished = Some(final_path));
            }
            Err(e) => self.set(|s| s.last_error = Some(format!("{e:#}"))),
        }
        self.set(|s| {
            s.phase = Phase::Idle;
            s.current_file = None;
            s.elapsed = Duration::ZERO;
        });
    }
}

fn poll_game_data(live: &LiveClient, sess: &mut Session, force: bool) {
    // One `allgamedata` request every 3s is plenty: event timestamps come from
    // the game clock, so polling cadence doesn't affect marker accuracy.
    sess.polls += 1;
    if !force && sess.polls % 3 != 1 {
        return;
    }
    // The API only answers once the match is loaded; failures before that are
    // expected and silent.
    let Ok(data) = live.all_game_data() else { return };
    let video_now = sess.rec.elapsed().as_secs_f64();
    if sess.meta.game_time_offset.is_none() {
        sess.meta.game_time_offset = Some(video_now - data.game_data.game_time);
        sess.meta.game_mode = Some(data.game_data.game_mode.clone());
    }
    if !sess.lcu_checked {
        sess.lcu_checked = true;
        if let Some(info) = timewarp_league::lcu::match_info() {
            sess.meta.queue = info.queue;
            sess.meta.patch = info.patch;
            sess.meta.game_id = info.game_id;
        }
    }
    if let Some(p) = data.me() {
        sess.meta.champion = Some(p.champion_name.clone());
        sess.meta.player = Some(PlayerLine::from(p));
    }
    sess.meta.opponent = data.lane_opponent().map(PlayerLine::from);
    if let Some(me) = data.me() {
        sess.meta.roster = data
            .all_players
            .iter()
            // Practice Tool lists a nameless training dummy.
            .filter(|p| !p.champion_name.is_empty())
            .map(|p| RosterEntry {
                champion: p.champion_name.clone(),
                champion_key: p.champion_key(),
                position: p.position.clone(),
                ally: p.team == me.team,
                me: std::ptr::eq(p, me),
            })
            .collect();
    }
    sess.meta.infer_role();
    if sess.me.is_none() {
        sess.me = Me::from_game(&data);
    }
    let (Some(me), Some(offset)) = (&sess.me, sess.meta.game_time_offset) else { return };
    let last_seen = sess.last_event_id;
    for ev in data.events.events.iter().filter(|e| last_seen.is_none_or(|last| e.id > last)) {
        if let Some(marker) = classify(ev, me) {
            if ev.name == "GameEnd" {
                sess.meta.result = ev.result.clone();
            }
            let video_time = marker.game_time + offset;
            // Happened before this recording started (joined mid-game).
            if video_time >= 0.0 {
                sess.meta.markers.push(VideoMarker { marker, video_time });
            }
        }
        sess.last_event_id = Some(ev.id);
    }
}

fn rename_with_champion(path: &Path, meta: &SessionMeta) -> PathBuf {
    let Some(champ) = &meta.champion else { return path.to_owned() };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut name = format!("{stem}_{champ}");
    if let Some(r) = &meta.result {
        name.push('_');
        name.push_str(r);
    }
    let new = path.with_file_name(format!("{name}.mp4"));
    match std::fs::rename(path, &new) {
        Ok(()) => new,
        Err(_) => path.to_owned(),
    }
}

pub fn write_sidecar(video: &Path, meta: &SessionMeta) -> anyhow::Result<()> {
    std::fs::write(video.with_extension("json"), serde_json::to_vec_pretty(meta)?)?;
    Ok(())
}

/// Recordings in the output dir, newest first.
pub fn list_recordings(dir: &Path) -> Vec<(PathBuf, Option<SessionMeta>)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp4"))
        .map(|p| {
            let meta = std::fs::read(p.with_extension("json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            (p, meta)
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Recordings that `policy` says to delete, oldest first. `keep` protects a
/// recording (notes, saved moments, pinned, or currently being written).
pub fn plan_cleanup(
    recordings: &[(PathBuf, u64, chrono::DateTime<chrono::Local>)],
    policy: &StoragePolicy,
    keep: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut oldest_first: Vec<_> = recordings.iter().collect();
    oldest_first.sort_by_key(|(_, _, started)| *started);
    let mut total: u64 = recordings.iter().map(|(_, bytes, _)| bytes).sum();
    let now = chrono::Local::now();
    let mut out = vec![];
    for (path, bytes, started) in oldest_first {
        if keep(path) {
            continue;
        }
        let too_old = policy.max_days.is_some_and(|d| (now - *started).num_days() >= i64::from(d));
        let over = policy.max_gb.is_some_and(|gb| total > u64::from(gb) * 1_000_000_000);
        if too_old || over {
            total = total.saturating_sub(*bytes);
            out.push(path.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_respects_limits_and_keep() {
        let now = chrono::Local::now();
        let day = chrono::Duration::days(1);
        let gb = 1_000_000_000;
        let recs = vec![
            (PathBuf::from("old-kept.mp4"), 2 * gb, now - day * 40),
            (PathBuf::from("old.mp4"), 2 * gb, now - day * 35),
            (PathBuf::from("mid.mp4"), 2 * gb, now - day * 5),
            (PathBuf::from("new.mp4"), 2 * gb, now),
        ];
        let keep = |p: &Path| p.ends_with("old-kept.mp4");
        // Nothing configured: nothing deleted.
        assert!(plan_cleanup(&recs, &StoragePolicy::default(), keep).is_empty());
        // Age limit only.
        let by_age = plan_cleanup(&recs, &StoragePolicy { max_days: Some(30), ..Default::default() }, keep);
        assert_eq!(by_age, vec![PathBuf::from("old.mp4")]);
        // 5 GB cap over 8 GB: drop oldest unprotected until under (old, then mid).
        let by_size = plan_cleanup(&recs, &StoragePolicy { max_gb: Some(5), ..Default::default() }, keep);
        assert_eq!(by_size, vec![PathBuf::from("old.mp4"), PathBuf::from("mid.mp4")]);
    }
}

#[cfg(test)]
mod role_tests {
    use super::*;

    fn entry(champ: &str, pos: &str, ally: bool, me: bool) -> RosterEntry {
        RosterEntry { champion: champ.into(), champion_key: champ.into(), position: pos.into(), ally, me }
    }

    #[test]
    fn infers_jungle_from_the_missing_ally_role() {
        // Real Practice Tool shape: you are NONE, bots have roles.
        let mut m = SessionMeta {
            player: Some(PlayerLine { champion: "Rammus".into(), position: "NONE".into(), ..Default::default() }),
            roster: vec![
                entry("Rammus", "NONE", true, true),
                entry("Cassiopeia", "BOTTOM", true, false),
                entry("Morgana", "UTILITY", true, false),
                entry("Anivia", "MIDDLE", true, false),
                entry("Lee Sin", "TOP", true, false),
                entry("Pantheon", "JUNGLE", false, false),
                entry("Garen", "TOP", false, false),
            ],
            ..Default::default()
        };
        m.infer_role();
        assert_eq!(m.player.as_ref().unwrap().position, "JUNGLE");
        assert_eq!(m.opponent.as_ref().unwrap().champion, "Pantheon");
        // Manual override wins.
        m.role_override = Some("TOP".into());
        m.infer_role();
        assert_eq!(m.opponent.as_ref().unwrap().champion, "Garen");
    }
}
