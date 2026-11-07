//! Updates, on your terms: Timewarp can look for a new version (once a day,
//! or when you ask) and tell you, but it never downloads anything until you
//! click Update, and it only restarts when you say so, never during a game.
//!
//! Only releases signed with the Timewarp release key are believed:
//!
//! 1. `latest.json` and `latest.json.sig` come from the latest GitHub release.
//!    The signature is checked against the public key built into this app
//!    (`update-key.pub`) before anything in the manifest is used.
//! 2. The download must have exactly the size and SHA-256 the manifest lists.
//! 3. The unpacked Timewarp.app must be intact (`codesign --verify`), have
//!    our bundle ID and the promised version, and, when this copy is signed
//!    by a team, the same team.
//! 4. It is staged next to the app. On restart a small helper waits for this
//!    copy to quit, swaps the two, starts the new one and, if it doesn't come
//!    up within 30 seconds, puts the old one back.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sha2::{Digest, Sha256};
use timewarp_release::Manifest;

use crate::about::{REPO, VERSION};
use crate::ui::{self, icon};
use crate::{TimewarpApp, theme};

/// The release key's public half. Empty means this build can't update.
const PUBLIC_KEY: &str = include_str!("../update-key.pub");
/// How often the optional background check runs.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Don't slow down launch: the first automatic check waits a little.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);
/// Nothing we publish is this big; stops a runaway download.
const MAX_DOWNLOAD: u64 = 1 << 30;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// None when the release has no download for this Mac.
    pub asset: Option<timewarp_release::Asset>,
    /// Why this copy can't replace itself (then we offer the release page).
    pub cant_install: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// The repository has no releases yet.
    NoReleases,
    CheckFailed(String),
    Available(Release),
    Downloading(Release, f32),
    /// Verified and staged; it goes in when Timewarp restarts.
    Ready(Release),
    InstallFailed(Release, String),
}

/// Shared with the worker threads; `generation` bumps on every change so
/// the UI knows when to redraw.
#[derive(Default)]
pub struct Updater {
    pub state: State,
    pub generation: u64,
    pub checked_at: Option<chrono::DateTime<chrono::Local>>,
    /// The last check was the automatic one (it toasts; manual ones don't).
    pub automatic: bool,
    /// The verified new Timewarp.app, waiting next to the running one.
    staged: Option<PathBuf>,
    /// Restart was asked for during a game: do it once the game is saved.
    pub restart_after_game: bool,
}

pub type Shared = Arc<Mutex<Updater>>;

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Updater> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn set(shared: &Shared, state: State) {
    let mut u = lock(shared);
    u.state = state;
    u.generation += 1;
}

fn client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("Timewarp/{VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .build()
}

/// "owner/repo" from the repository URL.
fn slug() -> Option<String> {
    let s = REPO.trim_end_matches('/').strip_prefix("https://github.com/")?;
    (s.split('/').count() == 2).then(|| s.to_string())
}

fn parse_version(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Is `latest` newer than the version running now?
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

pub fn updates_configured() -> bool {
    !PUBLIC_KEY.trim().is_empty()
}

/// Fetches and verifies the signed manifest of the latest release.
fn latest(client: &reqwest::blocking::Client) -> Result<Option<Manifest>, String> {
    let slug = slug().ok_or("No GitHub repository is set for updates.")?;
    let base = format!("https://github.com/{slug}/releases/latest/download");
    let get = |name: &str| -> Result<Option<Vec<u8>>, String> {
        let resp = client
            .get(format!("{base}/{name}"))
            .timeout(Duration::from_secs(30))
            .send()
            .map_err(|_| "Couldn't reach GitHub. Check your connection.".to_string())?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("GitHub answered {}.", resp.status()));
        }
        resp.bytes().map(|b| Some(b.to_vec())).map_err(|_| "The download was interrupted.".to_string())
    };
    let Some(json) = get(timewarp_release::MANIFEST)? else { return Ok(None) };
    let sig = get(timewarp_release::SIGNATURE)?.ok_or("The latest release isn't signed, so Timewarp won't use it.")?;
    let sig = String::from_utf8_lossy(&sig);
    timewarp_release::verify(&json, &sig, PUBLIC_KEY).map(Some).map_err(|e| {
        log::warn!("update rejected: {e}");
        format!("The latest release failed its signature check ({e}), so Timewarp won't use it.")
    })
}

