//! Watches every regular app's windows. AX observers live on their own thread
//! and every AX read runs on the app's queue; only results reach the main
//! thread, as messages, so a slow app never stalls Canopy.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use canopy_core::{Rect, Size, WindowId, WindowRef};
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

use super::ax::{self, attr, AxWindow, Element, Observer};
use super::ax_service;
use super::run_loop::{self, RunLoop};
use super::workspace::{self, RunningApp};
use crate::bus::{self, Msg};

/// A window Canopy knows about. Main thread only.
pub struct TrackedWindow {
    pub ax: Arc<AxWindow>,
    pub bundle_id: String,
    pub app_name: String,
    pub title: String,
    pub minimum_size: Option<Size>,
    pub minimized: bool,
    pub full_screen: bool,
    pub hidden: bool,
    /// Last known frame: from the window server, or what Canopy last wrote.
    pub frame: Option<Rect>,
    /// The green button, relative to the window's top-left corner. Found ahead
    /// of time, so that a click can be matched to it without asking the app.
    pub green_button: Option<Rect>,
}

impl TrackedWindow {
    pub fn id(&self) -> WindowId {
        WindowId::from(self.ax.id)
    }

    pub fn pid(&self) -> i32 {
        self.ax.pid
    }

    pub fn window_ref(&self) -> WindowRef {
        WindowRef::new(self.bundle_id.clone(), self.title.clone())
    }
}

/// What one AX read of a window found. Built on the app's queue.
#[derive(Clone)]
pub struct Probe {
    element: Element,
    id: WindowId,
    role: Option<String>,
    subrole: Option<String>,
    title: String,
    minimized: bool,
    full_screen: bool,
    frame: Option<Rect>,
}

impl Probe {
    fn new(el: &Element) -> Option<Self> {
        let id = ax::window_id(el)?;
        let frame = match (ax::point(el, attr::POSITION), ax::size(el, attr::SIZE)) {
            (Some(p), Some(s)) => Some(Rect::new(p.x, p.y, s.width, s.height)),
            _ => None,
        };
        Some(Self {
            element: el.clone(),
            id: WindowId::from(id),
            role: ax::string(el, attr::ROLE),
            subrole: ax::string(el, attr::SUBROLE),
            title: ax::string(el, attr::TITLE).unwrap_or_default(),
            minimized: ax::boolean(el, attr::MINIMIZED).unwrap_or(false),
            full_screen: ax::boolean(el, attr::FULL_SCREEN).unwrap_or(false),
            frame,
        })
    }

    fn is_standard(&self) -> bool {
        self.role.as_deref() == Some(ax::WINDOW_ROLE) && self.subrole.as_deref() == Some(ax::STANDARD_WINDOW_SUBROLE)
    }
}

/// What reaches the main thread from the observer thread and the app queues.
pub enum TrackerMsg {
    AppLaunched(i32),
    AppTerminated(i32),
    AppHidden(i32, bool),
    RefreshFocus(i32),
    Scanned { pid: i32, probes: Vec<Probe>, gone: Vec<WindowId> },
    Consider { pid: i32, element: Element, attempt: u32 },
    Considered { pid: i32, element: Element, probe: Option<Box<Probe>>, attempt: u32 },
    Destroyed(WindowId),
    Minimized(WindowId, bool),
    MovedOrResized(WindowId, bool),
    TitleChanged(WindowId, String),
    Focus(i32, Option<WindowId>),
    GreenButton(WindowId, Option<Rect>),
}

/// What the tracker tells the coordinator.
pub enum TrackerEvent {
    Appeared(WindowId),
    Disappeared(WindowId),
    Minimized(WindowId, bool),
    FullScreen(WindowId, bool),
    AppHidden(i32, bool),
    MovedOrResized(WindowId),
    TitleChanged(WindowId),
    FocusChanged(Option<WindowId>),
    GreenButtonFound,
}

pub struct WindowTracker {
    pub windows: HashMap<WindowId, TrackedWindow>,
    /// The focused window of the frontmost app, as last reported.
    focused: Option<WindowId>,
    apps: HashMap<i32, Arc<AppWatch>>,
    own_pid: i32,
    observer_thread: OnceLock<RunLoop>,
}

