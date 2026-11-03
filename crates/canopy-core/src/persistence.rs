use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::arrangement::{Arrangement, WindowId};
use crate::navigator::Navigator;

/// What identifies a window across restarts when its id has changed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowRef {
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    pub title: String,
}

impl WindowRef {
    pub fn new(bundle_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self { bundle_id: bundle_id.into(), title: title.into() }
    }
}

/// One display's saved arrangement, camera and the windows it referred to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayState {
    pub arrangement: Arrangement,
    pub navigator: Navigator,
    /// Keyed by the window id as text, so the JSON stays an object.
    pub windows: BTreeMap<String, WindowRef>,
}

impl DisplayState {
    pub fn new(arrangement: Arrangement, navigator: Navigator, windows: HashMap<WindowId, WindowRef>) -> Self {
        let windows = windows.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        Self { arrangement, navigator, windows }
    }

    pub fn window_refs(&self) -> HashMap<WindowId, WindowRef> {
        self.windows.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()))).collect()
    }

    /// Re-attaches the saved layout to the windows that exist now. Windows that
    /// cannot be matched leave the layout and their space is shared.
    pub fn reattach(&mut self, live: &HashMap<WindowId, WindowRef>, share_space: bool) {
        let mapping = WindowMatcher::match_windows(&self.window_refs(), live);
        let keep: HashSet<WindowId> = mapping.keys().copied().collect();
        self.arrangement.retain_only(&keep, share_space);
        self.arrangement.remap(&mapping);
        self.navigator.remap(&mapping);
        self.navigator.prune(&self.arrangement.windows().into_iter().collect());
        self.windows = self
            .arrangement
            .windows()
            .into_iter()
            .filter_map(|w| Some((w.to_string(), live.get(&w)?.clone())))
            .collect();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub version: i64,
    /// Keyed by display UUID and Space.
    pub displays: BTreeMap<String, DisplayState>,
}

impl SavedState {
    pub const CURRENT_VERSION: i64 = 1;

    pub fn new(displays: BTreeMap<String, DisplayState>) -> Self {
        Self { version: Self::CURRENT_VERSION, displays }
    }

    pub fn encoded(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }

    pub fn decode(data: &[u8]) -> serde_json::Result<SavedState> {
        serde_json::from_slice(data)
    }
}

pub struct WindowMatcher;

impl WindowMatcher {
    /// Maps saved window ids to live ones:
    /// 1. same id and same app;
    /// 2. same app and same title;
    /// 3. the only unmatched saved window of an app to its only unmatched live window.
    pub fn match_windows(saved: &HashMap<WindowId, WindowRef>, live: &HashMap<WindowId, WindowRef>) -> HashMap<WindowId, WindowId> {
        let mut mapping = HashMap::new();
        let mut used_live = HashSet::new();
        let mut saved_ids: Vec<WindowId> = saved.keys().copied().collect();
        saved_ids.sort_unstable();
        let mut live_ids: Vec<WindowId> = live.keys().copied().collect();
        live_ids.sort_unstable();

        for &id in &saved_ids {
            if live.get(&id).is_some_and(|l| l.bundle_id == saved[&id].bundle_id) {
                mapping.insert(id, id);
                used_live.insert(id);
            }
        }
        for &id in &saved_ids {
            if mapping.contains_key(&id) {
                continue;
            }
            if let Some(&l) = live_ids.iter().find(|&&l| !used_live.contains(&l) && live[&l] == saved[&id]) {
                mapping.insert(id, l);
                used_live.insert(l);
            }
        }
        let mut saved_by_app: HashMap<&str, Vec<WindowId>> = HashMap::new();
        for &id in saved_ids.iter().filter(|id| !mapping.contains_key(id)) {
            saved_by_app.entry(&saved[&id].bundle_id).or_default().push(id);
        }
        let mut live_by_app: HashMap<&str, Vec<WindowId>> = HashMap::new();
        for &id in live_ids.iter().filter(|id| !used_live.contains(id)) {
            live_by_app.entry(&live[&id].bundle_id).or_default().push(id);
        }
        for (app, ids) in saved_by_app {
            if let ([only], Some([candidate])) = (ids.as_slice(), live_by_app.get(app).map(Vec::as_slice)) {
                mapping.insert(*only, *candidate);
            }
        }
        mapping
    }
}

