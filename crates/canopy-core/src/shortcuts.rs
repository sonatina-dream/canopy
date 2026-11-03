use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::commands::Command;
use crate::geometry::Direction;

/// A key with modifiers, stored with Carbon modifier bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCombo {
    pub key_code: u32,
    pub modifiers: u32,
}

impl KeyCombo {
    pub const COMMAND: u32 = 1 << 8;
    pub const SHIFT: u32 = 1 << 9;
    pub const OPTION: u32 = 1 << 11;
    pub const CONTROL: u32 = 1 << 12;

    pub const fn new(key_code: u32, modifiers: u32) -> Self {
        Self { key_code, modifiers }
    }

    /// A shortcut needs ⌃, ⌥ or ⌘.
    pub fn is_valid(&self) -> bool {
        self.modifiers & (Self::CONTROL | Self::OPTION | Self::COMMAND) != 0
    }

    /// `⌃⌥Space`; `key_name` is the key's label on the current keyboard layout.
    pub fn display(&self, key_name: &str) -> String {
        let mut s = String::new();
        if self.modifiers & Self::CONTROL != 0 {
            s.push('⌃');
        }
        if self.modifiers & Self::OPTION != 0 {
            s.push('⌥');
        }
        if self.modifiers & Self::SHIFT != 0 {
            s.push('⇧');
        }
        if self.modifiers & Self::COMMAND != 0 {
            s.push('⌘');
        }
        s + key_name
    }
}

/// macOS virtual key codes (`kVK_…`).
pub mod key {
    pub const SPACE: u32 = 49;
    pub const RETURN: u32 = 36;
    pub const ESCAPE: u32 = 53;
    pub const TAB: u32 = 48;
    pub const DELETE: u32 = 51;
    pub const LEFT_ARROW: u32 = 123;
    pub const RIGHT_ARROW: u32 = 124;
    pub const DOWN_ARROW: u32 = 125;
    pub const UP_ARROW: u32 = 126;
    pub const LEFT_BRACKET: u32 = 33;
    pub const RIGHT_BRACKET: u32 = 30;
    pub const D: u32 = 2;
    pub const N: u32 = 45;
    pub const DIGITS: [u32; 9] = [18, 19, 20, 21, 23, 22, 26, 28, 25];
}

/// Default bindings (SPEC §12), keyed by each command.
pub struct Shortcuts;

impl Shortcuts {
    pub fn bindable() -> Vec<Command> {
        let mut c = vec![
            Command::Everything,
            Command::StepOut,
            Command::StepIn,
            Command::Neighbour(Direction::Left),
            Command::Neighbour(Direction::Right),
            Command::Neighbour(Direction::Up),
            Command::Neighbour(Direction::Down),
            Command::Back,
            Command::Forward,
            Command::FrameWindow,
            Command::ToggleDesktop,
            Command::SplitFinder,
        ];
        c.extend((1..=9).map(Command::SaveView));
        c.extend((1..=9).map(Command::GoToView));
        c
    }

    pub fn defaults() -> HashMap<Command, KeyCombo> {
        let co = KeyCombo::CONTROL | KeyCombo::OPTION;
        let mut d = HashMap::from([
            (Command::Everything, KeyCombo::new(key::SPACE, co)),
            (Command::StepOut, KeyCombo::new(key::UP_ARROW, co)),
            (Command::StepIn, KeyCombo::new(key::DOWN_ARROW, co)),
            (Command::Neighbour(Direction::Left), KeyCombo::new(key::LEFT_ARROW, co)),
            (Command::Neighbour(Direction::Right), KeyCombo::new(key::RIGHT_ARROW, co)),
            (Command::Neighbour(Direction::Up), KeyCombo::new(key::UP_ARROW, co | KeyCombo::SHIFT)),
            (Command::Neighbour(Direction::Down), KeyCombo::new(key::DOWN_ARROW, co | KeyCombo::SHIFT)),
            (Command::Back, KeyCombo::new(key::LEFT_BRACKET, co)),
            (Command::Forward, KeyCombo::new(key::RIGHT_BRACKET, co)),
            (Command::FrameWindow, KeyCombo::new(key::RETURN, co)),
            (Command::ToggleDesktop, KeyCombo::new(key::D, co)),
            (Command::SplitFinder, KeyCombo::new(key::N, co)),
        ]);
        for (i, k) in key::DIGITS.into_iter().enumerate() {
            let n = i as i64 + 1;
            d.insert(Command::SaveView(n), KeyCombo::new(k, co | KeyCombo::SHIFT));
            d.insert(Command::GoToView(n), KeyCombo::new(k, co));
        }
        d
    }

    /// Bindings from their stored form: by command URL, with a command bound
    /// to nothing stored explicitly so it doesn't fall back to its default.
    pub fn from_stored(raw: HashMap<String, Option<KeyCombo>>) -> HashMap<Command, KeyCombo> {
        let mut b = Self::defaults();
        for (url, combo) in raw {
            let Some(c) = Command::from_url(&url) else { continue };
            match combo {
                Some(combo) => b.insert(c, combo),
                None => b.remove(&c),
            };
        }
        b
    }

    pub fn to_stored(bindings: &HashMap<Command, KeyCombo>) -> HashMap<String, Option<KeyCombo>> {
        Self::bindable().into_iter().map(|c| (c.url(), bindings.get(&c).copied())).collect()
    }
}
