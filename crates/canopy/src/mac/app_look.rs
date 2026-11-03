//! App icon and colour, used for covers and for motion without pictures, plus
//! the wallpaper and the symbols the settings use.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::{CFURLRef, CFURL};
use gpui::RenderImage;
use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::{NSImage, NSWorkspace};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::capture::{image_size, pixels, render_image};
use super::displays::Display;
use super::workspace;
use crate::bus::{self, Msg};
use crate::imaging::{placeholder, Colour, Pixels};

const ICON_PIXELS: usize = 128;

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageSourceCreateWithURL(url: CFURLRef, options: *const c_void) -> *const c_void;
    fn CGImageSourceCreateThumbnailAtIndex(source: *const c_void, index: usize, options: *const c_void) -> *const c_void;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    static kCGImageSourceShouldCacheImmediately: CFStringRef;
}

#[derive(Default)]
struct Cache {
    requested: HashSet<String>,
    icons: HashMap<String, Arc<RenderImage>>,
    colours: HashMap<String, Colour>,
    wallpapers: HashMap<String, Arc<RenderImage>>,
    symbols: HashMap<String, Arc<RenderImage>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

fn ns_image_pixels(image: &NSImage, width: usize, height: usize) -> Option<Pixels> {
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width as f64, height as f64));
    // SAFETY: `rect` outlives the call, and no context or hints are passed.
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    pixels(&cg, width, height)
}

fn app_icon(bundle_id: &str, pid: i32) -> Option<Retained<NSImage>> {
    workspace::icon(pid).or_else(|| {
        let ws = NSWorkspace::sharedWorkspace();
        let url = ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
        let path = url.path()?;
        Some(ws.iconForFile(&path))
    })
}

/// Icons load from disk and decode, so they are worked out on a thread of their
/// own, one app at a time. Until one arrives, its app is shown in grey.
static LOOKS: LazyLock<Mutex<Sender<(String, i32)>>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel::<(String, i32)>();
    thread::Builder::new()
        .name("canopy.app-looks".to_string())
        .spawn(move || {
            while let Ok((bundle_id, pid)) = rx.recv() {
                let (icon, colour) = autoreleasepool(|_| {
                    let picture = app_icon(&bundle_id, pid).and_then(|i| ns_image_pixels(&i, ICON_PIXELS, ICON_PIXELS));
                    let colour = picture.as_ref().and_then(|p| p.shrunk(ICON_PIXELS / 16).app_colour());
                    (picture.and_then(render_image), colour)
                });
                bus::send(Msg::AppLook { bundle_id, icon, colour });
            }
        })
        .expect("thread starts");
    Mutex::new(tx)
});

/// Asks for an app's icon and colour, once.
pub fn request(bundle_id: &str, pid: i32) {
    if CACHE.with_borrow_mut(|c| c.requested.insert(bundle_id.to_string())) {
        let _ = LOOKS.lock().unwrap().send((bundle_id.to_string(), pid));
    }
}

pub fn look_arrived(bundle_id: String, icon: Option<Arc<RenderImage>>, colour: Option<Colour>) {
    CACHE.with_borrow_mut(|c| {
        if let Some(icon) = icon {
            c.icons.insert(bundle_id.clone(), icon);
        }
        if let Some(colour) = colour {
            c.colours.insert(bundle_id, colour);
        }
    });
}

pub fn icon(bundle_id: &str, pid: i32) -> Option<Arc<RenderImage>> {
    request(bundle_id, pid);
    CACHE.with_borrow(|c| c.icons.get(bundle_id).cloned())
}

/// The icon's average opaque colour, a little more saturated.
pub fn colour(bundle_id: &str, pid: i32) -> Colour {
    request(bundle_id, pid);
    CACHE.with_borrow(|c| c.colours.get(bundle_id).copied()).unwrap_or(Colour::grey(0.56))
}

/// What a window without a picture shows.
pub fn placeholder_colour(bundle_id: &str, pid: i32) -> Colour {
    placeholder(colour(bundle_id, pid))
}

