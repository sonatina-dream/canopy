//! A running log in ~/Library/Logs/Canopy.log (the previous run's is kept as
//! Canopy.previous.log), plus the last lines in memory for the test harness.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

struct State {
    recent: Vec<String>,
    file: Option<File>,
    folder: PathBuf,
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    Mutex::new(State { recent: Vec::new(), file: None, folder: home.join("Library/Logs") })
});

/// The test harness keeps its log somewhere disposable.
pub fn set_folder(folder: PathBuf) {
    STATE.lock().unwrap().folder = folder;
}

pub fn open() {
    let mut s = STATE.lock().unwrap();
    let url = s.folder.join("Canopy.log");
    let previous = s.folder.join("Canopy.previous.log");
    let _ = fs::create_dir_all(&s.folder);
    let _ = fs::remove_file(&previous);
    let _ = fs::rename(&url, &previous);
    s.file = File::create(&url).ok();
}

pub fn add(message: String) {
    let line = format!("{:8.3} {message}", START.elapsed().as_secs_f64());
    let mut s = STATE.lock().unwrap();
    if let Some(f) = s.file.as_mut() {
        let _ = writeln!(f, "{line}");
    }
    s.recent.push(line);
    if s.recent.len() > 400 {
        let extra = s.recent.len() - 400;
        s.recent.drain(..extra);
    }
}

/// Logs main-thread work long enough to be felt. A STALL with no SLOW beside it
/// wasn't Canopy's work: macOS didn't run Canopy in time.
pub fn slow(what: &str, started: Instant) {
    let ms = started.elapsed().as_millis();
    if ms >= 30 {
        add(format!("SLOW {what} {ms} ms"));
    }
}

pub fn take() -> Vec<String> {
    std::mem::take(&mut STATE.lock().unwrap().recent)
}

/// Sends warnings from GPUI and its crates to Canopy's log. GPUI reports most
/// failures only here, and carries on as if nothing happened.
struct LogBridge;

impl ::log::Log for LogBridge {
    fn enabled(&self, metadata: &::log::Metadata) -> bool {
        metadata.level() <= ::log::Level::Warn
    }

    fn log(&self, record: &::log::Record) {
        if self.enabled(record.metadata()) {
            add(format!("{} {}: {}", record.level(), record.target(), record.args()));
        }
    }

    fn flush(&self) {}
}

pub fn capture_library_logs() {
    static BRIDGE: LogBridge = LogBridge;
    if ::log::set_logger(&BRIDGE).is_ok() {
        ::log::set_max_level(::log::LevelFilter::Warn);
    }
}

/// `clog!("WRITE w{id}")` adds a line to the log.
#[macro_export]
macro_rules! clog {
    ($($arg:tt)*) => { $crate::log::add(format!($($arg)*)) };
}
