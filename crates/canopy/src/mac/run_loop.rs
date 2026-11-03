//! A thread with its own run loop, for work that must never wait on the main
//! thread: the event tap (so the pointer never stutters) and AX observers.

use std::ffi::c_void;
use std::sync::mpsc;
use std::thread;

use core_foundation_sys::date::CFAbsoluteTimeGetCurrent;
use core_foundation_sys::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopAddTimer, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRemoveSource,
    CFRunLoopRunInMode, CFRunLoopSourceRef, CFRunLoopTimerCreate, CFRunLoopTimerRef, CFRunLoopWakeUp,
};

extern "C" {
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
}

const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[derive(Clone, Copy)]
pub struct RunLoop(usize);

impl RunLoop {
    fn raw(self) -> CFRunLoopRef {
        self.0 as CFRunLoopRef
    }

    pub fn add_source(self, source: CFRunLoopSourceRef) {
        // SAFETY: both are valid; adding a source from another thread is allowed.
        unsafe {
            CFRunLoopAddSource(self.raw(), source, kCFRunLoopDefaultMode);
            CFRunLoopWakeUp(self.raw());
        }
    }

    pub fn remove_source(self, source: CFRunLoopSourceRef) {
        // SAFETY: both are valid; removing a source from another thread is allowed.
        unsafe { CFRunLoopRemoveSource(self.raw(), source, kCFRunLoopDefaultMode) };
    }
}

extern "C" fn keep_alive(_: CFRunLoopTimerRef, _: *mut c_void) {}

/// Starts a thread with a run loop and waits until it is running. `init` runs
/// on that thread first, and its result is handed back.
pub fn start<T: Send + 'static>(name: &str, init: impl FnOnce(RunLoop) -> T + Send + 'static) -> (RunLoop, T) {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            // SAFETY: plain calls on this thread's own run loop.
            unsafe {
                pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
                let run_loop = RunLoop(CFRunLoopGetCurrent() as usize);
                // A far-off timer keeps the run loop alive before any source is added.
                let at = CFAbsoluteTimeGetCurrent() + 1e9;
                let timer = CFRunLoopTimerCreate(std::ptr::null(), at, 1e9, 0, 0, keep_alive, std::ptr::null_mut());
                CFRunLoopAddTimer(run_loop.raw(), timer, kCFRunLoopDefaultMode);
                let _ = tx.send((run_loop, init(run_loop)));
                loop {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 60.0, 0);
                }
            }
        })
        .expect("thread starts");
    rx.recv().expect("thread reports in")
}
