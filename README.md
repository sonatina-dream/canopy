# Canopy

A window manager for macOS that docks real app windows side by side, nests them
as deep as you like, and zooms a camera in and out over the arrangement. It's
modelled on TrellisWM's behaviour. [SPEC.md](SPEC.md) has the spec, the evidence
it came from, and where Canopy deliberately differs.

## Build and run

```sh
scripts/build-app.sh          # → build/Canopy.app
open build/Canopy.app
```

Needs macOS 14+, a Rust toolchain and Xcode's command-line tools. Canopy is
written in Rust; everything it draws uses GPUI, Zed's GPU-accelerated UI
framework, pinned to a commit in `Cargo.toml`. The first build fetches and
compiles GPUI, which takes several minutes. On first launch a short setup
walks you through **Accessibility** (required, to move windows) and **Screen
Recording** (optional: windows keep their real contents while they move). It
also turns off the macOS features that fight docking: edge tiling,
click-wallpaper-to-reveal and Stage Manager.

Run only one window manager at a time. If TrellisWM (or AeroSpace, Rectangle
and similar) is running, Canopy says so and offers to quit it. Canopy keeps a
log in `~/Library/Logs/Canopy.log`.

The script signs with your first code-signing identity, so permissions survive
rebuilds. With `CODESIGN_IDENTITY=-` it signs ad hoc, and you have to allow
Accessibility again after every rebuild.

## Using it

- **Dock:** drag a window by its title bar onto the edge of another window,
  or onto the screen edge to dock it beside everything you're looking at. A
  preview shows where it will land. Drag the gap between two windows to resize both.
- **Nest:** to put a window inside a space, drop it on the top or bottom edge of a
  window in a row (or the sides, in a column). Or zoom in on the space and drag
  the window to the screen edge. Over a floating window, pause: the two dock together.
- **Float:** drop it on the Desktop, or click the green button (⌥-click still zooms).
- **Tabs:** drop a window on the middle of another and pause. Click a tab to
  switch, drag it out to tear it off, or right-click it for more.
- **Desktops:** every macOS desktop (Space) on every display has its own
  arrangement, view and saved views. Switching desktops switches arrangements;
  windows on other desktops are never touched.
- **Zoom:** windows too small to use show as their app's colour and icon.
  Double-click one to go in. Double-click a title bar to give that window the
  screen, and double-click it again to come back.

| Shortcut | |
|---|---|
| ⌃⌥Space | Show everything |
| ⌃⌥↑ / ↓ | Step out / in a level |
| ⌃⌥← / → | Neighbouring space left / right |
| ⌃⌥⇧↑ / ↓ | Neighbouring space above / below |
| ⌃⌥[ / ] | Back / forward |
| ⌃⌥↩ | Focused window on its own |
| ⌃⌥D | Hide or show the Desktop |
| ⌃⌥N | Split with a new Finder window |
| ⌃⌥⇧1–9 / ⌃⌥1–9 | Save view / go to view |
| ⌘⇧-drag | Zoom to a rectangle |
| ⌥-pinch | Zoom in and out |

All of these can be changed in Settings ▸ Shortcuts. Every command is also a URL,
for Raycast, Alfred, Shortcuts or a shell:

```sh
open canopy://everything
open "canopy://neighbour?dir=left"     # left, right, up, down
open "canopy://view?n=1"               # also save-view?n=1
open canopy://back                     # forward, zoom-in, zoom-out
open canopy://frame-window             # frame-desktop, toggle-desktop
open canopy://arrange                  # float-all, split-finder
open "canopy://settings?pane=shortcuts"
```

Quitting Canopy brings every window back into view. Arrangements are saved in
`~/Library/Application Support/Canopy/arrangements.json`.

## Tests

```sh
cargo test --workspace                  # 86 tests of the model and image code
python3 -I Tests/E2E/e2e.py             # 76 checks against real windows
python3 -I Tests/E2E/e2e.py --input     # adds 19 checks with real mouse input and pinches
python3 -I Tests/E2E/e2e.py --spaces    # adds 8 checks that switch to a second desktop and back
```

The end-to-end test needs `scripts/build-app.sh` first. It opens a throwaway app
with test windows and runs `Canopy --harness <bundle-id>`, which manages only
that app's windows and keeps its state in a temporary folder. Your own windows
aren't touched. One test window answers as slowly as Finder, and the test checks
that Canopy's main thread never stalls over 100 ms. Quit other window managers
first. Run it at an unlocked Mac: while it's locked, macOS scales every window's
reported frame and refuses to say which window is which. `--input` moves the
pointer for about 30 seconds and posts real clicks.
Before every press it checks that the topmost ordinary window under the pointer
is a test window, but that check doesn't see system dialogs, so don't run it with
one open. The terminal running the tests needs the Accessibility permission.

The harness takes no screenshots unless it is given `--pictures`, so the tests
don't raise macOS's screen-capture prompts. To see what Canopy draws without
capturing the screen, build with `--features snapshots`: the harness then has
`fake-pictures` and `snapshot <dir>`, which render its windows offscreen to PNGs.
GPUI reports most failures only through `log`, and Canopy sends those to its log.

## Layout

| Path | What |
|---|---|
| `crates/canopy-core` | Platform-neutral model: layout tree, solver, drop resolution, camera and history, URL commands, persistence. Its tests are organised by SPEC section |
| `crates/canopy/src/mac` | Everything that talks to macOS: background Accessibility queues, event tap on its own thread, Carbon hotkeys, ScreenCaptureKit pictures, panels, and the AppKit menu bar and alerts, which GPUI doesn't have |
| `crates/canopy/src/ui` | Everything Canopy draws, in GPUI: the motion stage, glass, overlays, setup and settings |
| `crates/canopy/src/coordinator` | Ties windows, input, layout and motion together, on the main thread |
| `crates/canopy-test-windows` | The test-window app for the end-to-end test |
| `Tests/E2E` | End-to-end test |
| `legacy-swift` | The previous Swift and AppKit version, kept for reference; nothing builds it |