fn check(shared: &Shared) {
    let result = (|| -> Result<State, String> {
        if !updates_configured() {
            return Err("This build has no update key, so it can't update itself.".into());
        }
        let client = client().map_err(|e| e.to_string())?;
        let Some(m) = latest(&client)? else { return Ok(State::NoReleases) };
        if !is_newer(&m.version, VERSION) {
            return Ok(State::UpToDate);
        }
        let asset = m.asset_for(std::env::consts::OS, std::env::consts::ARCH).cloned();
        let cant_install = match &asset {
            None => Some("This release has no download for your Mac.".to_string()),
            Some(_) => install_target().err(),
        };
        Ok(State::Available(Release { version: m.version, notes: m.notes, page: m.page, asset, cant_install }))
    })();
    lock(shared).checked_at = Some(chrono::Local::now());
    set(shared, result.unwrap_or_else(State::CheckFailed));
}

/// Running from a cargo build folder: updating would overwrite a dev build.
pub fn is_dev_build() -> bool {
    std::env::current_exe().is_ok_and(|p| {
        let s = p.to_string_lossy();
        s.contains("/target/debug/") || s.contains("/target/release/")
    })
}

/// The .app the running binary lives in, if any.
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn writable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// The Timewarp.app this copy would replace, or why it can't.
fn install_target() -> Result<PathBuf, String> {
    if is_dev_build() {
        return Err("This is a development build. Pull the latest code and rebuild instead.".into());
    }
    let bundle = running_bundle().ok_or("Timewarp isn't running from Timewarp.app, so it can't replace itself.")?;
    let parent = bundle.parent().ok_or("Timewarp.app has no folder.")?;
    if !writable(parent) || !writable(&bundle) {
        return Err(format!(
            "Timewarp can't write to {}. Move Timewarp.app to Applications, or download the update yourself.",
            parent.display()
        ));
    }
    Ok(bundle)
}

/// Sibling paths of the app: the staged new copy and the previous one.
/// No `.app` extension, so Launch Services never lists them as apps.
fn staged_path(bundle: &Path) -> PathBuf {
    bundle.with_file_name(".Timewarp.update")
}

fn previous_path(bundle: &Path) -> PathBuf {
    bundle.with_file_name(".Timewarp.previous")
}

/// Where the helper and the new copy talk: `launched` holds the version
/// that last started, `failed` the version that was rolled back.
fn update_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Timewarp").join("update")
}

