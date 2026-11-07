//! Setup: what Timewarp needs from the system, whether it has it, and exactly
//! how to fix what's missing. Checked continuously, so it turns green as soon
//! as you flip the switch in System Settings.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::permissions::{Check, Fix, State};
use crate::ui::{self, icon};
use crate::{Page, TimewarpApp, theme};

impl TimewarpApp {
    pub fn run_fix(&mut self, fix: Fix, cx: &mut Context<Self>) {
        match fix {
            #[cfg(target_os = "macos")]
            Fix::RequestScreenRecording => {
                crate::permissions::mac::request_screen_recording();
                // The prompt only appears once; the pane always works.
                let _ = std::process::Command::new("open").arg(crate::permissions::mac::SCREEN_PANE).spawn();
            }
            #[cfg(target_os = "macos")]
            Fix::RequestMicrophone => crate::permissions::mac::request_microphone(),
            #[cfg(not(target_os = "macos"))]
            Fix::RequestScreenRecording | Fix::RequestMicrophone => {}
            Fix::OpenSettings(url) => {
                let _ = std::process::Command::new("open").arg(url).spawn();
            }
            Fix::Restart => {
                // Start a fresh copy, then quit this one the safe way.
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).args(std::env::args().skip(1)).spawn();
                }
                cx.quit();
            }
            Fix::ChooseFolder => self.choose_folder(cx),
            Fix::GoToStorageSettings => self.go(Page::Settings, cx),
        }
        // Re-check soon so the screen reflects the change.
        self.checks = crate::permissions::checks(&self.config);
        cx.notify();
    }

    /// Red bar on every page while something required is missing.
    pub fn setup_banner(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.page == Page::Setup {
            return None;
        }
        let c = self.checks.iter().find(|c| c.blocking())?;
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .h(px(40.))
                .px_5()
                .bg(theme::death().opacity(0.14))
                .border_b_1()
                .border_color(theme::death().opacity(0.45))
                .child(icon(IconName::TriangleAlert, 15., theme::death()))
                .child(div().flex_1().text_sm().child(format!("Timewarp can't record yet: {} needs your attention.", c.title)))
                .child(
                    ui::solid_button("banner-fix", theme::death())
                        .h(px(28.))
                        .text_color(theme::text())
                        .child("Fix this")
                        .on_click(cx.listener(|this, _, _, cx| this.go(Page::Setup, cx))),
                ),
        )
    }

    fn check_card(&self, i: usize, c: &Check, cx: &mut Context<Self>) -> AnyElement {
        let (glyph, color) = match c.state {
            State::Ok => (IconName::Check, theme::assist()),
            State::Missing => (IconName::X, theme::death()),
            State::NotAsked | State::Warning => (IconName::TriangleAlert, theme::kill()),
            State::NotNeeded => (IconName::Check, theme::muted()),
        };
        let muted = matches!(c.state, State::Ok | State::NotNeeded);
        div()
            .flex()
            .gap_4()
            .p_4()
            .rounded_lg()
            .bg(if muted { theme::bg() } else { theme::panel() })
            .border_1()
            .border_color(if c.blocking() { theme::death().opacity(0.5) } else { theme::line() })
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.opacity(0.15))
                    .child(icon(glyph, 15., color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(c.title))
                            .when(c.required, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded(px(4.))
                                        .bg(theme::raised())
                                        .text_xs()
                                        .text_color(theme::muted())
                                        .child("Required"),
                                )
                            }),
                    )
                    .child(div().text_sm().text_color(theme::muted()).child(c.why.clone()))
                    // Numbered: these are steps, in order.
                    .when(!c.steps.is_empty(), |d| {
                        d.child(div().flex().flex_col().gap_1p5().pt_2().children(c.steps.iter().enumerate().map(|(n, step)| {
                            div()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .size(px(18.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(theme::raised())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .child((n + 1).to_string()),
                                )
                                .child(div().flex_1().child(step.clone()))
                        })))
                    })
                    .when(!c.fixes.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap_2().pt_3().children(c.fixes.iter().enumerate().map(|(k, (fix, label))| {
                            let fix = *fix;
                            let primary = k == 0;
                            let el = ui::solid_button(("fix", i * 10 + k), if primary { theme::accent() } else { theme::raised() })
                                .text_color(if primary { theme::bg() } else { theme::text() })
                                .when(matches!(fix, Fix::OpenSettings(_)), |b| b.child(icon(IconName::ExternalLink, 13., theme::text())))
                                .when(matches!(fix, Fix::Restart), |b| b.child(icon(IconName::RotateCw, 13., theme::text())))
                                .child(*label);
                            el.on_click(cx.listener(move |this, _, _, cx| this.run_fix(fix, cx)))
                        })))
                    }),
            )
            .into_any_element()
    }

    pub fn setup_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let issues = self.checks.iter().filter(|c| c.needs_attention()).count();
        let blocking = self.checks.iter().any(|c| c.blocking());
        let header = ui::page_header().child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Setup")).child(ui::drag_area());

        let summary = div()
            .flex()
            .items_center()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(if blocking { theme::death().opacity(0.5) } else { theme::line() })
            .bg(if blocking { theme::death().opacity(0.08) } else { theme::panel() })
            .child(icon(
                if issues == 0 { IconName::ShieldCheck } else { IconName::ShieldAlert },
                20.,
                if blocking { theme::death() } else if issues > 0 { theme::kill() } else { theme::assist() },
            ))
            .child(div().flex_1().min_w_0().flex().flex_col().child(div().font_weight(FontWeight::SEMIBOLD).child(match (blocking, issues) {
                (_, 0) => "Everything's ready".to_string(),
                (true, _) => "Timewarp can't record yet".to_string(),
                (false, 1) => "1 thing could use your attention".to_string(),
                (false, n) => format!("{n} things could use your attention"),
            })).child(div().text_sm().text_color(theme::muted()).child(if issues == 0 {
                "Your next match records automatically. This page updates on its own, so you can keep it open while you change things.".to_string()
            } else {
                "Follow the steps below. This page re-checks every few seconds and turns green as soon as something is fixed.".to_string()
            })));

        div().size_full().flex().flex_col().child(header).child(
            div()
                .id("setup")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(760.))
                        .px_6()
                        .py_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(summary)
                        .children(self.checks.iter().enumerate().map(|(i, c)| self.check_card(i, c, cx)).collect::<Vec<_>>())
                        .child(div().pt_2().text_sm().text_color(theme::muted()).child(
                            "Timewarp doesn't need Accessibility, Input Monitoring or Full Disk Access. The bookmark key works without them, and nothing reads the game's memory.",
                        )),
                ),
        )
    }
}
