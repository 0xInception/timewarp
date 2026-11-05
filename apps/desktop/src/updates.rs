//! Updates, on your terms: Timewarp can look for a new version (once a day,
//! or when you ask) and tell you, but it never downloads or installs
//! anything until you click Update, and it only restarts when you say so,
//! never during a game.
//!
//! Versions come from the GitHub releases of `about::REPO`. A release
//! carries `timewarp-<version>-macos-<arch>.tar.gz` (the app binary, or
//! Timewarp.app) and, optionally, the same name plus `.sha256`, which is
//! checked before anything is replaced.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::about::{REPO, VERSION};
use crate::ui::{self, icon};
use crate::{TimewarpApp, theme};

/// How often the optional background check runs.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Don't slow down launch: the first automatic check waits a little.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// None when the release has no download for this Mac.
    pub asset: Option<Asset>,
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
    /// Installed on disk; the running copy is still the old one.
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
}

pub type Shared = Arc<Mutex<Updater>>;

fn set(shared: &Shared, state: State) {
    let mut u = shared.lock().unwrap_or_else(|e| e.into_inner());
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

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        a => a,
    }
}

/// The download for this Mac: our arch, else a universal build.
fn pick_asset(assets: &[GhAsset]) -> Option<Asset> {
    let os = if cfg!(target_os = "macos") { "macos" } else { std::env::consts::OS };
    let wanted = |a: &&GhAsset| {
        let n = a.name.to_lowercase();
        n.contains(os) && (n.ends_with(".tar.gz") || n.ends_with(".zip"))
    };
    let archive = assets
        .iter()
        .filter(wanted)
        .find(|a| a.name.contains(arch()) || (arch() == "aarch64" && a.name.contains("arm64")))
        .or_else(|| assets.iter().filter(wanted).find(|a| a.name.contains("universal")))?;
    let sum = format!("{}.sha256", archive.name);
    Some(Asset {
        name: archive.name.clone(),
        url: archive.browser_download_url.clone(),
        size: archive.size,
        sha256_url: assets.iter().find(|a| a.name == sum).map(|a| a.browser_download_url.clone()),
    })
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
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

fn check(shared: &Shared) {
    let result = (|| -> Result<State, String> {
        let slug = slug().ok_or("No GitHub repository is set for updates.")?;
        let resp = client()
            .map_err(|e| e.to_string())?
            .get(format!("https://api.github.com/repos/{slug}/releases/latest"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|_| "Couldn't reach GitHub. Check your connection.".to_string())?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(State::NoReleases);
        }
        if !resp.status().is_success() {
            return Err(format!("GitHub answered {}.", resp.status()));
        }
        let r: GhRelease = resp.json().map_err(|e| e.to_string())?;
        if r.draft || r.prerelease || !is_newer(&r.tag_name, VERSION) {
            return Ok(State::UpToDate);
        }
        Ok(State::Available(Release {
            version: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
            notes: r.body.unwrap_or_default(),
            page: r.html_url,
            asset: pick_asset(&r.assets),
        }))
    })();
    {
        let mut u = shared.lock().unwrap_or_else(|e| e.into_inner());
        u.checked_at = Some(chrono::Local::now());
    }
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
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn install(shared: &Shared, release: &Release) -> Result<(), String> {
    let asset = release.asset.as_ref().ok_or("This release has no download for your Mac.")?;
    let work = std::env::temp_dir().join(format!("timewarp-update-{}", release.version));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let archive = work.join(&asset.name);

    // 1. Download, reporting progress.
    let client = client().map_err(|e| e.to_string())?;
    let mut resp = client
        .get(&asset.url)
        .timeout(Duration::from_secs(30 * 60))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| "The download didn't start. Check your connection.".to_string())?;
    let total = resp.content_length().unwrap_or(asset.size).max(1);
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
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if last.elapsed() > Duration::from_millis(150) {
            last = Instant::now();
            set(shared, State::Downloading(release.clone(), (done as f32 / total as f32).min(1.0)));
        }
    }
    drop(file);

    // 2. Check it's exactly the file that was published.
    if let Some(url) = &asset.sha256_url {
        let expected = client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|_| "Couldn't download the checksum.".to_string())?;
        let expected = expected.split_whitespace().next().unwrap_or_default().to_lowercase();
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if expected != actual {
            return Err("The download is damaged (checksum mismatch). Nothing was changed.".into());
        }
    }

    // 3. Unpack with the system's own tools.
    let out = work.join("unpacked");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let ok = if asset.name.ends_with(".zip") {
        std::process::Command::new("ditto").arg("-x").arg("-k").arg(&archive).arg(&out).status()
    } else {
        std::process::Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&out).status()
    };
    if !ok.is_ok_and(|s| s.success()) {
        return Err("Couldn't unpack the update.".into());
    }

    // 4. Swap it in. The running copy keeps working until the restart.
    match running_bundle() {
        Some(bundle) => {
            let new = find(&out, &|p| p.extension().is_some_and(|e| e == "app")).ok_or("The update has no Timewarp.app inside.")?;
            let old = bundle.with_extension("app.old");
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(&bundle, &old).map_err(|e| format!("Couldn't replace the app: {e}"))?;
            if let Err(e) = move_dir(&new, &bundle) {
                let _ = std::fs::rename(&old, &bundle);
                return Err(format!("Couldn't replace the app: {e}"));
            }
            let _ = std::fs::remove_dir_all(&old);
        }
        None => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let name = exe.file_name().unwrap_or_default().to_os_string();
            let new = find(&out, &|p| p.is_file() && p.file_name() == Some(name.as_os_str()))
                .ok_or("The update has no Timewarp program inside.")?;
            // Copy next to the old one, then rename over it: never half-written.
            let staged = exe.with_extension("update");
            std::fs::copy(&new, &staged).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
            }
            std::fs::rename(&staged, &exe).map_err(|e| format!("Couldn't replace the program: {e}"))?;
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

