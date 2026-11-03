//! Running apps, and what NSWorkspace tells us about them.

use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification, NSRunningApplication, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidHideApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification, NSWorkspaceDidUnhideApplicationNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue, NSString, NSURL};

use crate::bus::{self, Msg};
use crate::mac::tracker::TrackerMsg;

#[derive(Clone, Debug)]
pub struct RunningApp {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub name: Option<String>,
    pub regular: bool,
    pub finished_launching: bool,
    pub hidden: bool,
}

impl RunningApp {
    fn new(app: &NSRunningApplication) -> Self {
        Self {
            pid: app.processIdentifier(),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            name: app.localizedName().map(|s| s.to_string()),
            regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
            finished_launching: app.isFinishedLaunching(),
            hidden: app.isHidden(),
        }
    }
}

pub fn running_apps() -> Vec<RunningApp> {
    NSWorkspace::sharedWorkspace().runningApplications().iter().map(|a| RunningApp::new(&a)).collect()
}

pub fn app(pid: i32) -> Option<RunningApp> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).map(|a| RunningApp::new(&a))
}

pub fn frontmost() -> Option<RunningApp> {
    NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| RunningApp::new(&a))
}

pub fn activate(pid: i32) {
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}

pub fn terminate(pid: i32) {
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.terminate();
    }
}

pub fn icon(pid: i32) -> Option<Retained<objc2_app_kit::NSImage>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).and_then(|a| a.icon())
}

pub fn open_url(url: &str) {
    if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
}

/// The app's name from its bundle, for apps that aren't running.
pub fn app_name(bundle_id: &str) -> Option<String> {
    let url = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
    let path = url.path()?.to_string();
    let file = path.rsplit('/').next()?;
    Some(file.strip_suffix(".app").unwrap_or(file).to_string())
}

/// Runs `body` on the main thread, outside whatever is on the stack now.
/// AppKit's modal loops (alerts, context menus) must start from here: GPUI
/// runs its tasks during them, and it can't while it is itself on the stack.
pub fn on_main(body: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(body);
}

fn app_of(notification: &NSNotification) -> Option<Retained<NSRunningApplication>> {
    // SAFETY: the key is a constant, and this notification's value is an app.
    let key = unsafe { NSWorkspaceApplicationKey };
    let info = notification.userInfo()?;
    let value = info.objectForKey(key)?;
    value.downcast::<NSRunningApplication>().ok()
}

fn observe(center: &NSNotificationCenter, name: &NSNotificationName, body: impl Fn(&NSNotification) + 'static) {
    let block = RcBlock::new(move |n: NonNull<NSNotification>| {
        // SAFETY: AppKit passes a valid notification.
        body(unsafe { n.as_ref() });
    });
    let queue = NSOperationQueue::mainQueue();
    // SAFETY: the block is copied by the center and only reads the notification.
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block) };
    // Observers last as long as the app.
    std::mem::forget(token);
}

/// Sends what the system says about apps, displays and Spaces to the bus.
pub fn start_observing() {
    let ws = NSWorkspace::sharedWorkspace();
    let center = ws.notificationCenter();
    let on_app = |name: &NSNotificationName, make: fn(i32) -> TrackerMsg| {
        observe(&center, name, move |n| {
            if let Some(app) = app_of(n) {
                bus::send(Msg::Tracker(make(app.processIdentifier())));
            }
        });
    };
    // SAFETY: these constants are valid for the process's lifetime.
    unsafe {
        on_app(NSWorkspaceDidLaunchApplicationNotification, TrackerMsg::AppLaunched);
        on_app(NSWorkspaceDidTerminateApplicationNotification, TrackerMsg::AppTerminated);
        on_app(NSWorkspaceDidActivateApplicationNotification, TrackerMsg::RefreshFocus);
        on_app(NSWorkspaceDidHideApplicationNotification, |pid| TrackerMsg::AppHidden(pid, true));
        on_app(NSWorkspaceDidUnhideApplicationNotification, |pid| TrackerMsg::AppHidden(pid, false));
        observe(&center, NSWorkspaceActiveSpaceDidChangeNotification, |_| bus::send(Msg::SpaceChanged));
        observe(&NSNotificationCenter::defaultCenter(), NSApplicationDidChangeScreenParametersNotification, |_| {
            bus::send(Msg::ScreensChanged)
        });
    }
}

/// Runs a little AppleScript.
pub fn run_apple_script(source: &str) -> bool {
    let Some(script) = objc2_foundation::NSAppleScript::alloc_init_source(source) else { return false };
    let mut error: Option<Retained<objc2_foundation::NSDictionary<NSString, AnyObject>>> = None;
    // SAFETY: `error` is only written by the call.
    let result = unsafe { script.executeAndReturnError(Some(&mut error)) };
    let _ = result;
    error.is_none()
}

trait NSAppleScriptExt {
    fn alloc_init_source(source: &str) -> Option<Retained<objc2_foundation::NSAppleScript>>;
}

impl NSAppleScriptExt for objc2_foundation::NSAppleScript {
    fn alloc_init_source(source: &str) -> Option<Retained<objc2_foundation::NSAppleScript>> {
        objc2_foundation::NSAppleScript::initWithSource(objc2_foundation::NSAppleScript::alloc(), &NSString::from_str(source))
    }
}