fn plist_value(app: &Path, key: &str) -> Option<String> {
    let out = std::process::Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The signing team of an app, if it has one (ad hoc signatures don't).
fn team_of(app: &Path) -> Option<String> {
    let out = std::process::Command::new("codesign").arg("-dv").arg(app).output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

/// Is `new` really a Timewarp `version` from the same publisher as `current`?
fn check_app(new: &Path, current: &Path, version: &str) -> Result<(), String> {
    let intact = std::process::Command::new("codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(new)
        .status()
        .is_ok_and(|s| s.success());
    if !intact {
        return Err("The update's app signature is broken.".into());
    }
    let id = plist_value(current, "CFBundleIdentifier");
    if id.is_none() || plist_value(new, "CFBundleIdentifier") != id {
        return Err("The update isn't Timewarp.".into());
    }
    if plist_value(new, "CFBundleShortVersionString").as_deref() != Some(version) {
        return Err("The update's version doesn't match its release.".into());
    }
    if let Some(team) = team_of(current) {
        if team_of(new).as_deref() != Some(team.as_str()) {
            return Err("The update is signed by someone else.".into());
        }
    }
    Ok(())
}

/// Downloads, verifies and stages the release; returns the staged app.
fn download_and_stage(shared: &Shared, release: &Release) -> Result<PathBuf, String> {
    let bundle = install_target()?;
    let asset = release.asset.as_ref().ok_or("This release has no download for your Mac.")?;
    if asset.size > MAX_DOWNLOAD {
        return Err("The update is unexpectedly large.".into());
    }
    let work = std::env::temp_dir().join(format!("timewarp-update-{}-{}", release.version, std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        // 1. Download exactly the bytes the signed manifest describes.
        let archive = work.join("update.tar.gz");
        let mut resp = client()
            .map_err(|e| e.to_string())?
            .get(&asset.url)
            .timeout(Duration::from_secs(30 * 60))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| "The download didn't start. Check your connection.".to_string())?;
        let mut file = std::fs::File::create(&archive).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        let mut last = Instant::now();
        loop {
            let n = resp.read(&mut buf).map_err(|_| "The download was interrupted.".to_string())?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > asset.size {
                return Err("The download is bigger than the release says. Nothing was changed.".to_string());
            }
            std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
            hasher.update(&buf[..n]);
            if last.elapsed() > Duration::from_millis(150) {
                last = Instant::now();
                set(shared, State::Downloading(release.clone(), done as f32 / asset.size.max(1) as f32));
            }
        }
        drop(file);
        let actual = timewarp_release::to_hex(&hasher.finalize());
        if done != asset.size || actual != asset.sha256.to_lowercase() {
            return Err("The download is damaged or was changed. Nothing was changed.".to_string());
        }

        // 2. Unpack (bsdtar refuses absolute paths and `..`).
        let out = work.join("unpacked");
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let ok = std::process::Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&out).status();
        if !ok.is_ok_and(|s| s.success()) {
            return Err("Couldn't unpack the update.".to_string());
        }
        let new = out.join("Timewarp.app");
        if !new.join("Contents/MacOS/Timewarp").is_file() {
            return Err("The update has no Timewarp.app inside.".to_string());
        }

        // 3. Copy next to the app (same volume, so the swap is one rename),
        //    then check the copy that will actually run.
        let staged = staged_path(&bundle);
        let _ = std::fs::remove_dir_all(&staged);
        let copied = std::process::Command::new("ditto").arg(&new).arg(&staged).status().is_ok_and(|s| s.success());
        if !copied {
            let _ = std::fs::remove_dir_all(&staged);
            return Err("Couldn't copy the update next to Timewarp.app.".to_string());
        }
        if let Err(e) = check_app(&staged, &bundle, &release.version) {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }
        Ok(staged)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Waits for us to quit, swaps the apps, starts the new one and rolls back
/// if it doesn't report in. Its log is update/install.log.
const HELPER: &str = r#"
pid=$1 target=$2 staged=$3 previous=$4 dir=$5 version=$6
exec >>"$dir/install.log" 2>&1
echo "$(date '+%F %T') installing $version"
i=0
while kill -0 "$pid" 2>/dev/null; do
    sleep 0.2; i=$((i+1))
    [ $i -gt 900 ] && { echo "the running copy didn't quit"; exit 1; }
done
rm -rf "$previous"
if ! mv "$target" "$previous"; then echo "couldn't move the old app aside"; open "$target"; exit 1; fi
if ! mv "$staged" "$target"; then echo "couldn't move the new app in"; mv "$previous" "$target"; open "$target"; exit 1; fi
rm -f "$dir/launched"
open "$target"
i=0
while [ $i -lt 120 ]; do
    if [ "$(cat "$dir/launched" 2>/dev/null)" = "$version" ]; then
        rm -rf "$previous"; echo "done"; exit 0
    fi
    sleep 0.25; i=$((i+1))
done
echo "$version didn't start; putting the previous version back"
pkill -f "$target/Contents/MacOS/Timewarp"
sleep 1
rm -rf "$target"
mv "$previous" "$target"
echo "$version" > "$dir/failed"
open "$target"
"#;

/// Call once the window is up: tells a waiting helper the update worked,
/// and returns the version that was rolled back, if the last one failed.
pub fn launched() -> Option<String> {
    let dir = update_dir();
    if !dir.exists() {
        return None;
    }
    let _ = std::fs::write(dir.join("launched"), VERSION);
    if let Some(bundle) = running_bundle() {
        // Left over from an update that was downloaded but never restarted.
        let staged = staged_path(&bundle);
        if staged.exists() {
            let _ = std::fs::remove_dir_all(staged);
        }
    }
    let failed = std::fs::read_to_string(dir.join("failed")).ok()?;
    let _ = std::fs::remove_file(dir.join("failed"));
    Some(failed.trim().to_string())
}

impl TimewarpApp {
    pub fn update_state(&self) -> State {
        lock(&self.updater).state.clone()
    }

    /// Something to act on: shows a dot on About in the sidebar.
    pub fn update_badge(&self) -> bool {
        matches!(self.update_state(), State::Available(_) | State::Ready(_))
    }

    pub fn check_for_updates(&mut self, automatic: bool, cx: &mut Context<Self>) {
        if matches!(self.update_state(), State::Checking | State::Downloading(..) | State::Ready(_)) {
            return;
        }
        self.update_checked = Some(Instant::now());
        lock(&self.updater).automatic = automatic;
        set(&self.updater, State::Checking);
        let shared = self.updater.clone();
        std::thread::spawn(move || check(&shared));
        cx.notify();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let (State::Available(release) | State::InstallFailed(release, _)) = self.update_state() else { return };
        if release.cant_install.is_some() {
            return;
        }
        set(&self.updater, State::Downloading(release.clone(), 0.0));
        let shared = self.updater.clone();
        std::thread::spawn(move || {
            let state = match download_and_stage(&shared, &release) {
                Ok(staged) => {
                    log::info!("update {} verified and staged", release.version);
                    lock(&shared).staged = Some(staged);
                    State::Ready(release)
                }
                Err(e) => {
                    log::warn!("update {} failed: {e}", release.version);
                    State::InstallFailed(release, e)
                }
            };
            set(&shared, state);
        });
        cx.notify();
    }

    fn skip_version(&mut self, version: String, cx: &mut Context<Self>) {
        self.update_config(move |c| c.skip_version = Some(version), cx);
        set(&self.updater, State::UpToDate);
        cx.notify();
    }

    fn busy_for_restart(&self) -> bool {
        self.is_recording() || matches!(self.status.phase, timewarp_engine::Phase::Armed)
    }

    /// Hand over to the helper and quit the safe way (finalizing anything).
    pub fn restart_app(&mut self, cx: &mut Context<Self>) {
        if self.busy_for_restart() {
            lock(&self.updater).restart_after_game = true;
            self.toast(IconName::RotateCw, "Timewarp will restart once this game is saved.", None, cx);
            cx.notify();
            return;
        }
        let (Some(staged), Some(bundle), State::Ready(release)) =
            (lock(&self.updater).staged.clone(), running_bundle(), self.update_state())
        else {
            return;
        };
        let dir = update_dir();
        let spawned = std::fs::create_dir_all(&dir).is_ok() && {
            use std::os::unix::process::CommandExt;
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(HELPER)
                .arg("timewarp-update")
                .arg(std::process::id().to_string())
                .arg(&bundle)
                .arg(&staged)
                .arg(previous_path(&bundle))
                .arg(&dir)
                .arg(&release.version)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                // Its own process group, so it outlives us.
                .process_group(0)
                .spawn()
                .is_ok()
        };
        if !spawned {
            set(&self.updater, State::InstallFailed(release, "Couldn't start the installer.".into()));
            cx.notify();
            return;
        }
        log::info!("restarting into {}", release.version);
        cx.quit();
    }

    /// Called from the app's periodic refresh.
    pub fn poll_updates(&mut self, cx: &mut Context<Self>) {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            if let Some(v) = launched() {
                self.toast(
                    IconName::TriangleAlert,
                    format!("Timewarp {v} didn't start, so your previous version was kept."),
                    None,
                    cx,
                );
            }
        });
        if lock(&self.updater).restart_after_game && !self.busy_for_restart() {
            lock(&self.updater).restart_after_game = false;
            self.restart_app(cx);
            return;
        }
        let due = match self.update_checked {
            None => self.started_at.elapsed() >= FIRST_CHECK_AFTER,
            Some(t) => t.elapsed() >= CHECK_EVERY,
        };
        if self.config.check_updates && due && updates_configured() {
            self.check_for_updates(true, cx);
        }
        let (generation, state, automatic) = {
            let u = lock(&self.updater);
            (u.generation, u.state.clone(), u.automatic)
        };
        if generation == self.update_seen {
            return;
        }
        self.update_seen = generation;
        // A quiet heads-up from the daily check (not for skipped versions).
        if let State::Available(r) = &state {
            let skipped = self.config.skip_version.as_deref() == Some(r.version.as_str());
            if automatic && skipped {
                set(&self.updater, State::UpToDate);
            } else if automatic {
                self.toast(
                    IconName::Download,
                    format!("Timewarp {} is available.", r.version),
                    Some(ui::ToastAction::Go(crate::Page::About, "View")),
                    cx,
                );
            }
        }
        if let State::Ready(r) = &state {
            self.toast(
                IconName::Check,
                format!("Timewarp {} is ready. Restart when you're ready.", r.version),
                Some(ui::ToastAction::Go(crate::Page::About, "Restart…")),
                cx,
            );
        }
        cx.notify();
    }

    /// The Updates card on About.
    pub fn updates_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.update_state();
        let checked = self.updater.lock().unwrap_or_else(|e| e.into_inner()).checked_at;
        let ago = checked.map(|t| {
            let mins = (chrono::Local::now() - t).num_minutes();
            match mins {
                0 => "just now".to_string(),
                1..=59 => format!("{mins} min ago"),
                _ => t.format("at %H:%M").to_string(),
            }
        });
        let check_button = |label: &'static str| {
            ui::ghost_button("update-check").child(icon(IconName::RefreshCw, 13., theme::muted())).child(label).on_click(
                cx.listener(|this, _, _, cx| this.check_for_updates(false, cx)),
            )
        };
        let (glyph, color, title, detail): (IconName, Hsla, String, Option<String>) = match &state {
            State::Idle => (IconName::RefreshCw, theme::muted(), "Updates".into(), Some("Timewarp never updates by itself, and only goes online to check when you allow it.".into())),
            State::Checking => (IconName::RefreshCw, theme::muted(), "Checking for updates…".into(), None),
            State::UpToDate => (
                IconName::Check,
                theme::assist(),
                "You're up to date".into(),
                ago.as_ref().map(|a| format!("Checked {a}.")),
            ),
            State::NoReleases => (IconName::Check, theme::muted(), "No releases published yet".into(), Some(format!("Updates will come from {REPO}/releases."))),
            State::CheckFailed(e) => (IconName::TriangleAlert, theme::kill(), "Couldn't check for updates".into(), Some(e.clone())),
            State::Available(r) => (IconName::Download, theme::accent(), format!("Timewarp {} is available", r.version), Some(format!("You have {VERSION}."))),
            State::Downloading(r, f) => (IconName::Download, theme::accent(), format!("Downloading {}… {:.0}%", r.version, f * 100.0), None),
            State::Ready(r) => (
                IconName::Check,
                theme::assist(),
                format!("Timewarp {} is ready", r.version),
                Some(if lock(&self.updater).restart_after_game {
                    "Timewarp will restart once this game is saved.".into()
                } else {
                    "Downloaded and verified. It goes in when Timewarp restarts.".into()
                }),
            ),
            State::InstallFailed(_, e) => (IconName::TriangleAlert, theme::death(), "The update didn't install".into(), Some(e.clone())),
        };
        let actions: AnyElement = match &state {
            State::Available(r) | State::InstallFailed(r, _) => {
                let (page, version) = (r.page.clone(), r.version.clone());
                let can_install = r.cant_install.is_none();
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(can_install, |d| {
                        d.child(
                            ui::solid_button("update-install", theme::accent())
                                .text_color(theme::bg())
                                .child(icon(IconName::Download, 13., theme::bg()))
                                .child(if matches!(state, State::InstallFailed(..)) { "Try again" } else { "Update" })
                                .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                        )
                    })
                    .child(
                        ui::ghost_button("update-notes")
                            .child(icon(IconName::ExternalLink, 13., theme::muted()))
                            .child(if can_install { "Release page" } else { "Download from GitHub" })
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                    .child(ui::ghost_button("update-skip").child("Skip this version").on_click(cx.listener(move |this, _, _, cx| {
                        this.skip_version(version.clone(), cx)
                    })))
                    .into_any_element()
            }
            State::Downloading(_, f) => div()
                .w_full()
                .h(px(6.))
                .rounded_full()
                .bg(theme::raised())
                .child(div().h_full().rounded_full().bg(theme::accent()).w(relative(*f)))
                .into_any_element(),
            State::Ready(_) => ui::solid_button("update-restart", theme::accent())
                .text_color(theme::bg())
                .child(icon(IconName::RotateCw, 13., theme::bg()))
                .child("Restart Timewarp")
                .on_click(cx.listener(|this, _, _, cx| this.restart_app(cx)))
                .into_any_element(),
            State::Checking => div().into_any_element(),
            _ => check_button("Check for updates").into_any_element(),
        };
        let notes = match &state {
            State::Available(r) | State::InstallFailed(r, _) if !r.notes.trim().is_empty() => Some(
                div()
                    .id("update-notes-body")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .p_3()
                    .rounded_md()
                    .bg(theme::bg())
                    .text_sm()
                    .child(ui::markdown("release-notes", r.notes.clone())),
            ),
            _ => None,
        };
        div()
            .w_full()
            .max_w(px(520.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::line())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(div().pt(px(2.)).child(icon(glyph, 16., color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().font_weight(FontWeight::MEDIUM).child(title))
                            .children(detail.map(|d| div().text_sm().text_color(theme::muted()).child(d))),
                    ),
            )
            .children(notes)
            .child(actions)
            .children(match &state {
                State::Available(r) => r.cant_install.clone().map(|why| div().text_xs().text_color(theme::muted()).child(why)),
                _ => None,
            })
            .child(if self.config.check_updates {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).child(
                        "Timewarp checks once a day and lets you know. Nothing downloads until you click Update, and only releases signed with the Timewarp release key are accepted.",
                    ))
                    .child(ui::ghost_button("update-auto-off").flex_none().text_xs().child("Turn off").on_click(
                        cx.listener(|this, _, _, cx| this.update_config(|c| c.check_updates = false, cx)),
                    ))
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_xs().text_color(theme::muted()).child(
                        "Automatic checks are off, so Timewarp never contacts GitHub on its own. It only checks when you click Check for updates.",
                    ))
                    .child(ui::ghost_button("update-auto-on").flex_none().text_xs().child("Check daily").on_click(
                        cx.listener(|this, _, _, cx| this.update_config(|c| c.check_updates = true, cx)),
                    ))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn compares_versions_like_people_do() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[::core::prelude::v1::test]
    fn sibling_paths_are_not_apps() {
        let app = Path::new("/Applications/Timewarp.app");
        assert_eq!(staged_path(app), Path::new("/Applications/.Timewarp.update"));
        assert_eq!(previous_path(app), Path::new("/Applications/.Timewarp.previous"));
    }
}