/// User preferences. Missing keys decode to their defaults.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub version: i64,
    pub gap: f64,
    pub animate: bool,
    pub animation_duration: f64,
    pub redraw_wait: f64,
    pub share_removed_space: bool,
    pub tab_hold: f64,
    #[serde(rename = "ignoredBundleIDs")]
    pub ignored_bundle_ids: Vec<String>,
    pub menu_bar_controls: bool,
    pub tab_close_buttons: bool,
    pub option_pinch: bool,
    pub rect_zoom: bool,
    pub title_double_click_frames: bool,
    pub green_button_toggles: bool,
    pub step_toward_pointer: bool,
    pub cover_tint: f64,
    pub live_previews: bool,
}

impl Preferences {
    pub const CURRENT_VERSION: i64 = 2;
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            gap: 12.0,
            animate: true,
            animation_duration: 0.46,
            redraw_wait: 0.15,
            share_removed_space: true,
            tab_hold: 0.3,
            ignored_bundle_ids: Vec::new(),
            menu_bar_controls: true,
            tab_close_buttons: true,
            option_pinch: true,
            rect_zoom: true,
            title_double_click_frames: true,
            green_button_toggles: true,
            step_toward_pointer: false,
            cover_tint: 0.35,
            live_previews: true,
        }
    }
}

impl<'de> Deserialize<'de> for Preferences {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Stored {
            version: Option<i64>,
            gap: Option<f64>,
            animate: Option<bool>,
            animation_duration: Option<f64>,
            redraw_wait: Option<f64>,
            share_removed_space: Option<bool>,
            tab_hold: Option<f64>,
            #[serde(rename = "ignoredBundleIDs")]
            ignored_bundle_ids: Option<Vec<String>>,
            menu_bar_controls: Option<bool>,
            tab_close_buttons: Option<bool>,
            option_pinch: Option<bool>,
            rect_zoom: Option<bool>,
            title_double_click_frames: Option<bool>,
            green_button_toggles: Option<bool>,
            step_toward_pointer: Option<bool>,
            cover_tint: Option<f64>,
            live_previews: Option<bool>,
        }

        let s = Stored::deserialize(deserializer)?;
        let d = Preferences::default();
        let mut p = Preferences {
            version: Preferences::CURRENT_VERSION,
            gap: s.gap.unwrap_or(d.gap),
            animate: s.animate.unwrap_or(d.animate),
            animation_duration: s.animation_duration.unwrap_or(d.animation_duration),
            redraw_wait: s.redraw_wait.unwrap_or(d.redraw_wait),
            share_removed_space: s.share_removed_space.unwrap_or(d.share_removed_space),
            tab_hold: s.tab_hold.unwrap_or(d.tab_hold),
            ignored_bundle_ids: s.ignored_bundle_ids.unwrap_or(d.ignored_bundle_ids),
            menu_bar_controls: s.menu_bar_controls.unwrap_or(d.menu_bar_controls),
            tab_close_buttons: s.tab_close_buttons.unwrap_or(d.tab_close_buttons),
            option_pinch: s.option_pinch.unwrap_or(d.option_pinch),
            rect_zoom: s.rect_zoom.unwrap_or(d.rect_zoom),
            title_double_click_frames: s.title_double_click_frames.unwrap_or(d.title_double_click_frames),
            green_button_toggles: s.green_button_toggles.unwrap_or(d.green_button_toggles),
            step_toward_pointer: s.step_toward_pointer.unwrap_or(d.step_toward_pointer),
            cover_tint: s.cover_tint.unwrap_or(d.cover_tint),
            live_previews: s.live_previews.unwrap_or(d.live_previews),
        };
        // Version 1 waited half a second over a window before offering a tab.
        if s.version.unwrap_or(1) < 2 && p.tab_hold == 0.5 {
            p.tab_hold = d.tab_hold;
        }
        Ok(p)
    }
}
