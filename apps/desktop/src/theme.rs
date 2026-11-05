//! Palette. Near-black base with a faint cool tint, one azure accent, and
//! event/result colours from League's own vernacular (victory blue, defeat
//! red, gold for kills) carrying the meaning.

use gpui_kit::{Hsla, rgb};
use timewarp_league::MarkerKind;

pub fn bg() -> Hsla {
    rgb(0x0B0C10).into()
}
pub fn sidebar() -> Hsla {
    rgb(0x07080B).into()
}
pub fn panel() -> Hsla {
    rgb(0x13151B).into()
}
pub fn raised() -> Hsla {
    rgb(0x1B1E26).into()
}
pub fn line() -> Hsla {
    rgb(0x262A35).into()
}
pub fn text() -> Hsla {
    rgb(0xECEFF4).into()
}
pub fn muted() -> Hsla {
    rgb(0x8B92A1).into()
}

pub fn kill() -> Hsla {
    rgb(0xE2B14A).into()
}
pub fn death() -> Hsla {
    rgb(0xE0505A).into()
}
pub fn assist() -> Hsla {
    rgb(0x3DB8A5).into()
}
pub fn objective() -> Hsla {
    rgb(0xA78BFA).into()
}
/// The one accent: selection, primary buttons, focus, links.
pub fn accent() -> Hsla {
    victory()
}
pub fn victory() -> Hsla {
    rgb(0x3B9EFF).into()
}
pub fn defeat() -> Hsla {
    death()
}
pub fn live() -> Hsla {
    rgb(0xFF4D5E).into()
}

pub fn marker(kind: MarkerKind) -> Hsla {
    match kind {
        MarkerKind::Kill | MarkerKind::Multikill | MarkerKind::Ace => kill(),
        MarkerKind::Death => death(),
        MarkerKind::Assist => assist(),
        MarkerKind::Objective | MarkerKind::Structure => objective(),
        MarkerKind::GameEnd => muted(),
        MarkerKind::Bookmark => text(),
    }
}

pub fn moment(kind: crate::store::MomentKind) -> Hsla {
    match kind {
        crate::store::MomentKind::Event(k) => marker(k),
        crate::store::MomentKind::Note => text(),
    }
}

pub fn result(result: Option<&str>) -> Hsla {
    match result {
        Some("Win") => victory(),
        Some("Lose") => defeat(),
        _ => line(),
    }
}

pub fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
