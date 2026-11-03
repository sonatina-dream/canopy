//! Still pictures of windows from ScreenCaptureKit, kept in memory only. Used
//! while windows move and on covers. Without Screen Recording permission every
//! picture is missing and callers draw app colour and icon.
//!
//! All capture work, and sampling the pictures, happens on a worker thread;
//! only finished pictures reach the main thread.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::Duration;

use block2::RcBlock;
use canopy_core::{Size, WindowId};
use core_foundation::string::CFStringRef;
use gpui::RenderImage;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use super::window_server::{on_screen, screen_capture_allowed};
use super::CGRect;
use crate::bus::{self, Msg};
use crate::imaging::{Colour, Pixels};

const ALPHA_PREMULTIPLIED_FIRST: u32 = 2;
const BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
const INTERPOLATION_MEDIUM: i32 = 4;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bits: usize, row: usize, space: *mut c_void, info: u32) -> *mut c_void;
    fn CGContextRelease(context: *mut c_void);
    fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *const c_void);
    fn CGContextSetInterpolationQuality(context: *mut c_void, quality: i32);
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
    static kCGColorSpaceSRGB: CFStringRef;
}

/// A CGImage's pixels at the given size, as sRGB BGRA.
pub fn pixels(image: &CGImage, width: usize, height: usize) -> Option<Pixels> {
    let (width, height) = (width.max(1), height.max(1));
    let mut data = vec![0u8; width * height * 4];
    // SAFETY: `data` outlives the context, which is released here, and is big
    // enough for its rows.
    unsafe {
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let context = CGBitmapContextCreate(
            data.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            space,
            ALPHA_PREMULTIPLIED_FIRST | BYTE_ORDER_32_LITTLE,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return None;
        }
        CGContextSetInterpolationQuality(context, INTERPOLATION_MEDIUM);
        CGContextDrawImage(
            context,
            CGRect::new(0.0, 0.0, width as f64, height as f64),
            (image as *const CGImage).cast(),
        );
        CGContextRelease(context);
    }
    Some(Pixels { width, height, data })
}

pub fn image_size(image: &CGImage) -> (usize, usize) {
    let p = (image as *const CGImage).cast();
    // SAFETY: `image` is a valid CGImage.
    unsafe { (CGImageGetWidth(p), CGImageGetHeight(p)) }
}

/// Pixels as an image GPUI can paint (it wants BGRA, as these already are).
pub fn render_image(p: Pixels) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(p.width as u32, p.height as u32, p.data)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

pub struct Picture {
    pub image: Arc<RenderImage>,
    /// Its size in points.
    pub size: Size,
}

pub struct WindowShot {
    pub id: WindowId,
    pub picture: Arc<Picture>,
    pub edge: Option<Colour>,
}

pub struct DeskShot {
    pub display: u32,
    pub backdrop: Arc<RenderImage>,
    pub glass: Option<Arc<RenderImage>>,
}

enum Job {
    Windows { ids: HashSet<WindowId>, scale: f64 },
    /// Whatever is on screen then, except `skip`.
    OnScreen { skip: HashSet<WindowId>, scale: f64 },
    Desks { scales: HashMap<u32, f64> },
    Access,
}

static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Whether Screen Recording is allowed, as last checked. Really asking is a
/// round trip to another process, so only the worker does it.
pub fn access_allowed() -> bool {
    ALLOWED.load(Ordering::Relaxed)
}

fn work(job: Job) {
    let allowed = screen_capture_allowed();
    ALLOWED.store(allowed, Ordering::Relaxed);
    match job {
        Job::Access => {}
        Job::Windows { ids, scale } => bus::send(Msg::Shots(if allowed { capture_windows(&ids, scale) } else { Vec::new() })),
        Job::OnScreen { skip, scale } => {
            let own = std::process::id() as i32;
            let ids: HashSet<WindowId> = on_screen().iter().filter(|o| o.pid != own && !skip.contains(&o.id)).map(|o| o.id).collect();
            bus::send(Msg::Shots(if allowed { capture_windows(&ids, scale) } else { Vec::new() }));
        }
        Job::Desks { scales } => bus::send(Msg::Desks(if allowed { capture_desks(&scales) } else { Vec::new() })),
    }
}

