//! League of Legends integration — strictly passive.
//!
//! * Game detection: look for the game process by name (no handles opened).
//! * Game data: Riot's official Live Client Data API, which the game itself
//!   serves on `https://127.0.0.1:2999` while a match is running. No memory
//!   reading, no injection, nothing Vanguard/anti-cheat could object to.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// macOS bundle id of the in-game client (not the launcher/lobby client).
pub const GAME_BUNDLE_ID: &str = "com.riotgames.LeagueofLegends.GameClient";

#[cfg(target_os = "macos")]
const GAME_PROCESS_NAME: &str = "LeagueofLegends";
#[cfg(windows)]
const GAME_PROCESS_NAME: &str = "League of Legends.exe";
#[cfg(not(any(target_os = "macos", windows)))]
const GAME_PROCESS_NAME: &str = "League of Legends";

/// Cheap process-table scan for the in-game client.
pub struct ProcessWatcher {
    sys: System,
}

impl Default for ProcessWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessWatcher {
    pub fn new() -> Self {
        Self { sys: System::new() }
    }

    pub fn game_pid(&mut self) -> Option<u32> {
        self.game().map(|(pid, _)| pid)
    }

    /// (pid, process start time): identifies one running match even when the
    /// League client API isn't available.
    pub fn game(&mut self) -> Option<(u32, u64)> {
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        self.sys
            .processes_by_exact_name(GAME_PROCESS_NAME.as_ref())
            .next()
            .map(|p| (p.pid().as_u32(), p.start_time()))
    }
}

// ---------------------------------------------------------------------------
// Live Client Data API
// ---------------------------------------------------------------------------

