//! Canopy's menu bar controls: back, forward, everything, and one button per
//! saved view, plus the Canopy icon and its menu. Right-click any of them for
//! the menu. GPUI has no status items or native menus, so these are AppKit's.
//!
//! Menus are built while AppKit is tracking them, so they read a snapshot the
//! coordinator keeps up to date, and report back through the bus.

use std::cell::RefCell;
use std::collections::HashMap;

use canopy_core::{Command, KeyCombo, WindowId};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventMask, NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSStatusBarButton, NSStatusItem,
};
use objc2_foundation::{NSPoint, NSString};

use super::displays::mouse;
use super::hotkeys::key_name;
use super::panel::main_thread;
use super::workspace;
use crate::bus::{self, Msg};
use canopy_core::shortcuts::key;

const SETTINGS_ACCESSIBILITY: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

define_class!(
    /// A menu item that runs a closure.
    #[unsafe(super(NSMenuItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "CanopyClosureMenuItem"]
    #[ivars = Box<dyn Fn()>]
    struct ClosureMenuItem;

    impl ClosureMenuItem {
        #[unsafe(method(fire:))]
        fn fire(&self, _sender: Option<&AnyObject>) {
            (self.ivars())();
        }
    }
);

impl ClosureMenuItem {
    fn new(title: &str, handler: impl Fn() + 'static) -> Retained<Self> {
        let this: Allocated<Self> = Self::alloc(main_thread());
        let this = this.set_ivars(Box::new(handler) as Box<dyn Fn()>);
        let empty = NSString::new();
        // SAFETY: NSMenuItem's designated initialiser, with valid arguments.
        let this: Retained<Self> = unsafe {
            msg_send![super(this), initWithTitle: &*NSString::from_str(title), action: Some(sel!(fire:)), keyEquivalent: &*empty]
        };
        // SAFETY: the item is its own target, so the target outlives every click.
        unsafe { this.setTarget(Some(&this)) };
        this
    }

    fn set_key(&self, key: &str, modifiers: NSEventModifierFlags) {
        self.setKeyEquivalent(&NSString::from_str(key));
        self.setKeyEquivalentModifierMask(modifiers);
    }
}

/// What the menus show, as last published by the coordinator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuState {
    pub started: bool,
    pub screen_recording: bool,
    pub controls: bool,
    pub saved_views: Vec<i64>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub desktop_hidden: bool,
    pub ignored: Vec<String>,
    pub bindings: HashMap<Command, KeyCombo>,
}

pub struct Ivars {
    icon: Retained<NSStatusItem>,
    controls: RefCell<Vec<Retained<NSStatusItem>>>,
    /// The command of each control, by the tag on its button.
    commands: RefCell<Vec<Command>>,
    state: RefCell<MenuState>,
    shown: RefCell<String>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CanopyMenuBar"]
    #[ivars = Ivars]
    pub struct MenuBar;

    unsafe impl NSObjectProtocol for MenuBar {}

    unsafe impl NSMenuDelegate for MenuBar {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.build(menu);
        }
    }

    impl MenuBar {
        #[unsafe(method(controlClicked:))]
        fn control_clicked(&self, sender: &NSStatusBarButton) {
            let app = NSApplication::sharedApplication(main_thread());
            if app.currentEvent().is_some_and(|e| e.r#type() == NSEventType::RightMouseUp) {
                if let Some(b) = self.ivars().icon.button(main_thread()) {
                    // SAFETY: a nil sender is allowed.
                    unsafe { b.performClick(None) };
                }
                return;
            }
            if let Some(&c) = self.ivars().commands.borrow().get(sender.tag() as usize) {
                bus::send(Msg::Run(c));
            }
        }
    }
);

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )?;
    image.setTemplate(true);
    Some(image)
}

fn separator() -> Retained<NSMenuItem> {
    NSMenuItem::separatorItem(main_thread())
}

fn plain_item(title: &str) -> Retained<NSMenuItem> {
    let empty = NSString::new();
    // SAFETY: NSMenuItem's designated initialiser, with valid arguments.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(main_thread()), &NSString::from_str(title), None, &empty)
    }
}

