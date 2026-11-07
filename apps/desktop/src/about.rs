//! About Timewarp: version, links, what's new and acknowledgements.
//! Opened from the sidebar, the app menu (Timewarp → About Timewarp) or ⌘K.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::ui::{self, icon};
use crate::{TimewarpApp, theme};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Short git commit of this build (empty without git).
pub const COMMIT: &str = env!("TIMEWARP_COMMIT");

/// "0.1.0 (3f9c2a1b)", or just the version without a commit.
pub fn version_line() -> String {
    if COMMIT.is_empty() { VERSION.to_string() } else { format!("{VERSION} ({COMMIT})") }
}
/// From `repository` in the workspace Cargo.toml.
pub const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// The libraries Timewarp uses directly, with licence texts (made by build.rs).
const LICENSES: &str = include_str!(concat!(env!("OUT_DIR"), "/licenses.json"));

#[derive(serde::Deserialize)]
struct Package {
    name: String,
    version: String,
    license: String,
    url: String,
    #[serde(default)]
    description: String,
    texts: Vec<usize>,
    /// The package ships no licence file; this is the standard text.
    standard: bool,
}

#[derive(serde::Deserialize)]
struct Licenses {
    packages: Vec<Package>,
    texts: Vec<String>,
}

fn licenses() -> &'static Licenses {
    static L: std::sync::OnceLock<Licenses> = std::sync::OnceLock::new();
    L.get_or_init(|| serde_json::from_str(LICENSES).unwrap_or(Licenses { packages: vec![], texts: vec![] }))
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AboutTab {
    #[default]
    Overview,
    Changes,
    Credits,
}

const RIOT: &str = "Timewarp isn't endorsed by Riot Games and doesn't reflect the views or opinions of Riot Games or anyone officially involved in producing or managing Riot Games properties. Riot Games, and all associated properties are trademarks or registered trademarks of Riot Games, Inc.";

/// "macOS 26.1 (arm64)", for bug reports.
fn system() -> String {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        #[cfg(target_os = "macos")]
        let os = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|o| format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()))
            .unwrap_or_else(|| "macOS".into());
        #[cfg(not(target_os = "macos"))]
        let os = std::env::consts::OS.to_string();
        format!("{os} ({})", std::env::consts::ARCH)
    })
    .clone()
}