static WORKER: LazyLock<Mutex<Sender<Job>>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("canopy.capture".to_string())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                work(job);
            }
        })
        .expect("thread starts");
    Mutex::new(tx)
});

fn shareable_content(excluding_desktop: bool) -> Option<Retained<SCShareableContent>> {
    struct Content(Option<Retained<SCShareableContent>>);
    // SAFETY: the content is an immutable snapshot, handed over once.
    unsafe impl Send for Content {}
    let (tx, rx) = mpsc::sync_channel(1);
    let block = RcBlock::new(move |content: *mut SCShareableContent, _: *mut NSError| {
        // SAFETY: the content is valid for the callback, or null.
        let _ = tx.try_send(Content(unsafe { Retained::retain(content) }));
    });
    // SAFETY: the block is copied by the call and is safe to run on any queue.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            excluding_desktop,
            true,
            &block,
        );
    }
    rx.recv_timeout(CAPTURE_TIMEOUT).ok().and_then(|c| c.0)
}

fn capture(filter: &SCContentFilter, config: &SCStreamConfiguration) -> Option<Pixels> {
    let (tx, rx) = mpsc::sync_channel(1);
    let block = RcBlock::new(move |image: *mut CGImage, _: *mut NSError| {
        let picture = NonNull::new(image).and_then(|i| {
            // SAFETY: the image is valid for the callback.
            let image = unsafe { i.as_ref() };
            let (w, h) = image_size(image);
            pixels(image, w, h)
        });
        let _ = tx.try_send(picture);
    });
    // SAFETY: the block is copied by the call and is safe to run on any queue.
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, config, Some(&block)) };
    rx.recv_timeout(CAPTURE_TIMEOUT).ok().flatten()
}

fn capture_windows(ids: &HashSet<WindowId>, scale: f64) -> Vec<WindowShot> {
    let mut results = Vec::new();
    let Some(content) = shareable_content(true) else { return results };
    // SAFETY: plain reads of a valid snapshot.
    unsafe {
        for w in content.windows().iter() {
            let id = WindowId::from(w.windowID());
            if !ids.contains(&id) {
                continue;
            }
            let frame = w.frame();
            let config = SCStreamConfiguration::new();
            config.setWidth(((frame.size.width * scale) as usize).max(1));
            config.setHeight(((frame.size.height * scale) as usize).max(1));
            config.setShowsCursor(false);
            config.setIgnoreShadowsSingleWindow(true);
            let filter = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w);
            let Some(p) = capture(&filter, &config) else { continue };
            let size = Size::new(p.width as f64 / scale, p.height as f64 / scale);
            let edge = p.edge_colour();
            if let Some(image) = render_image(p) {
                results.push(WindowShot { id, picture: Arc::new(Picture { image, size }), edge });
            }
        }
    }
    results
}

/// Blur 20 pt and darken 30%, worked at a quarter size since a blur hides the
/// detail anyway.
pub fn make_glass(desktop: &Pixels) -> Pixels {
    desktop.shrunk(4).blurred(10.0).darkened(0.3)
}

fn capture_desks(scales: &HashMap<u32, f64>) -> Vec<DeskShot> {
    let mut results = Vec::new();
    let Some(content) = shareable_content(false) else { return results };
    // SAFETY: plain reads of a valid snapshot.
    unsafe {
        // Everything at or above the normal window level is left out: windows,
        // panels, Canopy's own stage, the Dock and the menu bar.
        let windows = content.windows();
        let above: Vec<Retained<SCWindow>> = windows.iter().filter(|w| w.windowLayer() >= 0).collect();
        let above = NSArray::from_retained_slice(&above);
        for display in content.displays().iter() {
            results.extend(capture_desk(&display, &above, scales));
        }
    }
    results
}

