//! Every Accessibility call goes through here, off the main thread. Apps
//! answer AX at their own pace (Finder takes 40+ ms a call), so each app gets
//! its own serial queue: a slow app never holds up the pointer, the layout, or
//! any other app.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use canopy_core::{Rect, Size, WindowId};
use dispatch2::{DispatchQueue, DispatchRetained};
use futures::channel::oneshot;

use super::ax::AxWindow;
use crate::bus::{self, Msg};
use crate::clog;

static QUEUES: LazyLock<Mutex<HashMap<i32, DispatchRetained<DispatchQueue>>>> = LazyLock::new(Default::default);

fn queue(pid: i32) -> DispatchRetained<DispatchQueue> {
    let mut queues = QUEUES.lock().unwrap();
    queues
        .entry(pid)
        .or_insert_with(|| DispatchQueue::new(&format!("canopy.ax.{pid}"), None))
        .clone()
}

/// Runs `work` on the app's queue.
pub fn run(pid: i32, work: impl FnOnce() + Send + 'static) {
    queue(pid).exec_async(work);
}

pub fn forget(pid: i32) {
    QUEUES.lock().unwrap().remove(&pid);
}

/// What a finished write found.
#[derive(Clone, Copy, Debug)]
pub struct WriteResult {
    pub id: WindowId,
    pub asked: Rect,
    /// The size the window actually took (`None` when it didn't answer).
    pub actual_size: Option<Size>,
}

#[derive(Default)]
struct Writes {
    pending: HashMap<WindowId, (Arc<AxWindow>, Rect)>,
    running: HashSet<WindowId>,
    idle_waiters: Vec<oneshot::Sender<()>>,
}

/// Moves and resizes windows asynchronously. Writes are coalesced: if a window
/// is asked to move again before its last move finished, only the newest
/// target is written.
#[derive(Clone, Default)]
pub struct WindowWriter {
    state: Arc<Mutex<Writes>>,
}

impl WindowWriter {
    /// Whether a write to this window is queued or running.
    pub fn is_busy(&self, id: WindowId) -> bool {
        let s = self.state.lock().unwrap();
        s.running.contains(&id) || s.pending.contains_key(&id)
    }

    /// Writes synchronously, all apps in parallel, waiting at most `timeout`.
    /// Only for quitting, when there is no later.
    pub fn flush_synchronously(&self, jobs: Vec<(Arc<AxWindow>, Rect)>, timeout: Duration) {
        let (tx, rx) = mpsc::channel();
        let count = jobs.len();
        for (w, r) in jobs {
            let tx = tx.clone();
            run(w.pid, move || {
                w.set_frame(r);
                let _ = tx.send(());
            });
        }
        let deadline = Instant::now() + timeout;
        for _ in 0..count {
            let left = deadline.saturating_duration_since(Instant::now());
            if rx.recv_timeout(left).is_err() {
                break;
            }
        }
    }

    pub fn set_frame(&self, w: &Arc<AxWindow>, r: Rect) {
        let id = WindowId::from(w.id);
        let start = {
            let mut s = self.state.lock().unwrap();
            s.pending.insert(id, (w.clone(), r));
            s.running.insert(id)
        };
        if start {
            let writer = self.clone();
            run(w.pid, move || writer.drain(id));
        }
    }

    fn drain(&self, id: WindowId) {
        loop {
            let next = {
                let mut s = self.state.lock().unwrap();
                match s.pending.remove(&id) {
                    Some(next) => next,
                    None => {
                        s.running.remove(&id);
                        if s.running.is_empty() && s.pending.is_empty() {
                            for waiter in s.idle_waiters.drain(..) {
                                let _ = waiter.send(());
                            }
                        }
                        return;
                    }
                }
            };
            let (w, r) = next;
            let started = Instant::now();
            w.set_frame(r);
            let result = WriteResult { id, asked: r, actual_size: w.current_size() };
            let ms = started.elapsed().as_millis();
            if ms > 40 {
                clog!("AX slow w{id} {ms} ms");
            }
            bus::send(Msg::Written(result));
        }
    }

    /// Resolves once every write has finished. The caller adds its own timeout.
    pub fn idle(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let mut s = self.state.lock().unwrap();
        if s.running.is_empty() && s.pending.is_empty() {
            let _ = tx.send(());
        } else {
            s.idle_waiters.push(tx);
        }
        rx
    }
}
