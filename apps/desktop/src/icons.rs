//! Champion portraits from Riot's Data Dragon CDN, cached on disk forever
//! (keyed by champion, so patches don't invalidate them).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

pub struct Icons {
    dir: PathBuf,
    ready: Mutex<HashSet<String>>,
    requested: Mutex<HashSet<String>>,
    tx: Sender<String>,
}

impl Icons {
    pub fn new() -> Arc<Self> {
        let dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("Timewarp").join("champion");
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = mpsc::channel::<String>();
        let icons = Arc::new(Self { dir: dir.clone(), ready: Default::default(), requested: Default::default(), tx });
        let weak = Arc::downgrade(&icons);
        std::thread::Builder::new()
            .name("icon-fetch".into())
            .spawn(move || {
                let http = reqwest::blocking::Client::new();
                let mut version: Option<String> = None;
                for key in rx {
                    if version.is_none() {
                        version = http
                            .get("https://ddragon.leagueoflegends.com/api/versions.json")
                            .send()
                            .and_then(|r| r.json::<Vec<String>>())
                            .ok()
                            .and_then(|v| v.into_iter().next());
                    }
                    let Some(ver) = &version else { continue };
                    let url = format!("https://ddragon.leagueoflegends.com/cdn/{ver}/img/champion/{key}.png");
                    let Ok(bytes) = http.get(url).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes())
                    else {
                        continue;
                    };
                    let path = dir.join(format!("{key}.png"));
                    let tmp = path.with_extension("tmp");
                    if std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &path)).is_ok() {
                        let Some(icons) = weak.upgrade() else { return };
                        icons.ready.lock().unwrap().insert(key);
                    }
                }
            })
            .expect("spawn icon thread");
        icons
    }

    /// Path to the portrait if cached; otherwise queues a download and returns None.
    pub fn champion(&self, key: &str) -> Option<PathBuf> {
        if key.is_empty() {
            return None;
        }
        let path = self.dir.join(format!("{key}.png"));
        if self.ready.lock().unwrap().contains(key) {
            return Some(path);
        }
        if path.exists() {
            self.ready.lock().unwrap().insert(key.to_owned());
            return Some(path);
        }
        if self.requested.lock().unwrap().insert(key.to_owned()) {
            let _ = self.tx.send(key.to_owned());
        }
        None
    }
}
