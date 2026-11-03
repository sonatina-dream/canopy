//! The Settings window: seven panes over the coordinator's preferences.

use std::cell::Cell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Duration;

use block2::RcBlock;
use canopy_core::{Command, KeyCombo, Preferences, SettingsPane, Shortcuts};
use gpui::{
    div, prelude::FluentBuilder, px, AnyElement, App, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSBeep, NSEvent, NSEventMask, NSEventModifierFlags};

use super::theme::{theme, Theme};
use super::welcome::{self, screen_recording_allowed};
use super::widgets::*;
use crate::bus::{self, Msg};
use crate::coordinator::Coordinator;
use crate::mac::features::{login_item, MacFeature};
use crate::mac::menu::show_apps_menu;
use crate::mac::{ax, hotkeys, window_server, workspace};

pub struct SettingsView {
    coordinator: Entity<Coordinator>,
    pane: SettingsPane,
    ax_trusted: bool,
    screen_allowed: bool,
    features: HashMap<MacFeature, bool>,
    login_enabled: bool,
    login_error: Option<String>,
    recording: Option<Command>,
    monitor: Option<Retained<AnyObject>>,
    sliders: HashMap<&'static str, Rc<Cell<bool>>>,
}

impl SettingsView {
    pub fn new(coordinator: Entity<Coordinator>, pane: Option<SettingsPane>, cx: &mut Context<Self>) -> Self {
        cx.observe(&coordinator, |_, _, cx| cx.notify()).detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            if this.update(cx, |view, cx| view.refresh_status(cx)).is_err() {
                break;
            }
        })
        .detach();
        let mut view = Self {
            coordinator,
            pane: pane.unwrap_or(SettingsPane::General),
            ax_trusted: false,
            screen_allowed: false,
            features: HashMap::new(),
            login_enabled: false,
            login_error: None,
            recording: None,
            monitor: None,
            sliders: HashMap::new(),
        };
        view.read_status();
        view
    }

    fn read_status(&mut self) {
        self.ax_trusted = ax::is_trusted();
        self.screen_allowed = screen_recording_allowed();
        self.features = MacFeature::ALL.iter().map(|f| (*f, f.is_on())).collect();
        self.login_enabled = login_item::is_enabled();
    }

    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        self.read_status();
        cx.notify();
    }

    pub fn show_pane(&mut self, pane: SettingsPane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.refresh_status(cx);
    }

    fn slider_state(&mut self, name: &'static str) -> Rc<Cell<bool>> {
        self.sliders.entry(name).or_default().clone()
    }

    /// Changes preferences. The window that took the click is in use until
    /// its handler returns, so the coordinator runs just after.
    fn change(coordinator: &Entity<Coordinator>, cx: &mut App, edit: impl FnOnce(&mut Preferences) + 'static) {
        let coordinator = coordinator.clone();
        cx.defer(move |cx| {
            coordinator.update(cx, |c, cx| {
                let mut p = c.prefs.clone();
                edit(&mut p);
                c.set_prefs(p, cx);
            });
        });
    }

    // MARK: Recording shortcuts

    fn start_recording(&mut self, command: Command, cx: &mut Context<Self>) {
        let same = self.recording == Some(command);
        self.stop_recording(cx);
        if same {
            return;
        }
        self.recording = Some(command);
        self.coordinator.update(cx, |c, _| c.pause_hotkeys());
        let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a valid event.
            let event = unsafe { event.as_ref() };
            bus::send(Msg::RecordedKey { key_code: event.keyCode(), modifiers: event.modifierFlags().bits() });
            // Swallowed: the key is for the recorder, not the window.
            std::ptr::null_mut()
        });
        // SAFETY: the block is copied by the call, and only reads the event.
        self.monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler) };
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(monitor) = self.monitor.take() {
            // SAFETY: the monitor came from `addLocalMonitorForEvents`.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        if self.recording.take().is_some() {
            self.coordinator.update(cx, |c, _| c.resume_hotkeys());
        }
        cx.notify();
    }

    pub fn key_recorded(&mut self, key_code: u16, modifiers: usize, cx: &mut Context<Self>) {
        let Some(command) = self.recording else { return };
        const ESCAPE: u16 = 53;
        const DELETE: u16 = 51;
        if key_code == ESCAPE {
            self.stop_recording(cx);
            return;
        }
        if key_code == DELETE {
            self.set_binding(command, None, cx);
            self.stop_recording(cx);
            return;
        }
        let flags = NSEventModifierFlags::from_bits_truncate(modifiers);
        let mut m = 0;
        for (flag, bit) in [
            (NSEventModifierFlags::Control, KeyCombo::CONTROL),
            (NSEventModifierFlags::Option, KeyCombo::OPTION),
            (NSEventModifierFlags::Shift, KeyCombo::SHIFT),
            (NSEventModifierFlags::Command, KeyCombo::COMMAND),
        ] {
            if flags.contains(flag) {
                m |= bit;
            }
        }
        let combo = KeyCombo::new(u32::from(key_code), m);
        if combo.is_valid() {
            self.set_binding(command, Some(combo), cx);
            self.stop_recording(cx);
        } else {
            NSBeep();
        }
    }

    fn set_binding(&mut self, command: Command, combo: Option<KeyCombo>, cx: &mut Context<Self>) {
        self.coordinator.update(cx, |c, _| {
            let mut b = c.bindings.clone();
            // One combination does one thing: take it from any other command.
            if let Some(combo) = combo {
                b.retain(|_, v| *v != combo);
                b.insert(command, combo);
            } else {
                b.remove(&command);
            }
            c.set_bindings(b);
        });
    }

    // MARK: Panes

    fn tab_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        const ICONS: [&str; 7] = ["gearshape", "rectangle.split.3x1", "menubar.rectangle", "paintbrush", "hand.draw", "app.badge", "keyboard"];
        let mut bar = div().flex().justify_center().gap(px(2.0)).pt(px(8.0)).pb(px(6.0)).border_b_1().border_color(theme.separator);
        for (pane, icon) in SettingsPane::ALL.into_iter().zip(ICONS) {
            let chosen = pane == self.pane;
            let tint = if chosen { theme.accent } else { theme.secondary };
            bar = bar.child(
                div()
                    .id(pane.name())
                    .w(px(78.0))
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.0))
                    .rounded(px(6.0))
                    .when(chosen, |d| d.bg(theme.separator))
                    .cursor_pointer()
                    .on_click(cx.listener(move |view, _, _, cx| view.show_pane(pane, cx)))
                    .children(symbol(icon, 19.0, tint))
                    .child(div().text_size(px(11.0)).text_color(tint).child(pane.title())),
            );
        }
        bar.into_any_element()
    }

    fn permissions(&self, theme: &Theme) -> AnyElement {
        let status = |ok: bool, bad: gpui::Hsla| {
            div().text_size(px(13.0)).text_color(if ok { theme.green } else { bad }).child(if ok { "Allowed" } else { "Not allowed" })
        };
        let accessibility = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(status(self.ax_trusted, theme.red))
            .when(!self.ax_trusted, |d| {
                d.child(button("allow-ax", "Allow…", false, theme, |_, _| {
                    ax::request_trust(true);
                    workspace::open_url(welcome::ACCESSIBILITY_SETTINGS);
                }))
            });
        let screen = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(status(self.screen_allowed, theme.secondary))
            .when(!self.screen_allowed, |d| {
                d.child(button("allow-screen", "Allow…", false, theme, |_, _| {
                    if !window_server::request_screen_capture() {
                        workspace::open_url(welcome::SCREEN_RECORDING_SETTINGS);
                    }
                }))
            });
        section(
            Some("Permissions"),
            vec![
                row(text("Accessibility", theme), accessibility),
                note("Required: macOS only lets apps you trust move windows.", theme),
                row(text("Screen Recording", theme), screen),
                note("Optional: shows windows' real contents while they move. Nothing is recorded or kept. Reopen Canopy after allowing.", theme),
            ],
            theme,
        )
        .into_any_element()
    }

    fn general(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let features = MacFeature::ALL
            .into_iter()
            .map(|f| {
                let on = self.features.get(&f).copied().unwrap_or(false);
                let title = if on {
                    "Turn Off"
                } else if f.turned_off_by_canopy() {
                    "Turn Back On"
                } else {
                    "Turn On"
                };
                let view = cx.entity().downgrade();
                row(
                    titled(f.title(), f.detail(), theme),
                    button(f.title(), title, false, theme, move |_, cx| {
                        f.set(!on);
                        let _ = view.update(cx, |v, cx| v.refresh_status(cx));
                    }),
                )
            })
            .collect();
        let view = cx.entity().downgrade();
        let mut login = vec![toggle_row("login", "Open Canopy at login", self.login_enabled, theme, move |on, cx| {
            let result = login_item::set(on);
            let _ = view.update(cx, |v, cx| {
                v.login_error = result.err();
                v.refresh_status(cx);
            });
        })];
        if let Some(e) = &self.login_error {
            login.push(div().px(px(12.0)).py(px(6.0)).text_size(px(11.0)).text_color(theme.red).child(e.clone()).into_any_element());
        }
        vec![
            self.permissions(theme),
            section(Some("macOS features"), features, theme).into_any_element(),
            section(None, login, theme).into_any_element(),
        ]
    }

    fn pref_toggle(&self, id: &'static str, label: &str, on: bool, theme: &Theme, edit: fn(&mut Preferences, bool)) -> AnyElement {
        let coordinator = self.coordinator.clone();
        toggle_row(id, label, on, theme, move |value, cx| Self::change(&coordinator, cx, move |p| edit(p, value)))
    }

    fn pref_slider(&mut self, name: &'static str, value: f64, range: (f64, f64), step: Option<f64>, theme: &Theme, edit: fn(&mut Preferences, f64)) -> impl IntoElement {
        let coordinator = self.coordinator.clone();
        slider(value, range, step, self.slider_state(name), theme, move |v, cx| Self::change(&coordinator, cx, move |p| edit(p, v)))
    }

    fn windows(&mut self, p: &Preferences, theme: &Theme) -> Vec<AnyElement> {
        let reading = |s: String, width: f32| div().w(px(width)).text_size(px(12.0)).text_color(theme.secondary).child(s);
        let gap = self.pref_slider("gap", p.gap, (0.0, 40.0), Some(1.0), theme, |p, v| p.gap = v);
        let duration = self.pref_slider("duration", p.animation_duration, (0.15, 1.0), None, theme, |p, v| p.animation_duration = v);
        vec![
            section(
                Some("Layout"),
                vec![
                    row(
                        text("Gap", theme),
                        div().flex().items_center().gap(px(6.0)).child(gap).child(reading(format!("{} pt", p.gap as i64), 44.0)),
                    ),
                    self.pref_toggle("share", "Share a leaving window's space with the whole row", p.share_removed_space, theme, |p, v| p.share_removed_space = v),
                    note("So closing a window exactly undoes adding it. Otherwise the neighbour after it takes all its space.", theme),
                ],
                theme,
            )
            .into_any_element(),
            section(
                Some("Motion"),
                vec![
                    self.pref_toggle("animate", "Animate layout changes", p.animate, theme, |p, v| p.animate = v),
                    row(
                        text("Duration", theme),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(duration)
                            .child(reading(format!("{} ms", (p.animation_duration * 1000.0) as i64), 60.0)),
                    ),
                ],
                theme,
            )
            .into_any_element(),
            section(
                Some("Title bar"),
                vec![
                    self.pref_toggle("double", "Double-click frames the window on its own", p.title_double_click_frames, theme, |p, v| p.title_double_click_frames = v),
                    self.pref_toggle("green", "Green button docks and floats", p.green_button_toggles, theme, |p, v| p.green_button_toggles = v),
                    note("⌥-click still zooms, fn-click still goes full screen.", theme),
                ],
                theme,
            )
            .into_any_element(),
        ]
    }

    fn tabs(&mut self, p: &Preferences, theme: &Theme) -> Vec<AnyElement> {
        let hold = self.pref_slider("hold", p.tab_hold, (0.2, 1.5), None, theme, |p, v| p.tab_hold = v);
        vec![section(
            Some("Tab groups"),
            vec![
                note("Drop a window on the middle of another and pause to make them tabs of one window.", theme),
                row(
                    text("Hold over a target", theme),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(hold)
                        .child(div().w(px(44.0)).text_size(px(12.0)).text_color(theme.secondary).child(format!("{:.1} s", p.tab_hold))),
                ),
                self.pref_toggle("close", "Close buttons on tabs", p.tab_close_buttons, theme, |p, v| p.tab_close_buttons = v),
            ],
            theme,
        )
        .into_any_element()]
    }

    fn appearance(&mut self, p: &Preferences, theme: &Theme) -> Vec<AnyElement> {
        let tint = self.pref_slider("tint", p.cover_tint, (0.0, 1.0), None, theme, |p, v| p.cover_tint = v);
        vec![
            section(
                Some("Menu bar"),
                vec![self.pref_toggle("controls", "Show back, forward, everything and saved views", p.menu_bar_controls, theme, |p, v| p.menu_bar_controls = v)],
                theme,
            )
            .into_any_element(),
            section(
                Some("Previews"),
                vec![
                    self.pref_toggle("live", "Live previews (needs Screen Recording)", p.live_previews, theme, |p, v| p.live_previews = v),
                    row(text("App colour on covers", theme), tint),
                    note("Windows too small to use show their app's colour and icon until there's room. Double-click one to go in.", theme),
                ],
                theme,
            )
            .into_any_element(),
        ]
    }

    fn gestures(&self, p: &Preferences, theme: &Theme) -> Vec<AnyElement> {
        let coordinator = self.coordinator.clone();
        let choice = segmented(
            "step",
            &["The window you're using", "The pointer"],
            usize::from(p.step_toward_pointer),
            theme,
            move |i, cx| Self::change(&coordinator, cx, move |p| p.step_toward_pointer = i == 1),
        );
        vec![
            section(Some("Trackpad"), vec![self.pref_toggle("pinch", "⌥-pinch anywhere to zoom in and out", p.option_pinch, theme, |p, v| p.option_pinch = v)], theme).into_any_element(),
            section(Some("Mouse"), vec![self.pref_toggle("rect", "⌘⇧-drag a rectangle to zoom to what it frames", p.rect_zoom, theme, |p, v| p.rect_zoom = v)], theme).into_any_element(),
            section(Some("Stepping in"), vec![row(text("Stepping in goes toward", theme), choice)], theme).into_any_element(),
        ]
    }

    fn apps(&self, p: &Preferences, theme: &Theme) -> Vec<AnyElement> {
        let mut rows = vec![note("Windows of these apps are left alone: they float freely and are never arranged.", theme)];
        if p.ignored_bundle_ids.is_empty() {
            rows.push(div().px(px(12.0)).py(px(8.0)).text_size(px(13.0)).text_color(theme.secondary).child("No apps").into_any_element());
        }
        for id in &p.ignored_bundle_ids {
            let name = workspace::app_name(id).unwrap_or_else(|| id.clone());
            let (coordinator, bundle) = (self.coordinator.clone(), id.clone());
            rows.push(row(
                titled(name, id.clone(), theme),
                button(("remove", rows.len()), "Remove", false, theme, move |_, cx| {
                    let bundle = bundle.clone();
                    Self::change(&coordinator, cx, move |p| p.ignored_bundle_ids.retain(|b| *b != bundle));
                }),
            ));
        }
        let ignored = p.ignored_bundle_ids.clone();
        rows.push(row(
            div(),
            button("add-app", "Add a Running App", false, theme, move |_, _| {
                let own = std::process::id() as i32;
                let mut apps: Vec<(String, String)> = workspace::running_apps()
                    .into_iter()
                    .filter(|a| a.regular && a.pid != own)
                    .filter_map(|a| {
                        let bundle = a.bundle_id?;
                        (!ignored.contains(&bundle)).then(|| (a.name.unwrap_or_else(|| bundle.clone()), bundle))
                    })
                    .collect();
                apps.sort();
                show_apps_menu(apps);
            }),
        ));
        vec![section(None, rows, theme).into_any_element()]
    }

    fn shortcuts(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let bindings = self.coordinator.read(cx).bindings.clone();
        let rows = Shortcuts::bindable()
            .into_iter()
            .map(|c| {
                let label = if self.recording == Some(c) {
                    "Type shortcut…".to_string()
                } else {
                    bindings.get(&c).map_or("No shortcut".to_string(), hotkeys::display)
                };
                let view = cx.entity().downgrade();
                row(
                    text(c.title(), theme),
                    div().min_w(px(120.0)).flex().justify_end().child(button(c.url(), label, false, theme, move |_, cx| {
                        let _ = view.update(cx, |v, cx| v.start_recording(c, cx));
                    })),
                )
            })
            .collect();
        let view = cx.entity().downgrade();
        vec![
            section(None, rows, theme).into_any_element(),
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(caption("A shortcut needs ⌃, ⌥ or ⌘. Press ⌫ while recording to clear one.", theme))
                .child(button("restore", "Restore Defaults", false, theme, move |_, cx| {
                    let _ = view.update(cx, |v, cx| {
                        v.coordinator.update(cx, |c, _| c.set_bindings(Shortcuts::defaults()));
                        cx.notify();
                    });
                }))
                .into_any_element(),
        ]
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(window);
        let prefs = self.coordinator.read(cx).prefs.clone();
        let tab_bar = self.tab_bar(&theme, cx);
        let body = match self.pane {
            SettingsPane::General => self.general(&theme, cx),
            SettingsPane::Windows => self.windows(&prefs, &theme),
            SettingsPane::Tabs => self.tabs(&prefs, &theme),
            SettingsPane::Appearance => self.appearance(&prefs, &theme),
            SettingsPane::Gestures => self.gestures(&prefs, &theme),
            SettingsPane::Apps => self.apps(&prefs, &theme),
            SettingsPane::Shortcuts => self.shortcuts(&theme, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window)
            .text_color(theme.text)
            .child(tab_bar)
            .child(
                div()
                    .id("pane")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(16.0))
                    .child(div().flex().flex_col().gap(px(14.0)).children(body)),
            )
    }
}