impl TimewarpApp {
    fn about_overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let info = format!("Timewarp {}, {}", version_line(), system());
        let link = |id: &'static str, glyph: IconName, label: &'static str, url: String| {
            ui::solid_button(id, theme::raised())
                .child(icon(glyph, 14., theme::text()))
                .child(label)
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .pt_12()
            .pb_8()
            .child(
                div()
                    .size(px(72.))
                    .rounded_2xl()
                    .bg(theme::victory())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(IconName::Rewind, 36., theme::bg())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Timewarp"))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_sm()
                            .text_color(theme::muted())
                            .child(format!("Version {VERSION}"))
                            .when(!COMMIT.is_empty(), |d| {
                                let url = format!("{REPO}/commit/{COMMIT}");
                                d.child("·").child(
                                    div()
                                        .id("about-commit")
                                        .font_family("Menlo")
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(theme::accent()))
                                        .child(COMMIT)
                                        .tooltip(ui::tip("The commit this build was made from"))
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                            }),
                    )
                    .child(div().pt_1().max_w(px(420.)).text_center().text_color(theme::muted()).child(
                        "Records your League of Legends games and helps you learn from them.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .child(link("about-gh", IconName::Github, "GitHub", REPO.to_string()))
                    .child(link("about-issue", IconName::Bug, "Report a problem", format!("{REPO}/issues/new")))
                    .child(
                        ui::ghost_button("about-whatsnew")
                            .child(icon(IconName::Sparkles, 14., theme::muted()))
                            .child("What's new")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.about_tab = AboutTab::Changes;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("about-copy")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme::muted())
                    .hover(|s| s.bg(theme::raised()))
                    .child(info.clone())
                    .child(icon(IconName::Copy, 12., theme::muted()))
                    .tooltip(ui::tip("Copy, for bug reports"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(info.clone()));
                        this.toast(IconName::Copy, "Version info copied.", None, cx);
                    })),
            )
            .child(div().pt_4().w_full().flex().justify_center().child(self.updates_card(cx)))
            .child(div().pt_6().text_xs().text_color(theme::muted()).child("MIT License. Made with Rust."))
            .into_any_element()
    }

    fn about_credits(&self, cx: &mut Context<Self>) -> AnyElement {
        let l = licenses();
        let rows = l.packages.iter().enumerate().map(|(i, p)| {
            let open = self.about_open == Some(i);
            let url = p.url.clone();
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id(("lib", i))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .when(open, |d| d.bg(theme::panel()))
                        .hover(|s| s.bg(theme::panel()))
                        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme::muted()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap_2()
                                        .child(div().font_weight(FontWeight::MEDIUM).child(p.name.clone()))
                                        .child(div().text_xs().text_color(theme::muted()).child(p.version.clone())),
                                )
                                .when(!p.description.is_empty(), |d| {
                                    d.child(div().truncate().text_sm().text_color(theme::muted()).child(p.description.clone()))
                                }),
                        )
                        .child(div().flex_none().text_xs().text_color(theme::muted()).child(p.license.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.about_open = if this.about_open == Some(i) { None } else { Some(i) };
                            cx.notify();
                        })),
                )
                .when(open, |d| {
                    d.child(
                        div()
                            .ml_8()
                            .mr_3()
                            .mb_2()
                            .p_3()
                            .rounded_md()
                            .bg(theme::panel())
                            .flex()
                            .flex_col()
                            .gap_3()
                            .when(!url.is_empty(), |d| {
                                d.child(
                                    div()
                                        .id(("lib-url", i))
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .text_xs()
                                        .text_color(theme::accent())
                                        .cursor_pointer()
                                        .child(icon(IconName::ExternalLink, 11., theme::accent()))
                                        .child(url.clone())
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                            })
                            .when(p.standard, |d| {
                                d.child(div().text_xs().text_color(theme::muted()).child(
                                    "This library doesn't include its own licence file, so this is the standard text of the licence it declares.",
                                ))
                            })
                            .children(p.texts.iter().map(|&t| {
                                div().text_xs().font_family("Menlo").text_color(theme::muted()).child(l.texts[t].clone())
                            })),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(div().px_3().pb_2().text_sm().text_color(theme::muted()).child(format!(
                "Timewarp is built on these {} open-source libraries. Click one for its licence. Thank you to everyone who works on them.",
                l.packages.len()
            )))
            .children(rows.collect::<Vec<_>>())
            .child(div().pt_5().pb_1().px_3().font_weight(FontWeight::MEDIUM).child("Also"))
            .child(div().px_3().text_sm().text_color(theme::muted()).child(
                "GPUI, the UI framework by Zed Industries, comes with gpui-kit. Icons are from Lucide (ISC licence). Apple's ScreenCaptureKit and AVFoundation do the recording and playback. Champion portraits come from Riot's Data Dragon, and match events from the game's Live Client Data API.",
            ))
            .child(div().pt_4().px_3().text_xs().text_color(theme::muted()).child(RIOT))
            .into_any_element()
    }

    pub fn about_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, t: AboutTab, label: &'static str, cx: &mut Context<Self>| {
            let active = self.about_tab == t;
            ui::ghost_button(id)
                .when(active, |b| b.bg(theme::raised()).text_color(theme::text()))
                .when(!active, |b| b.text_color(theme::muted()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.about_tab = t;
                    cx.notify();
                }))
        };
        let header = ui::page_header()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("About"))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(tab("about-tab-1", AboutTab::Overview, "Timewarp", cx))
                    .child(tab("about-tab-2", AboutTab::Changes, "What's new", cx))
                    .child(tab("about-tab-3", AboutTab::Credits, "Acknowledgements", cx)),
            )
            .child(ui::drag_area());
        let body = match self.about_tab {
            AboutTab::Overview => self.about_overview(cx),
            AboutTab::Changes => ui::markdown("changelog", CHANGELOG.trim_start_matches("# Changelog").trim().to_string()).into_any_element(),
            AboutTab::Credits => self.about_credits(cx),
        };
        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("about")
                .flex_1()
                .overflow_y_scroll()
                .child(div().max_w(px(720.)).mx_auto().px_6().py_5().child(body)),
        )
    }
}