/// The display's wallpaper, decoded ahead of time so the first motion doesn't
/// wait for a large image to decode.
pub fn wallpaper(display: &Display) -> Option<Arc<RenderImage>> {
    let key = wallpaper_key(display)?;
    let found = CACHE.with_borrow(|c| c.wallpapers.get(&key).cloned());
    if found.is_none() {
        prepare_wallpapers();
    }
    found
}

fn wallpaper_url(display: &Display) -> Option<String> {
    let screen = display.screen()?;
    let url = NSWorkspace::sharedWorkspace().desktopImageURLForScreen(&screen)?;
    Some(url.path()?.to_string())
}

fn wallpaper_key(display: &Display) -> Option<String> {
    Some(format!("{}{}", display.uuid, wallpaper_url(display)?))
}

pub fn wallpaper_arrived(key: String, image: Arc<RenderImage>) {
    CACHE.with_borrow_mut(|c| {
        c.wallpapers.insert(key, image);
    });
}

/// Decodes every display's wallpaper off the main thread, at screen size.
pub fn prepare_wallpapers() {
    for display in Display::current() {
        let (Some(path), Some(key)) = (wallpaper_url(&display), wallpaper_key(&display)) else { continue };
        if CACHE.with_borrow(|c| c.wallpapers.contains_key(&key)) {
            continue;
        }
        let side = display.frame.width.max(display.frame.height) * display.scale;
        thread::spawn(move || {
            if let Some(image) = decode(&path, side as i64) {
                bus::send(Msg::Wallpaper(key, image));
            }
        });
    }
}

fn decode(path: &str, max_side: i64) -> Option<Arc<RenderImage>> {
    let url = CFURL::from_path(path, false)?;
    let options = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[
        // SAFETY: these keys are constants valid for the process's lifetime.
        unsafe { (CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways), CFBoolean::true_value().as_CFType()) },
        unsafe { (CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize), CFNumber::from(max_side).as_CFType()) },
        unsafe { (CFString::wrap_under_get_rule(kCGImageSourceShouldCacheImmediately), CFBoolean::true_value().as_CFType()) },
    ]);
    // SAFETY: both results follow the create rule and are wrapped straight away.
    unsafe {
        let source = CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), ptr::null());
        if source.is_null() {
            return None;
        }
        let source = CFType::wrap_under_create_rule(source);
        let image = CGImageSourceCreateThumbnailAtIndex(source.as_CFTypeRef(), 0, options.as_CFTypeRef());
        if image.is_null() {
            return None;
        }
        let image = CFType::wrap_under_create_rule(image);
        let cg = &*(image.as_CFTypeRef() as *const CGImage);
        let (w, h) = image_size(cg);
        render_image(pixels(cg, w, h)?)
    }
}

/// A system symbol in the given colour and opacity, at twice `points` for
/// sharpness.
pub fn symbol(name: &str, points: f64, colour: Colour, opacity: f32) -> Option<Arc<RenderImage>> {
    let key = format!("{name}/{points}/{colour:?}/{opacity}");
    if let Some(found) = CACHE.with_borrow(|c| c.symbols.get(&key).cloned()) {
        return Some(found);
    }
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
    let size = image.size();
    let longest = size.width.max(size.height).max(1.0);
    let (w, h) = (points * 2.0 * size.width / longest, points * 2.0 * size.height / longest);
    let mut p = ns_image_pixels(&image, w.round() as usize, h.round() as usize)?;
    let tint = [colour.b, colour.g, colour.r].map(|c| (c * 255.0) as u8);
    for px in p.data.chunks_exact_mut(4) {
        px[..3].copy_from_slice(&tint);
        px[3] = (f32::from(px[3]) * opacity) as u8;
    }
    let image = render_image(p)?;
    CACHE.with_borrow_mut(|c| c.symbols.insert(key, image.clone()));
    Some(image)
}

