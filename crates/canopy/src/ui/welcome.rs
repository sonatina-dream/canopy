//! First-run setup, one step at a time: the permissions Canopy needs, the
//! macOS features that fight it, and the first move.

use std::sync::OnceLock;
use std::time::Duration;

use gpui::{
    div, prelude::FluentBuilder, px, AnyElement, App, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window,
};

use super::theme::{theme, Theme};
use super::widgets::*;
use crate::bus::{self, Msg};
use crate::coordinator::Coordinator;
use crate::mac::features::MacFeature;
use crate::mac::{ax, window_server, workspace};
use crate::store;

pub const ACCESSIBILITY_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
pub const SCREEN_RECORDING_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

static PRETEND_NO_SCREEN_RECORDING: OnceLock<bool> = OnceLock::new();

/// `--pretend-no-screen-recording` shows the request flow without touching
/// the real permission (for checking the UI).
pub fn set_pretend_no_screen_recording(on: bool) {
    let _ = PRETEND_NO_SCREEN_RECORDING.set(on);
}

fn pretend() -> bool {
    PRETEND_NO_SCREEN_RECORDING.get().copied().unwrap_or(false)
}

pub fn screen_recording_allowed() -> bool {
    !pretend() && window_server::screen_capture_allowed()
}

/// The same, as last checked, for the main thread's frequent paths.
pub fn screen_recording_last_known() -> bool {
    !pretend() && crate::mac::capture::access_allowed()
}

pub fn should_show() -> bool {
    !store::get_bool("welcomed") || !ax::is_trusted()
}

pub struct WelcomeView {
    coordinator: Entity<Coordinator>,
    step: usize,
    ax_trusted: bool,
    screen_allowed: bool,
    /// Only the Screen Recording step: Canopy asking again on a later launch.
    screen_only: bool,
    features_on: Vec<MacFeature>,
    /// Screen Recording takes effect only after a relaunch.
    needs_reopen: bool,
    done: bool,
}

fn features_on() -> Vec<MacFeature> {
    MacFeature::ALL.into_iter().filter(|f| f.is_on()).collect()
}