pub struct LiveClient {
    http: reqwest::blocking::Client,
    base: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStats {
    pub game_mode: String,
    pub game_time: f64,
    pub map_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scores {
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub creep_score: u32,
    pub ward_score: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Player {
    pub champion_name: String,
    /// `game_character_displayname_MonkeyKing`; the suffix is the Data Dragon key.
    pub raw_champion_name: String,
    pub riot_id: String,
    pub riot_id_game_name: String,
    /// In-event identity: events name players by this (bots: "Taric Bot").
    pub summoner_name: String,
    pub team: String,
    /// TOP / JUNGLE / MIDDLE / BOTTOM / UTILITY / NONE
    pub position: String,
    pub level: u32,
    pub is_bot: bool,
    pub scores: Scores,
}

impl Player {
    /// Data Dragon asset key (e.g. "MonkeyKing" for Wukong).
    pub fn champion_key(&self) -> String {
        self.raw_champion_name
            .strip_prefix("game_character_displayname_")
            .map(str::to_owned)
            .unwrap_or_else(|| self.champion_name.replace([' ', '\'', '.'], ""))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActivePlayer {
    pub riot_id: String,
    pub riot_id_game_name: String,
    pub summoner_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllGameData {
    #[serde(default)]
    pub active_player: ActivePlayer,
    #[serde(default)]
    pub all_players: Vec<Player>,
    pub game_data: GameStats,
    #[serde(default)]
    pub events: EventEnvelope,
}

impl AllGameData {
    pub fn me(&self) -> Option<&Player> {
        let a = &self.active_player;
        self.all_players.iter().find(|p| {
            (!a.riot_id.is_empty() && p.riot_id == a.riot_id)
                || (!a.summoner_name.is_empty() && p.summoner_name == a.summoner_name)
        })
    }

    /// Enemy in the same lane, when positions are assigned (not in ARAM / practice).
    pub fn lane_opponent(&self) -> Option<&Player> {
        let me = self.me()?;
        if me.position.is_empty() || me.position == "NONE" {
            return None;
        }
        self.all_players.iter().find(|p| p.team != me.team && p.position == me.position)
    }
}

/// Raw event from `/liveclientdata/eventdata`. Field set varies per event.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawEvent {
    #[serde(rename = "EventID")]
    pub id: u64,
    #[serde(rename = "EventName")]
    pub name: String,
    #[serde(rename = "EventTime")]
    pub time: f64,
    #[serde(rename = "KillerName", default)]
    pub killer: Option<String>,
    #[serde(rename = "VictimName", default)]
    pub victim: Option<String>,
    #[serde(rename = "Assisters", default)]
    pub assisters: Vec<String>,
    #[serde(rename = "KillStreak", default)]
    pub kill_streak: Option<u32>,
    #[serde(rename = "Result", default)]
    pub result: Option<String>,
    #[serde(rename = "Acer", default)]
    pub acer: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventEnvelope {
    #[serde(rename = "Events", default)]
    pub events: Vec<RawEvent>,
}

impl Default for LiveClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveClient {
    pub fn new() -> Self {
        let http = reqwest::blocking::Client::builder()
            // Riot signs this endpoint with its own root; it is loopback-only.
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_millis(800))
            .build()
            .expect("tls backend available");
        Self { http, base: "https://127.0.0.1:2999/liveclientdata".into() }
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<T> {
        Ok(self.http.get(format!("{}/{path}", self.base)).send()?.error_for_status()?.json()?)
    }

    pub fn game_stats(&self) -> anyhow::Result<GameStats> {
        self.get("gamestats")
    }

    /// Everything in one request: players, scores, events.
    pub fn all_game_data(&self) -> anyhow::Result<AllGameData> {
        self.get("allgamedata")
    }
}

// ---------------------------------------------------------------------------
// Highlight classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerKind {
    Kill,
    Death,
    Assist,
    Multikill,
    Ace,
    Objective,
    Structure,
    GameEnd,
    /// Placed by the player with the bookmark hotkey during the game.
    Bookmark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub kind: MarkerKind,
    pub label: String,
    /// In-game clock, seconds.
    pub game_time: f64,
}

/// Who "me" is in event payloads. Events name players by summoner name
/// (verified on 16.22: "Taric Bot"); Riot ID forms are accepted too.
#[derive(Debug, Clone, Default)]
pub struct Me {
    names: Vec<String>,
    pub team: Option<String>,
}

impl Me {
    pub fn from_game(data: &AllGameData) -> Option<Self> {
        let p = data.me()?;
        let mut names = vec![p.summoner_name.clone(), p.riot_id.clone(), p.riot_id_game_name.clone()];
        names.retain(|n| !n.is_empty());
        Some(Me { names, team: Some(p.team.clone()) })
    }

    pub fn is(&self, name: Option<&str>) -> bool {
        name.is_some_and(|n| self.names.iter().any(|m| m == n))
    }
}

pub fn classify(ev: &RawEvent, me: &Me) -> Option<Marker> {
    let mk = |kind, label: String| Some(Marker { kind, label, game_time: ev.time });
    match ev.name.as_str() {
        "ChampionKill" if me.is(ev.killer.as_deref()) => {
            mk(MarkerKind::Kill, format!("Killed {}", ev.victim.clone().unwrap_or_default()))
        }
        "ChampionKill" if me.is(ev.victim.as_deref()) => {
            mk(MarkerKind::Death, format!("Killed by {}", ev.killer.clone().unwrap_or_default()))
        }
        "ChampionKill" if ev.assisters.iter().any(|a| me.is(Some(a))) => {
            mk(MarkerKind::Assist, format!("Assist on {}", ev.victim.clone().unwrap_or_default()))
        }
        "Multikill" if me.is(ev.killer.as_deref()) => {
            let n = ev.kill_streak.unwrap_or(2);
            let label = match n {
                2 => "Double kill",
                3 => "Triple kill",
                4 => "Quadra kill",
                _ => "Penta kill",
            };
            mk(MarkerKind::Multikill, label.into())
        }
        "Ace" if me.is(ev.acer.as_deref()) => mk(MarkerKind::Ace, "Ace".into()),
        "DragonKill" | "BaronKill" | "HeraldKill" | "HordeKill" | "AtakhanKill" if me.is(ev.killer.as_deref()) => {
            mk(MarkerKind::Objective, ev.name.trim_end_matches("Kill").to_string())
        }
        "TurretKilled" | "InhibKilled" if me.is(ev.killer.as_deref()) => {
            mk(MarkerKind::Structure, ev.name.trim_end_matches("Killed").to_string())
        }
        "GameEnd" => mk(MarkerKind::GameEnd, ev.result.clone().unwrap_or_else(|| "Game end".into())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(json: &str) -> RawEvent {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn classifies_kills_and_deaths() {
        let me = Me { names: vec!["Faker".into()], team: None };
        let k = ev(r#"{"EventID":3,"EventName":"ChampionKill","EventTime":312.4,"KillerName":"Faker","VictimName":"Bob","Assisters":[]}"#);
        assert_eq!(classify(&k, &me).unwrap().kind, MarkerKind::Kill);
        let d = ev(r#"{"EventID":4,"EventName":"ChampionKill","EventTime":400.0,"KillerName":"Bob","VictimName":"Faker","Assisters":[]}"#);
        assert_eq!(classify(&d, &me).unwrap().kind, MarkerKind::Death);
        let other = ev(r#"{"EventID":5,"EventName":"ChampionKill","EventTime":401.0,"KillerName":"A","VictimName":"B","Assisters":[]}"#);
        assert!(classify(&other, &me).is_none());
    }
}

// ---------------------------------------------------------------------------
// League client (LCU) — queue name and patch. Read-only, loopback only.
// ---------------------------------------------------------------------------

pub mod lcu {
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    const LOCKFILES: &[&str] = &["/Applications/League of Legends.app/Contents/LoL/lockfile"];
    #[cfg(windows)]
    const LOCKFILES: &[&str] = &[r"C:\Riot Games\League of Legends\lockfile"];
    #[cfg(not(any(target_os = "macos", windows)))]
    const LOCKFILES: &[&str] = &[];

    #[derive(Debug, Clone, Default)]
    pub struct MatchInfo {
        /// e.g. "Ranked Solo/Duo", "Multiplayer Practice Tool".
        pub queue: Option<String>,
        pub queue_id: Option<i64>,
        /// "16.22"
        pub patch: Option<String>,
        /// "CLASSIC", "ARAM", "PRACTICETOOL", "CHERRY"...
        pub game_mode: Option<String>,
        pub ranked: bool,
        pub custom: bool,
        /// Riot's match id (also set for Practice Tool).
        pub game_id: Option<i64>,
    }

    pub fn match_info() -> Option<MatchInfo> {
        let lock = LOCKFILES.iter().find_map(|p| std::fs::read_to_string(p).ok())?;
        // name:pid:port:password:protocol
        let parts: Vec<&str> = lock.trim().split(':').collect();
        let (port, password) = (parts.get(2)?, parts.get(3)?);
        let http = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(2))
            .build()
            .ok()?;
        let get = |path: &str| -> Option<serde_json::Value> {
            http.get(format!("https://127.0.0.1:{port}{path}"))
                .basic_auth("riot", Some(password))
                .send()
                .ok()?
                .json()
                .ok()
        };
        let session = get("/lol-gameflow/v1/session").unwrap_or_default();
        let queue = &session["gameData"]["queue"];
        let patch = get("/lol-patch/v1/game-version")
            .and_then(|v| v.as_str().map(str::to_owned))
            .map(|v| v.split('.').take(2).collect::<Vec<_>>().join("."));
        Some(MatchInfo {
            queue: queue["description"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
            queue_id: queue["id"].as_i64(),
            patch,
            game_mode: queue["gameMode"].as_str().map(str::to_owned),
            ranked: queue["isRanked"].as_bool().unwrap_or(false),
            custom: session["gameData"]["isCustomGame"].as_bool().unwrap_or(false),
            game_id: session["gameData"]["gameId"].as_i64().filter(|id| *id != 0),
        })
    }
}