impl WindowTracker {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            focused: None,
            apps: HashMap::new(),
            own_pid: std::process::id() as i32,
            observer_thread: OnceLock::new(),
        }
    }

    pub fn start(&mut self) -> Vec<TrackerEvent> {
        let (run_loop, ()) = run_loop::start("canopy.ax-observers", |_| ());
        let _ = self.observer_thread.set(run_loop);
        for app in workspace::running_apps() {
            self.watch(&app);
        }
        if let Some(front) = workspace::frontmost() {
            self.refresh_focus(front.pid);
        }
        Vec::new()
    }

    fn watch(&mut self, app: &RunningApp) {
        let Some(&run_loop) = self.observer_thread.get() else { return };
        if !app.regular || app.pid == self.own_pid || self.apps.contains_key(&app.pid) {
            return;
        }
        let watch = AppWatch::new(app);
        self.apps.insert(app.pid, watch.clone());
        AppWatch::start(&watch, run_loop);
        self.scan(app.pid);
        // Some apps only answer once they have finished launching.
        if !app.finished_launching {
            bus::send_after(Duration::from_millis(1500), Msg::Tracker(TrackerMsg::AppLaunched(-app.pid)));
        }
    }

    fn unwatch(&mut self, pid: i32) -> Vec<TrackerEvent> {
        let Some(watch) = self.apps.remove(&pid) else { return Vec::new() };
        watch.stop();
        ax_service::forget(pid);
        let gone: Vec<WindowId> = self.windows.values().filter(|w| w.pid() == pid).map(|w| w.id()).collect();
        gone.into_iter()
            .map(|id| {
                self.windows.remove(&id);
                TrackerEvent::Disappeared(id)
            })
            .collect()
    }

    /// Finds an app's new windows and drops ones that have gone, off the main thread.
    pub fn scan(&self, pid: i32) {
        let Some(watch) = self.apps.get(&pid).cloned() else { return };
        let known: HashSet<WindowId> = self.windows.values().filter(|w| w.pid() == pid).map(|w| w.id()).collect();
        ax_service::run(pid, move || {
            let probes: Vec<Probe> = ax::elements(&watch.element, attr::WINDOWS).iter().filter_map(Probe::new).collect();
            let seen: HashSet<WindowId> = probes.iter().map(|p| p.id).collect();
            // AX lists only the current Space's windows; keep others that still answer.
            let gone = known
                .difference(&seen)
                .copied()
                .filter(|id| match watch.element_for(*id) {
                    Some(el) => ax::string(&el, attr::ROLE).is_none(),
                    None => true,
                })
                .collect();
            bus::send(Msg::Tracker(TrackerMsg::Scanned { pid, probes, gone }));
        });
    }

    pub fn rescan_all(&self) {
        for &pid in self.apps.keys() {
            self.scan(pid);
        }
    }

    pub fn tracked_pids(&self) -> HashSet<i32> {
        self.windows.values().map(|w| w.pid()).collect()
    }

    fn adopt(&mut self, p: &Probe, watch: &Arc<AppWatch>) -> Option<TrackerEvent> {
        if !p.is_standard() || self.windows.contains_key(&p.id) {
            return None;
        }
        let ax = Arc::new(AxWindow {
            element: p.element.clone(),
            app: watch.element.clone(),
            pid: watch.pid,
            id: p.id as u32,
        });
        let hidden = workspace::app(watch.pid).is_some_and(|a| a.hidden);
        self.windows.insert(
            p.id,
            TrackedWindow {
                ax,
                bundle_id: watch.bundle_id.clone(),
                app_name: watch.name.clone(),
                title: p.title.clone(),
                minimum_size: None,
                minimized: p.minimized,
                full_screen: p.full_screen,
                hidden,
                frame: p.frame,
                green_button: None,
            },
        );
        AppWatch::observe_window(watch, &p.element, p.id);
        self.find_green_button(p.id);
        Some(TrackerEvent::Appeared(p.id))
    }

    fn find_green_button(&self, id: WindowId) {
        let Some(ax) = self.windows.get(&id).map(|w| w.ax.clone()) else { return };
        ax_service::run(ax.pid, move || {
            bus::send(Msg::Tracker(TrackerMsg::GreenButton(id, ax.green_button())));
        });
    }

    /// A window that just appeared can report its subrole a moment late.
    fn consider(&self, pid: i32, element: Element, attempt: u32) {
        ax_service::run(pid, move || {
            let probe = Probe::new(&element).map(Box::new);
            bus::send(Msg::Tracker(TrackerMsg::Considered { pid, element, probe, attempt }));
        });
    }

    fn refresh_focus(&self, pid: i32) {
        let Some(watch) = self.apps.get(&pid).cloned() else { return };
        ax_service::run(pid, move || {
            let id = ax::element(&watch.element, attr::FOCUSED_WINDOW).and_then(|el| ax::window_id(&el));
            bus::send(Msg::Tracker(TrackerMsg::Focus(pid, id.map(WindowId::from))));
        });
    }

    /// The focused window of the frontmost app, if Canopy tracks it.
    pub fn focused_window(&self) -> Option<WindowId> {
        self.focused.filter(|f| self.windows.contains_key(f))
    }

    pub fn handle(&mut self, msg: TrackerMsg) -> Vec<TrackerEvent> {
        match msg {
            TrackerMsg::AppLaunched(pid) if pid < 0 => self.scan(-pid),
            TrackerMsg::AppLaunched(pid) => {
                if let Some(app) = workspace::app(pid) {
                    self.watch(&app);
                }
            }
            TrackerMsg::AppTerminated(pid) => return self.unwatch(pid),
            TrackerMsg::AppHidden(pid, hidden) => {
                for w in self.windows.values_mut().filter(|w| w.ax.pid == pid) {
                    w.hidden = hidden;
                }
                return vec![TrackerEvent::AppHidden(pid, hidden)];
            }
            TrackerMsg::RefreshFocus(pid) => self.refresh_focus(pid),
            TrackerMsg::Scanned { pid, probes, gone } => {
                let Some(watch) = self.apps.get(&pid).cloned() else { return Vec::new() };
                let mut events: Vec<TrackerEvent> =
                    probes.iter().filter_map(|p| self.adopt(p, &watch)).collect();
                for id in gone {
                    if self.windows.remove(&id).is_some() {
                        events.push(TrackerEvent::Disappeared(id));
                    }
                }
                return events;
            }
            TrackerMsg::Consider { pid, element, attempt } => self.consider(pid, element, attempt),
            TrackerMsg::Considered { pid, element, probe, attempt } => {
                let (Some(probe), Some(watch)) = (probe, self.apps.get(&pid).cloned()) else { return Vec::new() };
                if self.windows.contains_key(&probe.id) {
                    return Vec::new();
                }
                if probe.is_standard() {
                    return self.adopt(&probe, &watch).into_iter().collect();
                }
                if probe.role.as_deref() == Some(ax::WINDOW_ROLE) && attempt < 3 {
                    let next = TrackerMsg::Consider { pid, element, attempt: attempt + 1 };
                    bus::send_after(Duration::from_millis(250), Msg::Tracker(next));
                }
            }
            TrackerMsg::Destroyed(id) => {
                if self.windows.remove(&id).is_some() {
                    return vec![TrackerEvent::Disappeared(id)];
                }
            }
            TrackerMsg::Minimized(id, minimized) => {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.minimized = minimized;
                    return vec![TrackerEvent::Minimized(id, minimized)];
                }
            }
            TrackerMsg::MovedOrResized(id, full_screen) => {
                if let Some(w) = self.windows.get_mut(&id) {
                    if full_screen != w.full_screen {
                        w.full_screen = full_screen;
                        return vec![TrackerEvent::FullScreen(id, full_screen)];
                    }
                    return vec![TrackerEvent::MovedOrResized(id)];
                }
            }
            TrackerMsg::TitleChanged(id, title) => {
                if let Some(w) = self.windows.get_mut(&id).filter(|w| w.title != title) {
                    w.title = title;
                    return vec![TrackerEvent::TitleChanged(id)];
                }
            }
            TrackerMsg::Focus(pid, id) => {
                if workspace::frontmost().map(|a| a.pid) != Some(pid) {
                    return Vec::new();
                }
                let tracked = id.filter(|i| self.windows.contains_key(i));
                if tracked != self.focused {
                    self.focused = tracked;
                    // A window can gain its toolbar, and so move its buttons, after it opens.
                    if let Some(id) = tracked {
                        self.find_green_button(id);
                    }
                    return vec![TrackerEvent::FocusChanged(tracked)];
                }
            }
            TrackerMsg::GreenButton(id, button) => {
                if let Some(w) = self.windows.get_mut(&id).filter(|w| w.green_button != button) {
                    w.green_button = button;
                    return vec![TrackerEvent::GreenButtonFound];
                }
            }
        }
        Vec::new()
    }
}