fn find(dir: &Path, pred: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if pred(&p) {
            return Some(p);
        }
        if p.is_dir() && p.extension().is_none_or(|e| e != "app") {
            if let Some(f) = find(&p, pred) {
                return Some(f);
            }
        }
    }
    None
}

fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Different volume: copy with the system tool (keeps signatures intact).
    let ok = std::process::Command::new("ditto").arg(from).arg(to).status()?.success();
    if ok { Ok(()) } else { Err(std::io::Error::other("ditto failed")) }
}

impl TimewarpApp {
    pub fn update_state(&self) -> State {
        self.updater.lock().unwrap_or_else(|e| e.into_inner()).state.clone()
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
        {
            let mut u = self.updater.lock().unwrap_or_else(|e| e.into_inner());
            u.automatic = automatic;
        }
        set(&self.updater, State::Checking);
        let shared = self.updater.clone();
        std::thread::spawn(move || check(&shared));
        cx.notify();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let (State::Available(release) | State::InstallFailed(release, _)) = self.update_state() else { return };
        set(&self.updater, State::Downloading(release.clone(), 0.0));
        let shared = self.updater.clone();
        std::thread::spawn(move || {
            let state = match install(&shared, &release) {
                Ok(()) => State::Ready(release),
                Err(e) => State::InstallFailed(release, e),
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

    /// Start the new copy, then quit this one the safe way.
    pub fn restart_app(&mut self, cx: &mut Context<Self>) {
        if self.is_recording() {
            self.toast(IconName::Circle, "Finish your game first. Timewarp restarts after it's saved.", None, cx);
            return;
        }
        match running_bundle() {
            Some(bundle) => {
                // `open -n` waits for nothing; the new copy starts as we quit.
                let _ = std::process::Command::new("open").arg("-n").arg(bundle).spawn();
            }
            None => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).args(std::env::args().skip(1)).spawn();
                }
            }
        }
        cx.quit();
    }

    /// Called from the app's periodic refresh.
    pub fn poll_updates(&mut self, cx: &mut Context<Self>) {
        let due = match self.update_checked {
            None => self.started_at.elapsed() >= FIRST_CHECK_AFTER,
            Some(t) => t.elapsed() >= CHECK_EVERY,
        };
        if self.config.check_updates && due && !is_dev_build() {
            self.check_for_updates(true, cx);
        }
        let (generation, state, automatic) = {
            let u = self.updater.lock().unwrap_or_else(|e| e.into_inner());
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
                format!("Timewarp {} is installed. Restart when you're ready.", r.version),
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
            State::Idle => (IconName::RefreshCw, theme::muted(), "Updates".into(), Some("Timewarp never updates by itself.".into())),
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
            State::Ready(r) => (IconName::Check, theme::assist(), format!("Timewarp {} is installed", r.version), Some("Restart to start using it.".into())),
            State::InstallFailed(_, e) => (IconName::TriangleAlert, theme::death(), "The update didn't install".into(), Some(e.clone())),
        };
        let dev = is_dev_build();
        let actions: AnyElement = match &state {
            State::Available(r) | State::InstallFailed(r, _) => {
                let (page, version) = (r.page.clone(), r.version.clone());
                let can_install = r.asset.is_some() && !dev;
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
            .when(dev && matches!(state, State::Available(_)), |d| {
                d.child(div().text_xs().text_color(theme::muted()).child(
                    "This is a development build, so it can't update itself. Pull the latest code and rebuild instead.",
                ))
            })
            .child(div().text_xs().text_color(theme::muted()).child(if self.config.check_updates {
                "Timewarp checks once a day and lets you know. Nothing downloads or installs until you click Update. Change this in Settings."
            } else {
                "Automatic checks are off. Timewarp only looks for updates when you click Check."
            }))
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
    fn picks_this_macs_download() {
        let a = |n: &str| GhAsset { name: n.into(), browser_download_url: format!("https://x/{n}"), size: 1 };
        let assets = [
            a("timewarp-0.2.0-macos-x86_64.tar.gz"),
            a("timewarp-0.2.0-macos-aarch64.tar.gz"),
            a("timewarp-0.2.0-macos-aarch64.tar.gz.sha256"),
            a("timewarp-0.2.0-windows-x86_64.zip"),
        ];
        let picked = pick_asset(&assets).unwrap();
        assert!(picked.name.contains(arch()));
        assert!(picked.name.contains("macos"));
        if arch() == "aarch64" {
            assert!(picked.sha256_url.is_some());
        }
    }
}
