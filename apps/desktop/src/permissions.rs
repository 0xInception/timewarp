//! Setup checks: everything Timewarp needs from the system, re-checked every
//! few seconds, each with plain steps and one-click fixes.
//!
//! The list is platform-neutral; macOS adds its privacy permissions, and the
//! Windows port will add its own items to the same list.

use std::path::Path;

use timewarp_engine::Config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ok,
    /// Required and missing: Timewarp can't record.
    Missing,
    /// Not decided yet (the system can show its prompt).
    NotAsked,
    /// Works, but worth attention (low disk, League not found…).
    Warning,
    /// Not relevant with the current settings.
    NotNeeded,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fix {
    /// Show the system's permission prompt.
    RequestScreenRecording,
    RequestMicrophone,
    /// Open System Settings at a privacy pane.
    OpenSettings(&'static str),
    /// Permissions like Screen Recording only apply after a restart.
    Restart,
    ChooseFolder,
    GoToStorageSettings,
}

#[derive(Clone)]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub state: State,
    pub required: bool,
    pub why: String,
    pub steps: Vec<String>,
    pub fixes: Vec<(Fix, &'static str)>,
}

impl Check {
    pub fn blocking(&self) -> bool {
        self.required && matches!(self.state, State::Missing | State::NotAsked)
    }
    pub fn needs_attention(&self) -> bool {
        matches!(self.state, State::Missing | State::NotAsked | State::Warning)
    }
}

pub fn checks(config: &Config) -> Vec<Check> {
    let mut out = vec![];
    #[cfg(target_os = "macos")]
    out.extend(mac::checks(config));
    out.push(save_folder(&config.output_dir));
    out.push(free_space(&config.output_dir));
    out.push(league_installed());
    out
}

fn save_folder(dir: &Path) -> Check {
    let probe = dir.join(".timewarp-write-test");
    let ok = std::fs::create_dir_all(dir).is_ok() && std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    Check {
        id: "folder",
        title: "Save folder",
        state: if ok { State::Ok } else { State::Missing },
        required: true,
        why: if ok {
            format!("Recordings are saved to {}.", dir.display())
        } else {
            format!("Timewarp can't write to {}. It may be on a drive that isn't connected, or macOS is blocking access.", dir.display())
        },
        steps: if ok {
            vec![]
        } else {
            vec![
                "If the folder is on an external drive, connect it.".into(),
                "Or choose a different folder, for example inside Movies.".into(),
            ]
        },
        fixes: if ok { vec![] } else { vec![(Fix::ChooseFolder, "Choose folder…")] },
    }
}

/// Free bytes on the volume holding `dir`.
fn available_bytes(dir: &Path) -> Option<u64> {
    let probe = std::iter::successors(Some(dir), |p| p.parent()).find(|p| p.exists())?;
    let c = std::ffi::CString::new(probe.to_string_lossy().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

fn free_space(dir: &Path) -> Check {
    let free = available_bytes(dir);
    let gb = free.map(|b| b as f64 / 1e9).unwrap_or(f64::MAX);
    let low = gb < 10.0;
    Check {
        id: "space",
        title: "Free space",
        state: if low { State::Warning } else { State::Ok },
        required: false,
        why: match free {
            Some(_) if low => format!(
                "Only {gb:.1} GB free. A 30-minute game can take 2 to 5 GB, so recording may stop when the disk fills up."
            ),
            Some(_) => format!("{gb:.0} GB free on this drive."),
            None => "Couldn't read the free space for this drive.".into(),
        },
        steps: if low {
            vec![
                "Turn on Keep only the moments, or a storage limit, in Settings → Storage.".into(),
                "Or move recordings you don't need to the Trash and empty it.".into(),
            ]
        } else {
            vec![]
        },
        fixes: if low { vec![(Fix::GoToStorageSettings, "Storage settings")] } else { vec![] },
    }
}

fn league_installed() -> Check {
    #[cfg(target_os = "macos")]
    let found = Path::new("/Applications/League of Legends.app").exists();
    #[cfg(windows)]
    let found = Path::new(r"C:\Riot Games\League of Legends").exists();
    #[cfg(not(any(target_os = "macos", windows)))]
    let found = true;
    Check {
        id: "league",
        title: "League of Legends",
        state: if found { State::Ok } else { State::Warning },
        required: false,
        why: if found {
            "Found. Matches are detected automatically when the game starts.".into()
        } else {
            "League wasn't found in the usual place. If it's installed elsewhere, that's fine: matches are detected from the running game.".into()
        },
        steps: vec![],
        fixes: vec![],
    }
}

#[cfg(target_os = "macos")]
pub mod mac {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

    use super::{Check, Fix, State};
    use timewarp_engine::Config;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    pub const SCREEN_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
    pub const MIC_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

    pub fn request_screen_recording() {
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }

    pub fn request_microphone() {
        unsafe {
            if let Some(audio) = AVMediaTypeAudio {
                let done = RcBlock::new(|_: Bool| {});
                AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &done);
            }
        }
    }

    /// The app macOS lists permissions under. When Timewarp runs as its own
    /// .app that's Timewarp; started from a terminal or editor, macOS
    /// attributes it to that app instead.
    pub fn permission_owner() -> String {
        static OWNER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        OWNER.get_or_init(find_owner).clone()
    }

    fn find_owner() -> String {
        let exe = std::env::current_exe().unwrap_or_default();
        if let Some(app) = app_name(&exe.to_string_lossy()) {
            return app;
        }
        let mut pid = std::process::id();
        for _ in 0..12 {
            let out = std::process::Command::new("ps").args(["-o", "ppid=,comm=", "-p", &pid.to_string()]).output();
            let Ok(out) = out else { break };
            let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let (ppid, comm) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            if let Some(app) = app_name(comm) {
                return app;
            }
            match ppid.trim().parse::<u32>() {
                Ok(p) if p > 1 => pid = p,
                _ => break,
            }
        }
        "Terminal".into()
    }

    /// "…/T3 Code.app/Contents/MacOS/x" → "T3 Code"
    fn app_name(path: &str) -> Option<String> {
        let i = path.find(".app/")?;
        let start = path[..i].rfind('/').map(|j| j + 1).unwrap_or(0);
        Some(path[start..i].to_string())
    }

    pub fn checks(config: &Config) -> Vec<Check> {
        let owner = permission_owner();
        let screen = unsafe { CGPreflightScreenCaptureAccess() };
        let mut out = vec![Check {
            id: "screen",
            title: "Screen Recording",
            state: if screen { State::Ok } else { State::Missing },
            required: true,
            why: if screen {
                "Allowed. Timewarp can record League's window and sound.".into()
            } else {
                "Needed to record League. Timewarp only records the League window and its sound, never the rest of your screen.".into()
            },
            steps: if screen {
                vec![]
            } else {
                vec![
                    "Click Allow below. If macOS shows a prompt, choose Open System Settings.".into(),
                    format!("In Privacy & Security → Screen & System Audio Recording, turn on “{owner}”."),
                    "macOS asks to quit and reopen. Choose Later, then click Restart Timewarp here.".into(),
                ]
            },
            fixes: if screen {
                vec![]
            } else {
                vec![
                    (Fix::RequestScreenRecording, "Allow"),
                    (Fix::OpenSettings(SCREEN_PANE), "Open System Settings"),
                    (Fix::Restart, "Restart Timewarp"),
                ]
            },
        }];

        let mic = unsafe { AVMediaTypeAudio.map(|a| AVCaptureDevice::authorizationStatusForMediaType(a)) };
        let (state, why, steps, fixes) = if !config.capture.microphone {
            (State::NotNeeded, "Off. Turn on Microphone in Settings to record your voice.".to_string(), vec![], vec![])
        } else {
            match mic {
                Some(s) if s == AVAuthorizationStatus::Authorized => {
                    (State::Ok, "Allowed. Your voice is recorded with the game.".into(), vec![], vec![])
                }
                Some(s) if s == AVAuthorizationStatus::NotDetermined => (
                    State::NotAsked,
                    "Microphone recording is on, but macOS hasn't been asked yet.".into(),
                    vec!["Click Allow, then OK in the macOS prompt.".into()],
                    vec![(Fix::RequestMicrophone, "Allow")],
                ),
                _ => (
                    State::Missing,
                    "Microphone recording is on, but macOS blocks it. Games still record, without your voice.".into(),
                    vec![
                        "Click Open System Settings.".into(),
                        format!("In Privacy & Security → Microphone, turn on “{owner}”."),
                        "Click Restart Timewarp here.".into(),
                    ],
                    vec![(Fix::OpenSettings(MIC_PANE), "Open System Settings"), (Fix::Restart, "Restart Timewarp")],
                ),
            }
        };
        out.push(Check { id: "mic", title: "Microphone", state, required: false, why, steps, fixes });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwritable_folder_blocks_and_offers_a_fix() {
        let c = save_folder(Path::new("/System/definitely-not-writable/timewarp"));
        assert_eq!(c.state, State::Missing);
        assert!(c.blocking());
        assert_eq!(c.fixes[0].0, Fix::ChooseFolder);
        let ok = save_folder(&std::env::temp_dir().join("timewarp-perm-test"));
        assert_eq!(ok.state, State::Ok);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_the_app_that_owns_permissions() {
        assert_eq!(mac::permission_owner().is_empty(), false);
    }
}