const APP_NOTIFICATIONS: [&str; 3] = ["AXWindowCreated", "AXFocusedWindowChanged", "AXMainWindowChanged"];
const WINDOW_NOTIFICATIONS: [&str; 6] = [
    "AXUIElementDestroyed",
    "AXWindowMiniaturized",
    "AXWindowDeminiaturized",
    "AXWindowMoved",
    "AXWindowResized",
    "AXTitleChanged",
];

/// The AXObserver of one app. Its callbacks arrive on the observer thread.
pub struct AppWatch {
    pid: i32,
    element: Element,
    bundle_id: String,
    name: String,
    observer: Mutex<Option<Observer>>,
    run_loop: OnceLock<RunLoop>,
    /// Window elements by id, for notifications about elements that can no longer answer.
    elements: Mutex<HashMap<WindowId, Element>>,
}

unsafe extern "C" fn callback(_: ax::AXObserverRef, element: ax::AXUIElementRef, notification: CFStringRef, refcon: *mut c_void) {
    // SAFETY: `refcon` is the watch's leaked reference (see `refcon`), and the
    // element and name are valid for the callback.
    unsafe {
        let watch = &*refcon.cast::<AppWatch>();
        let name = CFString::wrap_under_get_rule(notification).to_string();
        watch.handle(&name, Element::retained(element));
    }
}