/// For showing a shortcut beside a menu item.
fn key_equivalent(c: &KeyCombo) -> Option<(String, NSEventModifierFlags)> {
    let mut mods = NSEventModifierFlags::empty();
    if c.modifiers & KeyCombo::CONTROL != 0 {
        mods |= NSEventModifierFlags::Control;
    }
    if c.modifiers & KeyCombo::OPTION != 0 {
        mods |= NSEventModifierFlags::Option;
    }
    if c.modifiers & KeyCombo::SHIFT != 0 {
        mods |= NSEventModifierFlags::Shift;
    }
    if c.modifiers & KeyCombo::COMMAND != 0 {
        mods |= NSEventModifierFlags::Command;
    }
    let function_key = |code: u32| char::from_u32(code).map(String::from);
    let k = match c.key_code {
        key::LEFT_ARROW => function_key(0xF702)?,
        key::RIGHT_ARROW => function_key(0xF703)?,
        key::DOWN_ARROW => function_key(0xF701)?,
        key::UP_ARROW => function_key(0xF700)?,
        key::SPACE => " ".to_string(),
        key::RETURN => "\r".to_string(),
        code => {
            let name = key_name(code);
            if name.chars().count() != 1 {
                return None;
            }
            name.to_lowercase()
        }
    };
    Some((k, mods))
}

impl MenuBar {
    pub fn new() -> Retained<MenuBar> {
        let mtm = main_thread();
        let icon = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0);
        if let Some(b) = icon.button(mtm) {
            b.setImage(symbol("square.split.2x2", "Canopy").as_deref());
        }
        let this = Self::alloc(mtm).set_ivars(Ivars {
            icon,
            controls: RefCell::default(),
            commands: RefCell::default(),
            state: RefCell::default(),
            shown: RefCell::default(),
        });
        // SAFETY: NSObject's initialiser.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let menu = NSMenu::new(mtm);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        this.ivars().icon.setMenu(Some(&menu));
        this
    }

    /// Takes the latest state, and rebuilds the controls when what they show has changed.
    pub fn refresh(&self, state: MenuState) {
        let show = state.controls && state.started;
        let signature = format!("{show}|{:?}|{}|{}", state.saved_views, state.can_go_back, state.can_go_forward);
        *self.ivars().state.borrow_mut() = state.clone();
        if *self.ivars().shown.borrow() == signature {
            return;
        }
        *self.ivars().shown.borrow_mut() = signature;
        let bar = NSStatusBar::systemStatusBar();
        for c in self.ivars().controls.borrow_mut().drain(..) {
            bar.removeStatusItem(&c);
        }
        self.ivars().commands.borrow_mut().clear();
        if !show {
            return;
        }
        // Status items are added right to left.
        for &n in state.saved_views.iter().rev() {
            self.add_control(Some(&n.to_string()), None, Command::GoToView(n), true);
        }
        self.add_control(None, Some("rectangle.3.group"), Command::Everything, true);
        self.add_control(None, Some("chevron.right"), Command::Forward, state.can_go_forward);
        self.add_control(None, Some("chevron.left"), Command::Back, state.can_go_back);
    }

    /// Takes everything off the menu bar.
    fn remove(&self) {
        let bar = NSStatusBar::systemStatusBar();
        for c in self.ivars().controls.borrow_mut().drain(..) {
            bar.removeStatusItem(&c);
        }
        bar.removeStatusItem(&self.ivars().icon);
    }

    fn add_control(&self, title: Option<&str>, symbol_name: Option<&str>, command: Command, enabled: bool) {
        let mtm = main_thread();
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
        if let Some(b) = item.button(mtm) {
            if let Some(name) = symbol_name {
                b.setImage(symbol(name, &command.title()).as_deref());
            }
            if let Some(title) = title {
                b.setTitle(&NSString::from_str(title));
            }
            b.setToolTip(Some(&NSString::from_str(&command.title())));
            b.setAppearsDisabled(!enabled);
            let mut commands = self.ivars().commands.borrow_mut();
            b.setTag(commands.len() as isize);
            commands.push(command);
            // SAFETY: the menu bar outlives its controls.
            unsafe {
                b.setTarget(Some(self));
                b.setAction(Some(sel!(controlClicked:)));
            }
            b.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
        self.ivars().controls.borrow_mut().push(item);
    }

    fn item(&self, command: Command, title: Option<&str>, enabled: bool) -> Retained<NSMenuItem> {
        let text = title.map_or_else(|| command.title(), str::to_string);
        let item = ClosureMenuItem::new(&text, move || bus::send(Msg::Run(command)));
        if let Some((key, mods)) = self.ivars().state.borrow().bindings.get(&command).and_then(key_equivalent) {
            item.set_key(&key, mods);
        }
        if !enabled {
            // SAFETY: an item without an action just shows as disabled.
            unsafe { item.setAction(None) };
        }
        Retained::into_super(item)
    }

    fn quit_item() -> Retained<NSMenuItem> {
        let quit = ClosureMenuItem::new("Quit Canopy", || bus::send(Msg::Quit));
        quit.set_key("q", NSEventModifierFlags::Command);
        Retained::into_super(quit)
    }

    fn build(&self, menu: &NSMenu) {
        menu.removeAllItems();
        let state = self.ivars().state.borrow().clone();
        if !state.started {
            menu.addItem(&plain_item("Canopy needs Accessibility permission"));
            let open = ClosureMenuItem::new("Open Accessibility Settings…", || workspace::open_url(SETTINGS_ACCESSIBILITY));
            menu.addItem(&open);
            menu.addItem(&separator());
            menu.addItem(&Self::quit_item());
            return;
        }
        if !state.screen_recording {
            menu.addItem(&ClosureMenuItem::new("Allow Screen Recording…", || bus::send(Msg::AskScreenRecording)));
            menu.addItem(&separator());
        }
        menu.addItem(&self.item(Command::Everything, None, true));
        menu.addItem(&self.item(Command::Back, None, state.can_go_back));
        menu.addItem(&self.item(Command::Forward, None, state.can_go_forward));

        let mtm = main_thread();
        let views_item = plain_item("Saved Views");
        let views = NSMenu::new(mtm);
        for &n in &state.saved_views {
            views.addItem(&self.item(Command::GoToView(n), None, true));
        }
        if !state.saved_views.is_empty() {
            views.addItem(&separator());
        }
        let save_item = plain_item("Save This View");
        let save_menu = NSMenu::new(mtm);
        for n in 1..=9 {
            save_menu.addItem(&self.item(Command::SaveView(n), Some(&format!("As {n}")), true));
        }
        save_item.setSubmenu(Some(&save_menu));
        views.addItem(&save_item);
        for &n in &state.saved_views {
            views.addItem(&ClosureMenuItem::new(&format!("Forget View {n}"), move || bus::send(Msg::ForgetView(n))));
        }
        views_item.setSubmenu(Some(&views));
        menu.addItem(&views_item);

        let title = if state.desktop_hidden { "Show Desktop in This Space" } else { "Hide Desktop in This Space" };
        menu.addItem(&self.item(Command::ToggleDesktop, Some(title), true));
        menu.addItem(&separator());
        menu.addItem(&self.item(Command::Arrange, None, true));
        menu.addItem(&self.item(Command::FloatAll, None, true));
        if let Some(front) = workspace::frontmost().filter(|a| a.pid != std::process::id() as i32) {
            if let Some(bundle) = front.bundle_id.clone() {
                let name = front.name.unwrap_or_else(|| bundle.clone());
                let ignored = state.ignored.contains(&bundle);
                let title = if ignored { format!("Arrange “{name}” Again") } else { format!("Don’t Arrange “{name}”") };
                menu.addItem(&ClosureMenuItem::new(&title, move || bus::send(Msg::SetIgnored(bundle.clone(), !ignored))));
            }
        }
        menu.addItem(&separator());
        let settings = ClosureMenuItem::new("Settings…", || bus::send(Msg::Run(Command::Settings(None))));
        settings.set_key(",", NSEventModifierFlags::Command);
        menu.addItem(&settings);
        menu.addItem(&Self::quit_item());
    }
}

