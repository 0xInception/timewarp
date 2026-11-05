use timewarp_league::{AllGameData, Me, MarkerKind, classify};

#[test]
fn parses_real_practice_tool_payload() {
    let raw = include_str!("fixtures/allgamedata_practice.json");
    let data: AllGameData = serde_json::from_str(raw).expect("allgamedata parses");
    let me = data.me().expect("active player found by riot id / summoner name");
    assert_eq!(me.champion_name, "Amumu");
    assert_eq!(me.champion_key(), "Amumu");
    assert!(data.lane_opponent().is_none(), "practice tool has no positions for the player");
    assert!(!data.events.events.is_empty());

    // Bots are named by summoner name in events.
    let taric = Me::from_game(&AllGameData {
        active_player: timewarp_league::ActivePlayer { summoner_name: "Taric Bot".into(), ..Default::default() },
        ..data.clone()
    })
    .unwrap();
    let kill = data.events.events.iter().find(|e| e.name == "ChampionKill").unwrap();
    assert_eq!(classify(kill, &taric).unwrap().kind, MarkerKind::Kill);
}
