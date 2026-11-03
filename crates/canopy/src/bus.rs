//! Everything that happens off the main thread reaches it here, as one stream
//! of messages. Nothing on the main thread ever waits for them.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use canopy_core::{Command, WindowId};
use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchTime, GlobalQueueIdentifier};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use gpui::RenderImage;

use crate::coordinator::input::InputMsg;
use crate::imaging::Colour;
use crate::mac::ax_service::WriteResult;
use crate::mac::capture::{DeskShot, WindowShot};
use crate::mac::tracker::TrackerMsg;

pub enum Msg {
    Tracker(TrackerMsg),
    Written(WriteResult),
    Input(InputMsg),
    Run(Command),
    Urls(Vec<String>),
    Shots(Vec<WindowShot>),
    Desks(Vec<DeskShot>),
    Wallpaper(String, Arc<RenderImage>),
    AppLook { bundle_id: String, icon: Option<Arc<RenderImage>>, colour: Option<Colour> },
    /// Which of the windows asked about have left each world's Space.
    SpacesChecked(Vec<(String, Vec<WindowId>)>),
    ScreensChanged,
    SpaceChanged,
    Quit,
    ForgetView(i64),
    SetIgnored(String, bool),
    AskScreenRecording,
    CloseWindow(WindowId),
    FloatWindow(WindowId),
    MinimizeWindow(WindowId),
    /// Add an app to the excluded list, from the Settings menu.
    IgnoreApp(String),
    /// A key pressed while recording a shortcut.
    RecordedKey { key_code: u16, modifiers: usize },
    Harness(String),
    HarnessClosed,
}

impl Msg {
    /// A name for the log.
    pub fn kind(&self) -> &'static str {
        match self {
            Msg::Tracker(_) => "tracker",
            Msg::Written(_) => "written",
            Msg::Input(_) => "input",
            Msg::Run(_) => "command",
            Msg::Urls(_) => "urls",
            Msg::Shots(_) => "pictures",
            Msg::Desks(_) => "desks",
            Msg::Wallpaper(..) => "wallpaper",
            Msg::AppLook { .. } => "app look",
            Msg::SpacesChecked(_) => "spaces",
            Msg::ScreensChanged => "screens",
            Msg::SpaceChanged => "space change",
            Msg::Quit | Msg::HarnessClosed => "quit",
            Msg::ForgetView(_) => "forget view",
            Msg::SetIgnored(..) | Msg::IgnoreApp(_) => "ignore app",
            Msg::AskScreenRecording => "ask screen recording",
            Msg::CloseWindow(_) | Msg::FloatWindow(_) | Msg::MinimizeWindow(_) => "window action",
            Msg::RecordedKey { .. } => "recorded key",
            Msg::Harness(_) => "harness",
        }
    }
}

static BUS: OnceLock<UnboundedSender<Msg>> = OnceLock::new();

pub fn init() -> UnboundedReceiver<Msg> {
    let (tx, rx) = unbounded();
    let _ = BUS.set(tx);
    rx
}

pub fn send(msg: Msg) {
    if let Some(bus) = BUS.get() {
        let _ = bus.unbounded_send(msg);
    }
}

/// Sends `msg` once `delay` has passed.
pub fn send_after(delay: Duration, msg: Msg) {
    let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Default));
    let when = DispatchTime::try_from(delay).unwrap_or(DispatchTime::NOW);
    let msg = SendCell(msg);
    let _ = queue.after(when, move || send(msg.take()));
}

/// Moves a message into a dispatch closure, which wants `Fn`-free ownership.
struct SendCell(Msg);

impl SendCell {
    fn take(self) -> Msg {
        self.0
    }
}