/// Builds the menu as AppKit would and reads it back (for the smoke test).
pub fn menu_titles(state: MenuState) -> Vec<String> {
    let bar = MenuBar::new();
    bar.refresh(state);
    let menu = NSMenu::new(main_thread());
    bar.build(&menu);
    let titles = menu.itemArray().iter().map(|i| i.title().to_string()).filter(|t| !t.is_empty()).collect();
    bar.remove();
    titles
}

/// Runs a context menu under the pointer. Starts a modal loop, so it must be
/// called from `workspace::on_main`.
fn pop_up(menu: &NSMenu) {
    let p = mouse();
    let location = NSPoint::new(p.x, super::displays::primary_height() - p.y);
    menu.popUpMenuPositioningItem_atLocation_inView(None, location, None);
}

/// The right-click menu of a tab.
pub fn show_tab_menu(id: WindowId) {
    workspace::on_main(move || {
        let menu = NSMenu::new(main_thread());
        menu.addItem(&ClosureMenuItem::new("Close Tab", move || bus::send(Msg::CloseWindow(id))));
        menu.addItem(&ClosureMenuItem::new("Move Tab to New Window", move || bus::send(Msg::FloatWindow(id))));
        menu.addItem(&ClosureMenuItem::new("Minimize Tab", move || bus::send(Msg::MinimizeWindow(id))));
        pop_up(&menu);
    });
}

/// A menu of running apps, for adding one to the excluded list.
pub fn show_apps_menu(apps: Vec<(String, String)>) {
    workspace::on_main(move || {
        let menu = NSMenu::new(main_thread());
        for (name, bundle) in apps {
            menu.addItem(&ClosureMenuItem::new(&name, move || bus::send(Msg::IgnoreApp(bundle.clone()))));
        }
        pop_up(&menu);
    });
}