/// # Safety
/// `display` and `above` come from a valid snapshot.
unsafe fn capture_desk(display: &SCDisplay, above: &NSArray<SCWindow>, scales: &HashMap<u32, f64>) -> Option<DeskShot> {
    let id = display.displayID();
    let scale = scales.get(&id).copied().unwrap_or(2.0);
    let config = SCStreamConfiguration::new();
    config.setWidth((display.width() as f64 * scale) as usize);
    config.setHeight((display.height() as f64 * scale) as usize);
    config.setShowsCursor(false);
    let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, above);
    let p = capture(&filter, &config)?;
    let glass = render_image(make_glass(&p));
    Some(DeskShot { display: id, backdrop: render_image(p)?, glass })
}

/// The pictures Canopy has now, on the main thread.
#[derive(Default)]
pub struct PictureStore {
    pub images: HashMap<WindowId, Arc<Picture>>,
    /// The colour at each picture's right and bottom edges.
    pub edges: HashMap<WindowId, Colour>,
    /// Each display's desktop exactly as it is drawn (wallpaper, dynamic or
    /// not, and desktop icons), captured with every window left out. The stage
    /// stands on this, so the background never changes under a drag.
    pub backdrops: HashMap<u32, Arc<RenderImage>>,
    /// The same desktop blurred and darkened: the glass behind arranged windows.
    pub glass: HashMap<u32, Arc<RenderImage>>,
    in_flight: bool,
    queued: HashSet<WindowId>,
    desks_in_flight: bool,
    pub enabled: bool,
}

impl PictureStore {
    pub fn forget(&mut self, id: WindowId) {
        self.images.remove(&id);
        self.edges.remove(&id);
    }

    /// Pictures of everything on screen, leaving out `skip`.
    pub fn refresh_on_screen(&mut self, skip: HashSet<WindowId>, scale: f64) {
        if !self.enabled {
            let _ = WORKER.lock().unwrap().send(Job::Access);
        } else if !self.in_flight {
            self.in_flight = true;
            let _ = WORKER.lock().unwrap().send(Job::OnScreen { skip, scale });
        }
    }

    pub fn refresh(&mut self, ids: impl IntoIterator<Item = WindowId>, scale: f64) {
        if !self.enabled {
            return;
        }
        let ids: HashSet<WindowId> = ids.into_iter().collect();
        if ids.is_empty() {
            return;
        }
        if self.in_flight {
            self.queued.extend(ids);
            return;
        }
        self.in_flight = true;
        let _ = WORKER.lock().unwrap().send(Job::Windows { ids, scale });
    }

    pub fn windows_arrived(&mut self, shots: Vec<WindowShot>, scale: f64) -> Vec<WindowId> {
        let mut arrived = Vec::with_capacity(shots.len());
        for shot in shots {
            self.images.insert(shot.id, shot.picture);
            if let Some(edge) = shot.edge {
                self.edges.insert(shot.id, edge);
            }
            arrived.push(shot.id);
        }
        self.in_flight = false;
        if !self.queued.is_empty() {
            let next: Vec<WindowId> = self.queued.drain().collect();
            self.refresh(next, scale);
        }
        arrived
    }

    pub fn refresh_desks(&mut self, scales: HashMap<u32, f64>) {
        if self.desks_in_flight {
            return;
        }
        self.desks_in_flight = true;
        let _ = WORKER.lock().unwrap().send(Job::Desks { scales });
    }

    pub fn desks_arrived(&mut self, shots: Vec<DeskShot>) -> Vec<u32> {
        self.desks_in_flight = false;
        shots
            .into_iter()
            .map(|shot| {
                self.backdrops.insert(shot.display, shot.backdrop);
                match shot.glass {
                    Some(glass) => self.glass.insert(shot.display, glass),
                    None => self.glass.remove(&shot.display),
                };
                shot.display
            })
            .collect()
    }
}