impl AppWatch {
    fn new(app: &RunningApp) -> Arc<Self> {
        let bundle_id = app.bundle_id.clone().unwrap_or_else(|| format!("pid.{}", app.pid));
        Arc::new(Self {
            pid: app.pid,
            element: Element::application(app.pid),
            name: app.name.clone().unwrap_or_else(|| bundle_id.clone()),
            bundle_id,
            observer: Mutex::new(None),
            run_loop: OnceLock::new(),
            elements: Mutex::new(HashMap::new()),
        })
    }

    /// The pointer AX hands back to `callback`. A late callback can arrive
    /// after an app quits, so each watch keeps one reference forever.
    fn refcon(this: &Arc<Self>) -> usize {
        Arc::as_ptr(this) as usize
    }

    fn start(this: &Arc<Self>, run_loop: RunLoop) {
        let Some(observer) = Observer::new(this.pid, callback) else { return };
        // Never freed: see `refcon`.
        std::mem::forget(this.clone());
        run_loop.add_source(observer.run_loop_source());
        let _ = this.run_loop.set(run_loop);
        *this.observer.lock().unwrap() = Some(observer);
        let watch = this.clone();
        ax_service::run(this.pid, move || {
            let refcon = Self::refcon(&watch) as *mut c_void;
            if let Some(o) = watch.observer.lock().unwrap().as_ref() {
                for n in APP_NOTIFICATIONS {
                    o.add(&watch.element, n, refcon);
                }
            }
        });
    }

    fn observe_window(this: &Arc<Self>, el: &Element, id: WindowId) {
        this.elements.lock().unwrap().insert(id, el.clone());
        let (watch, el) = (this.clone(), el.clone());
        ax_service::run(this.pid, move || {
            let refcon = Self::refcon(&watch) as *mut c_void;
            if let Some(o) = watch.observer.lock().unwrap().as_ref() {
                for n in WINDOW_NOTIFICATIONS {
                    o.add(&el, n, refcon);
                }
            }
        });
    }

    fn element_for(&self, id: WindowId) -> Option<Element> {
        self.elements.lock().unwrap().get(&id).cloned()
    }

    /// The id of a window element, matched locally (no AX call).
    fn id_of(&self, el: &Element) -> Option<WindowId> {
        self.elements.lock().unwrap().iter().find(|(_, v)| *v == el).map(|(k, _)| *k)
    }

    fn stop(&self) {
        if let (Some(o), Some(run_loop)) = (self.observer.lock().unwrap().take(), self.run_loop.get()) {
            run_loop.remove_source(o.run_loop_source());
        }
    }

    /// On the observer thread; hands the work to the app's queue.
    fn handle(&self, notification: &str, element: Element) {
        let pid = self.pid;
        let send = |m| bus::send(Msg::Tracker(m));
        match notification {
            "AXWindowCreated" => send(TrackerMsg::Consider { pid, element, attempt: 0 }),
            "AXFocusedWindowChanged" | "AXMainWindowChanged" => {
                send(TrackerMsg::Consider { pid, element, attempt: 0 });
                send(TrackerMsg::RefreshFocus(pid));
            }
            "AXUIElementDestroyed" => {
                if let Some(id) = self.id_of(&element) {
                    send(TrackerMsg::Destroyed(id));
                }
            }
            "AXWindowMiniaturized" | "AXWindowDeminiaturized" => {
                if let Some(id) = self.id_of(&element) {
                    send(TrackerMsg::Minimized(id, notification == "AXWindowMiniaturized"));
                }
            }
            "AXWindowMoved" | "AXWindowResized" => {
                let Some(id) = self.id_of(&element) else { return };
                ax_service::run(pid, move || {
                    let full_screen = ax::boolean(&element, attr::FULL_SCREEN).unwrap_or(false);
                    bus::send(Msg::Tracker(TrackerMsg::MovedOrResized(id, full_screen)));
                });
            }
            "AXTitleChanged" => {
                let Some(id) = self.id_of(&element) else { return };
                ax_service::run(pid, move || {
                    let title = ax::string(&element, attr::TITLE).unwrap_or_default();
                    bus::send(Msg::Tracker(TrackerMsg::TitleChanged(id, title)));
                });
            }
            _ => {}
        }
    }
}
