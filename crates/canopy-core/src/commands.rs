use serde::{Deserialize, Serialize};

use crate::geometry::Direction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsPane {
    General,
    Windows,
    Tabs,
    Appearance,
    Gestures,
    Apps,
    Shortcuts,
}

impl SettingsPane {
    pub const ALL: [SettingsPane; 7] = [
        SettingsPane::General,
        SettingsPane::Windows,
        SettingsPane::Tabs,
        SettingsPane::Appearance,
        SettingsPane::Gestures,
        SettingsPane::Apps,
        SettingsPane::Shortcuts,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SettingsPane::General => "general",
            SettingsPane::Windows => "windows",
            SettingsPane::Tabs => "tabs",
            SettingsPane::Appearance => "appearance",
            SettingsPane::Gestures => "gestures",
            SettingsPane::Apps => "apps",
            SettingsPane::Shortcuts => "shortcuts",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            SettingsPane::General => "General",
            SettingsPane::Windows => "Windows",
            SettingsPane::Tabs => "Tabs",
            SettingsPane::Appearance => "Appearance",
            SettingsPane::Gestures => "Gestures",
            SettingsPane::Apps => "Apps",
            SettingsPane::Shortcuts => "Shortcuts",
        }
    }

    fn parse(s: &str) -> Option<SettingsPane> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// Every command Canopy can run, from a shortcut, the menu bar, or a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Everything,
    StepOut,
    StepIn,
    Back,
    Forward,
    Neighbour(Direction),
    FrameWindow,
    FrameDesktop,
    ToggleDesktop,
    GoToView(i64),
    SaveView(i64),
    SplitFinder,
    Arrange,
    FloatAll,
    Settings(Option<SettingsPane>),
}

impl Command {
    pub const SCHEME: &'static str = "canopy";

    /// Parses `canopy://<command>[?args]`. Returns `None` for anything invalid.
    pub fn from_url(url: &str) -> Option<Command> {
        let parts = UrlParts::parse(url)?;
        if parts.scheme.to_ascii_lowercase() != Self::SCHEME {
            return None;
        }
        let mut name = parts.host.to_lowercase();
        if name.is_empty() {
            // canopy:everything or canopy:///everything
            name = parts.path.trim_matches('/').to_lowercase();
        }
        let slot = || {
            let n: i64 = parts.query("n")?.parse().ok()?;
            (1..=9).contains(&n).then_some(n)
        };
        Some(match name.as_str() {
            "everything" => Command::Everything,
            "zoom-out" => Command::StepOut,
            "zoom-in" => Command::StepIn,
            "back" => Command::Back,
            "forward" => Command::Forward,
            "neighbour" | "neighbor" => Command::Neighbour(Direction::parse(&parts.query("dir")?.to_lowercase())?),
            "frame-window" => Command::FrameWindow,
            "frame-desktop" => Command::FrameDesktop,
            "toggle-desktop" => Command::ToggleDesktop,
            "view" => Command::GoToView(slot()?),
            "save-view" => Command::SaveView(slot()?),
            "split-finder" => Command::SplitFinder,
            "arrange" => Command::Arrange,
            "float-all" => Command::FloatAll,
            "settings" => Command::Settings(parts.query("pane").and_then(|p| SettingsPane::parse(&p.to_lowercase()))),
            _ => return None,
        })
    }

    /// The canonical URL; `Command::from_url(&c.url()) == Some(c)`.
    pub fn url(self) -> String {
        let path = match self {
            Command::Everything => "everything".to_string(),
            Command::StepOut => "zoom-out".to_string(),
            Command::StepIn => "zoom-in".to_string(),
            Command::Back => "back".to_string(),
            Command::Forward => "forward".to_string(),
            Command::Neighbour(d) => format!("neighbour?dir={}", d.name()),
            Command::FrameWindow => "frame-window".to_string(),
            Command::FrameDesktop => "frame-desktop".to_string(),
            Command::ToggleDesktop => "toggle-desktop".to_string(),
            Command::GoToView(n) => format!("view?n={n}"),
            Command::SaveView(n) => format!("save-view?n={n}"),
            Command::SplitFinder => "split-finder".to_string(),
            Command::Arrange => "arrange".to_string(),
            Command::FloatAll => "float-all".to_string(),
            Command::Settings(Some(pane)) => format!("settings?pane={}", pane.name()),
            Command::Settings(None) => "settings".to_string(),
        };
        format!("{}://{}", Self::SCHEME, path)
    }

    pub fn title(self) -> String {
        match self {
            Command::Everything => "Show Everything".into(),
            Command::StepOut => "Step Out a Level".into(),
            Command::StepIn => "Step In a Level".into(),
            Command::Back => "Back".into(),
            Command::Forward => "Forward".into(),
            Command::Neighbour(Direction::Left) => "Neighbouring Space to the Left".into(),
            Command::Neighbour(Direction::Right) => "Neighbouring Space to the Right".into(),
            Command::Neighbour(Direction::Up) => "Neighbouring Space Above".into(),
            Command::Neighbour(Direction::Down) => "Neighbouring Space Below".into(),
            Command::FrameWindow => "Focused Window on Its Own".into(),
            Command::FrameDesktop => "Desktop on Its Own".into(),
            Command::ToggleDesktop => "Hide or Show the Desktop".into(),
            Command::GoToView(n) => format!("Go to View {n}"),
            Command::SaveView(n) => format!("Save View {n}"),
            Command::SplitFinder => "Split with a New Finder Window".into(),
            Command::Arrange => "Arrange Windows".into(),
            Command::FloatAll => "Float All Windows on Desktop".into(),
            Command::Settings(_) => "Settings…".into(),
        }
    }
}

/// The pieces of a URL the commands use.
struct UrlParts<'a> {
    scheme: &'a str,
    host: &'a str,
    path: &'a str,
    query: &'a str,
}

impl<'a> UrlParts<'a> {
    fn parse(url: &'a str) -> Option<Self> {
        let (scheme, rest) = url.split_once(':')?;
        if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
            return None;
        }
        let rest = rest.split('#').next().unwrap_or("");
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (host, path) = match rest.strip_prefix("//") {
            Some(authority) => authority.split_at(authority.find('/').unwrap_or(authority.len())),
            None => ("", rest),
        };
        Some(Self { scheme, host, path, query })
    }

    /// The first value of `key`, percent-decoded. Names match in any case.
    fn query(&self, key: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(name).to_lowercase() == key).then(|| percent_decode(value))
        })
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = if bytes[i] == b'%' { s.get(i + 1..i + 3) } else { None };
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