impl WelcomeView {
    pub fn new(coordinator: Entity<Coordinator>, screen_only: bool, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            if this.update(cx, |view, cx| view.poll(cx)).is_err() {
                break;
            }
        })
        .detach();
        let ax_trusted = ax::is_trusted();
        let resume = (!screen_only).then(|| store::get_int("welcomeResumeStep")).flatten();
        if resume.is_some() {
            store::remove_key("welcomeResumeStep");
        }
        let step = if screen_only {
            1
        } else {
            resume.map_or(usize::from(ax_trusted), |s| s as usize)
        };
        Self {
            coordinator,
            step,
            ax_trusted,
            screen_allowed: screen_recording_allowed(),
            screen_only,
            features_on: features_on(),
            needs_reopen: false,
            done: false,
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        let trusted = ax::is_trusted();
        if trusted && !self.ax_trusted {
            self.ax_trusted = true;
            self.coordinator.update(cx, |c, cx| c.start_after_trust(cx));
            if self.step == 0 {
                self.step = 1;
            }
        }
        self.screen_allowed = screen_recording_allowed();
        self.features_on = features_on();
        cx.notify();
    }

    #[cfg(feature = "snapshots")]
    pub fn show_step(&mut self, step: usize, cx: &mut Context<Self>) {
        self.step = step;
        cx.notify();
    }

    pub fn go_to_screen_recording(&mut self, cx: &mut Context<Self>) {
        self.step = 1;
        cx.notify();
    }

    /// macOS shows its own prompt the first time; after that, the Settings pane.
    fn ask_screen_recording(&mut self, cx: &mut Context<Self>) {
        if pretend() || !window_server::request_screen_capture() {
            workspace::open_url(SCREEN_RECORDING_SETTINGS);
        }
        self.needs_reopen = true;
        cx.notify();
    }

    /// Leaves the Screen Recording step.
    fn after_screen_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen_only {
            self.done = true;
            window.remove_window();
        } else {
            self.step = 2;
            cx.notify();
        }
    }

    fn turn_off_features(&mut self, cx: &mut Context<Self>) {
        for f in &self.features_on {
            f.set(false);
        }
        self.features_on = features_on();
        cx.notify();
    }

    fn reopen(&self) {
        if !self.screen_only {
            store::set_int("welcomeResumeStep", 2);
        }
        let path = objc2_foundation::NSBundle::mainBundle().bundlePath().to_string();
        let _ = std::process::Command::new("/bin/sh").args(["-c", "sleep 1; /usr/bin/open \"$0\"", &path]).spawn();
        bus::send(Msg::Quit);
    }

    fn finish(&mut self, window: &mut Window) {
        Coordinator::first_launch_done();
        self.done = true;
        window.remove_window();
    }

    fn heading(title: &str, detail: &str, theme: &Theme) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().text_size(px(22.0)).font_weight(FontWeight::BOLD).text_color(theme.text).child(title.to_string()))
            .child(div().text_size(px(13.0)).line_height(px(18.0)).text_color(theme.secondary).child(detail.to_string()))
            .into_any_element()
    }

    fn allowed(theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_size(px(13.0))
            .text_color(theme.green)
            .children(symbol("checkmark.circle.fill", 14.0, theme.green))
            .child("Allowed")
    }

    fn accessibility(&self, theme: &Theme) -> Vec<AnyElement> {
        vec![
            Self::heading(
                "Let Canopy arrange your windows",
                "macOS only lets apps you trust move windows. Click Allow…, then switch Canopy on in System Settings ▸ Privacy & Security ▸ Accessibility.",
                theme,
            ),
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(button("allow", "Allow…", true, theme, |_, _| {
                    ax::request_trust(true);
                    workspace::open_url(ACCESSIBILITY_SETTINGS);
                }))
                .when(self.ax_trusted, |d| d.child(Self::allowed(theme)))
                .into_any_element(),
            caption("Canopy carries on by itself once it's allowed.", theme).into_any_element(),
        ]
    }

    fn screen_recording(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let title = if self.screen_only { "Let Canopy see your windows" } else { "See your windows as they are" };
        let mut out = vec![Self::heading(
            title,
            "With Screen Recording, windows keep their real contents while they move, instead of showing as their app's colour and icon. Canopy only takes still pictures to draw motion; nothing is recorded or kept.",
            theme,
        )];
        if !self.screen_allowed {
            out.push(
                text("Click Allow…, switch Canopy on in System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording, then reopen Canopy.", theme)
                    .line_height(px(18.0))
                    .into_any_element(),
            );
        }
        let view = cx.entity().downgrade();
        let advance = |label: &'static str, primary: bool, theme: &Theme| {
            let view = view.clone();
            button(label, label, primary, theme, move |window, cx: &mut App| {
                let _ = view.update(cx, |v, cx| v.after_screen_recording(window, cx));
            })
        };
        let actions = div().flex().items_center().gap(px(10.0));
        let actions = if self.screen_allowed {
            actions.child(Self::allowed(theme)).child(div().flex_1()).child(advance("Continue", true, theme))
        } else if self.needs_reopen {
            let v = view.clone();
            actions
                .child(button("reopen", "Reopen Canopy", true, theme, move |_, cx| {
                    let _ = v.read_with(cx, |v, _| v.reopen());
                }))
                .child(caption("macOS applies this when Canopy reopens.", theme))
                .child(div().flex_1())
                .child(advance("Not Now", false, theme))
        } else {
            let v = view.clone();
            actions
                .child(button("allow", "Allow…", true, theme, move |_, cx| {
                    let _ = v.update(cx, |v, cx| v.ask_screen_recording(cx));
                }))
                .child(div().flex_1())
                .child(advance("Not Now", false, theme))
        };
        out.push(actions.into_any_element());
        out
    }

    fn features(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![Self::heading(
            "Turn off what gets in the way",
            "These macOS features move windows on their own and fight Canopy. You can turn them back on any time in Canopy's Settings ▸ General.",
            theme,
        )];
        if self.features_on.is_empty() {
            out.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(13.0))
                    .text_color(theme.green)
                    .children(symbol("checkmark.circle.fill", 14.0, theme.green))
                    .child("Nothing to turn off.")
                    .into_any_element(),
            );
        } else {
            let list = self.features_on.iter().fold(div().flex().flex_col().gap(px(6.0)), |list, f| {
                list.child(text(format!("• {}: {}", f.title(), f.detail()), theme).line_height(px(17.0)))
            });
            out.push(list.into_any_element());
        }
        let view = cx.entity().downgrade();
        let turn_off = view.clone();
        let next = move |_: &mut Window, cx: &mut App| {
            let _ = view.update(cx, |v, cx| {
                v.step = 3;
                cx.notify();
            });
        };
        let empty = self.features_on.is_empty();
        out.push(
            div()
                .flex()
                .items_center()
                .when(!empty, |d| {
                    d.child(button("off", "Turn Them Off", true, theme, move |_, cx| {
                        let _ = turn_off.update(cx, |v, cx| v.turn_off_features(cx));
                    }))
                })
                .child(div().flex_1())
                .child(button("keep", if empty { "Continue" } else { "Keep Them" }, empty, theme, next))
                .into_any_element(),
        );
        out
    }

    fn start(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let tip = |icon: &str, s: &str| {
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .children(symbol(icon, 15.0, theme.accent))
                .child(text(s.to_string(), theme).flex_1().min_w_0().line_height(px(17.0)))
        };
        let view = cx.entity().downgrade();
        vec![
            Self::heading("Your first move", "Nothing moves until you dock a window yourself.", theme),
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(tip("rectangle.split.2x1", "Drag a window by its title bar to the edge of the screen, or of another window. Let go and it docks."))
                .child(tip("menubar.rectangle", "Drop a window on the middle of another and pause to make tabs."))
                .child(tip("arrow.up.left.and.arrow.down.right", "⌃⌥Space shows everything. Double-click a title bar to give it the screen."))
                .child(tip("circle.fill", "The green button docks and floats a window."))
                .into_any_element(),
            div()
                .flex()
                .child(div().flex_1())
                .child(button("start", "Start Using Canopy", true, theme, move |window, cx| {
                    let _ = view.update(cx, |v, _| v.finish(window));
                }))
                .into_any_element(),
        ]
    }
}

impl Render for WelcomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(window);
        let body = match self.step {
            0 => self.accessibility(&theme),
            1 => self.screen_recording(&theme, cx),
            2 => self.features(&theme, cx),
            _ => self.start(&theme, cx),
        };
        let step = self.step;
        let progress = (!self.screen_only).then(|| {
            (0..4).fold(div().flex().gap(px(6.0)), |bar, i| {
                bar.child(div().flex_1().h(px(4.0)).rounded(px(2.0)).bg(if i <= step { theme.accent } else { theme.track }))
            })
        });
        div()
            .id("welcome")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .p(px(28.0))
            .pt(px(40.0))
            .bg(theme.window)
            .children(progress)
            .children(body)
    }
}
