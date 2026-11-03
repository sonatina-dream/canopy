# Canopy: specification

Canopy is a macOS window manager that docks real app windows side by side, nests
them as deep as you like, and lets you zoom a camera in and out over the
arrangement. This spec comes from an investigation of TrellisWM 0.9.8
(https://trelliswm.app). It describes behavior only and contains none of
TrellisWM's code or assets. Canopy is a separate implementation with its own
name, URL scheme (`canopy://`) and bundle identifier.

## 1. Evidence

All evidence comes from public material: the website and the notarized DMG,
which was mounted read-only and never run.

| ID | Source | What it established |
|----|--------|---------------------|
| E1 | `https://trelliswm.app`, `/docs/`, `/how-it-works/`, `/download/` (fetched 2026-10-29) | Feature list, every shortcut, every URL command, settings pane names, Free/Pro split, uninstall paths |
| E2 | `TrellisWM-0.9.8.dmg`, sha256 `82c54c77…ec003`, REA `inspect_artifact` (`ev_1effb61f…`, parent `ev_6d575e42…`) | Bundle layout: one arm64 executable, Sparkle, `ReleaseSettings.json`, `Localizable.strings`, Lessons |
| E3 | `Contents/Info.plist` | Bundle id `app.trellisui.trelliswm`, URL scheme `trelliswm`, `LSUIElement` (menu-bar only), macOS 14 minimum |
| E4 | `Contents/Resources/ReleaseSettings.json` | Default policy values, e.g. `gap: 12`, `shareRemovedSpace: true`, `redrawWait: 0.5`, `desktopEdgeStrength: 0.05`, `previewStyle: tinted`, `stepTowardPointer: false` |
| E5 | Executable sha256 `eeb7ab75…63d1`, REA `inspect_macho` (`ev_4e62fa61…`, parent `ev_0b5e069f…`), `otool -L`, `nm -u` | Frameworks (AppKit, ApplicationServices, ScreenCaptureKit, QuartzCore, Carbon) and imported APIs (below) |
| E6 | Swift nominal type descriptors and `__swift5_reflstr` field names | Module split `SpacesModel` / `SpacesEngine` / app; type names such as `SplitNode`, `PanelNode`, `DesktopNode`, `RestoreTarget`, `Visit`, `Navigator`, `DropResolver`, `CameraCurve`, `StagePanel`, `TabStripPanel`, `GapHandleWindow` |
| E7 | `en.lproj/Localizable.strings` (419 entries) and binary strings | UI wording and mechanisms: `com.apple.WindowManager` keys `EnableTilingByEdgeDrag`, `EnableStandardClickToShowDesktop`, `GloballyEnabled`; `AXEnhancedUserInterface`; `AXFullScreenButton` |
| E9 | TrellisWM 0.9.8 symbol names and disassembly of `DropResolver` (2026-10-31), its saved arrangement, E7 | `resolve` runs `resolveUnscoped`, then `onFramedEdge`, `scopedToRange` and `frameEdgeTarget`: a screen edge is the edge of the framed node or range. `resolveUnscoped` walks the floating panels from the top down before any tile, with no edge zones. A floating panel can hold tabs (`tabbed`; E7 "floating or docked") |
| E8 | TrellisWM 0.9.8 running on this Mac (2026-10-30): `~/Library/Logs/TrellisWM.log`, its preferences and saved arrangement, its windows in `CGWindowList`, imports such as `NSThread`, `CFRunLoopRun`, `CADisplayLink`, `SCStream` and `dlopen(MultitouchSupport)` | Runtime behaviour: Accessibility calls to Finder take 120–150 ms (`AX slow`) while its 60 Hz frame loop drops almost no frames; a drag puts the stage up (`MOTION stay`) and the drop lands on it (`MOTION land`, about 0.7–1 s including redraw); a press interrupts a landing; windows are parked shrunk at the bottom-right corner (`MINIMUM … refused (1439, 899, 480×128)`); minimum sizes are saved per app; ⌘⇧-drag frames a *range* of siblings; the stage is a level-19 panel over everything below the menu bar; gap handles are 22 pt wide; `SetsCursorInBackground` is set at launch |

Imported APIs (E5): `AXObserver*`, `AXUIElementCopyAttributeValue`/`SetAttributeValue`/
`PerformAction`/`CopyElementAtPosition`/`SetMessagingTimeout`, private
`_AXUIElementGetWindow`, `CGEventTapCreate`/`CGEventPost`, `CGWindowListCopyWindowInfo`,
`CGDisplayCreateUUIDFromDisplayID`, `CGPreflightScreenCaptureAccess`/`CGRequestScreenCaptureAccess`,
`SCStreamFrameInfoStatus`, `CGSSetConnectionProperty`, `TISCopyCurrentKeyboardLayoutInputSource`
and `UCKeyTranslate` (shortcut labels), `NSApplicationDidChangeScreenParametersNotification`.
No `RegisterEventHotKey` import was seen, so TrellisWM probably catches shortcuts through its
event tap.

**Observed** means taken directly from E1–E7. **Inferred** means reasoned from those
sources but not seen running. **Unknown** means not established. No runtime
observation of TrellisWM was done.

## 2. Concepts

- **Display world.** Every display has its own arrangement, camera, history
  and saved views on every macOS Space (desktop). E1 says "Each display has its
  own spaces and camera", and E8 shows TrellisWM saving
  `arrangement-<display>|desktop-…json` and logging `SPACE … leaves / returns`.
  Worlds are keyed by display UUID (E5 `CGDisplayCreateUUIDFromDisplayID`) plus
  the Space's uuid ("main" for the first desktop).
- **Arrangement.** A tree of nodes (E6). A *split* has an axis (horizontal = children
  left to right, vertical = top to bottom), ordered children and weights that sum
  to 1. A *tile* holds one or more windows; more than one window makes a tab
  group, with one tab selected. A *desktop* node is the free-floating Desktop
  region. Every world has exactly one desktop node. A fresh world is just the desktop.
- **Docked and floating.** Docked windows sit in tiles and never overlap.
  Floating windows aren't in the tree and live on the Desktop (E1).
- **Framing (camera).** The camera frames one node, and that node's subtree is laid
  out to fill the display's visible area. Framing the root means "everything".
  Windows outside the framed subtree are parked out of view (E4 `parkOutOfViewSmall`).
- **Cramped.** If a tile's on-screen window rect is smaller than the window's
  minimum size, or smaller than 160×100 pt, the real window is parked and a
  cover shows the app's colour and icon (plus its latest picture when allowed)
  until there's room. Double-clicking a cover frames it ("go in"). (E1, E4 `squeezeBelowMinimum`.)

## 3. Layout

Inputs: tree, framed node, display visible rect (top-left origin, y down), gap `g`
(default 12, E4). Output: a rect per node.

- The root rect is the visible rect inset by `g`.
- A split divides its rect along its axis. Available length = length − `g`·(k−1)
  for k visible children, and child i gets `available · wᵢ / Σw`. Edges are
  rounded to whole points, and the last child absorbs rounding.
- A hidden desktop gets no space. Its siblings share the length by their weights.
- In a tab group, the tile's top 28 pt is the tab strip and the selected window
  fills the rest. A single-window tile has no strip.
- Seams (the gaps between neighbouring children) are the drag handles for
  resizing. Dragging a seam by Δ pt moves weight `Δ/available·Σw` from one
  neighbour to the other. Neither neighbour can drop below 5% of the split or 60 pt.

## 4. Docking

`dock(window, beside: target, edge, fraction?)`:

- If the target's parent is a split on the edge's axis, the new tile is inserted
  next to the target (before it for left/top, after it for right/bottom). It gets
  `fraction ?? 1/(n+1)` and the other siblings scale by `(1 − f)`, so the
  neighbours make room (E7 "Docked. Its neighbours made room.").
- Otherwise the target is replaced by a new split on the edge's axis holding
  `[target, new]` (or `[new, target]`). Weights are `[1−f, f]`, f defaults to ½.
  If the target is itself a split on the same axis, its children are flattened
  into the new split, so f defaults to `1/(n+1)`.
- **Screen edge** = dock beside what's in view ("beside everything you're looking
  at", E1, E9): the framed node, or the near end of a framed range. Across a
  range it goes beside the range's whole split. A window that is the whole view
  docks beside the root. This is how a window gets into a space from the inside:
  zoom in, and drag a window to the screen edge.
- After a dock the view widens just enough to include the window. Right next to
  the view that is a range of the two, so the rest of the split stays out of it.
- Moving a whole tile (a dragged tab group) detaches it and inserts it the same way.

`remove(window)`:

- If the tile still has other tabs, only the tab goes. The selection moves to
  the next tab, or the previous one if it was last.
- Otherwise the tile leaves its split. With `shareRemovedSpace` (default on, E4)
  every sibling scales by `1/(1−w)`, so closing exactly undoes adding (E7).
  With it off, the next neighbour (or the previous one, if the tile was last)
  takes all the space (E7).
- A split left with one child collapses into its parent. A child split on the
  same axis as the parent is flattened into it.
- The result is a **restore target** (E6 `RestoreTarget`): `docked(besideNode, edge,
  fraction, fallbackWindow)`, `tab(inWindow, index)` or `floating`. Minimise saves
  it and restoring re-applies it, so the window "returns to its spot" (E1). If the
  neighbour node is gone, the fallback window's tile is used. If that's gone too,
  the window is auto-docked.

**Auto-dock** (Arrange, or the green button on a floating window with no restore
target): split the largest visible tile along its longer side. If nothing is
docked yet, dock at the left screen edge.

## 5. Drop resolution

Inputs: pointer, display visible rect, candidate rects for tiles and the desktop
(the dragged tile itself excluded), whether the pointer has rested in a tile's
centre for the hold time (default 0.5 s), and the floating window it has come to
rest on, if any. Output, first match wins:

1. Pointer within the screen-edge band (max(24 pt, 5% of the shorter side, from E4
   `desktopEdgeStrength`)) of the visible rect: `screenEdge(nearest edge)`. A
   pointer pushed past the visible rect into the menu bar or the Dock counts
   as that edge.
2. Pointer resting on a floating window that nothing else is in front of: the
   same zones as a tile give `floating(window, edge)` or `floating(window, tab)`.
   Windows are moved over each other on the Desktop all the time, so this takes
   a real stop: the pointer has to stay within 8 pt for the hold time, and however
   slowly a window passes over another, nothing docks. Once reached, it lasts
   while the pointer stays over that window.
3. Pointer in a tile: normalised position u, v. Edge zones are the outer 25% on
   each side, and the nearest edge wins when zones overlap. The centre zone gives
   `tab(tile)` once held (E1 "pause for a moment"), and `none` before that.
4. Pointer in the desktop: within 40 pt of a desktop edge that borders docked
   tiles gives `beside(desktop, edge)`. Anywhere else gives `float`.
5. Otherwise `none`.

What happens on drop: `screenEdge`, `beside` and `tab` dock. `floating` docks the
floating window too, along the longer side of the Desktop on the side it is nearer
(beside the view when the Desktop is out of it), with the dragged window beside it
or as its tab. `float` makes the window
float (a docked window leaves the tree and is placed where it was dropped). `none`
means a floating window stays where it is and a docked window snaps back. A drop
preview shows the exact rect the window will get, worked out by running the drop
on a copy of the arrangement.

## 6. Navigation

All of these push the current view onto **back** and clear **forward**, like a
browser (E1). A *visit* stores the framed node id plus the windows inside it, so
it still resolves after node ids change (falling back to the lowest common
ancestor of surviving windows, then the root).

| Action | Behaviour |
|---|---|
| Everything | Frame the root. |
| Step out | Frame the parent. Does nothing at the root. |
| Step in | Frame the child of the framed split that contains the focused window (E4 `stepTowardPointer: false`), else the one under the pointer, else the first child. Does nothing on a tile. |
| Neighbour left/right/up/down | Walk up to the nearest split on that axis with a sibling in that direction, then walk down that sibling to the framed node's depth. Same-axis splits pick the child nearest the old view, and cross-axis splits pick the child closest to the old view's centre. |
| Frame window | Frame the focused window's tile. Doing it again goes back (E1). |
| Frame desktop | Frame the desktop node. Doing it again goes back. |
| Toggle desktop | Hide or show the desktop node. While it's hidden, floating windows on that display are parked. |
| Back / forward | Pop the history stacks. |
| Save view n (1–9) | Store the current visit, per display (E1 "9 saved views per display"). |
| Go to view n | Frame the stored visit. Does nothing if nothing is saved. |
| Zoom to rect (⌘⇧-drag) | Take the tiles the rectangle covers a quarter of, that hold a fifth of it, or that hold its centre. When they are some of one split's children, frame just that contiguous *range* (E8). Otherwise frame their lowest common ancestor. Esc cancels. |
| ⌥-pinch | The stage zooms live around where the pinch began (scale = e^(sum of magnification)). Windows outside the view are drawn where the camera would see them. On lift, the camera settles on whichever node has an area zoom to fill the screen closest to the pinch. The candidates are the nodes under the pointer from the root down, plus the current view's ancestors. A pinch within ±12% stays put. |
| Double-click title bar | Frame that window's tile. Doing it again goes back (E1). ⌥-double-click passes through to the app. |
| Double-click bare Desktop | Frame the desktop. Doing it again goes back. |
| Focus a parked window | Frame `LCA(framed, tile)`, then step down toward the tile until it isn't cramped. A hidden tab gets selected. |

## 7. Tab groups

- Dropping on a tile's centre after the hold adds the window as a tab and
  selects it (E1).
- The strip shows one tab per window (app icon and title, with the selected one
  highlighted). Clicking a tab selects it.
- Unselected tabs wait directly behind the selected one, at the same frame, so
  switching only changes which is in front. Nothing moves or resizes, so no app
  has to redraw and nothing blinks. A tab whose minimum size doesn't fit the tile
  would stick out, so it is parked at a corner instead. When a group is out of
  view or shown as a cover, all its windows are parked.
- Dragging a tab out tears it off: the drop resolver applies, and `none` floats
  the window at the drop point (E1 "Tabs can be torn off").
- The right-click menu has Close Tab, Move Tab to New Window (float) and
  Minimize Tab (E7).

## 8. Window lifecycle

- If another window manager is running (TrellisWM, AeroSpace, Rectangle and
  so on), Canopy says so and offers to quit it. Two managers fight over every
  window (E8: TrellisWM adopts every new window and moves it back).
- Minimum sizes: a window that refuses a size teaches its own minimum. The
  app's largest learned minimum is saved, but only as a floor for parking,
  because windows of one app can differ.
- Windows that are managed: role `AXWindow`, subrole `AXStandardWindow`, not
  minimised, not full screen, owned by a regular app that isn't excluded. Sheets
  and dialogs are left to macOS (E1).
- New windows start floating. Nothing moves until the user docks a window (E1).
- Closed: removed and the space is shared. Minimised: removed with a restore
  target. Unminimised: restored.
- When the user resizes a docked window by its edge, the matching seam moves to
  follow it. If there's no seam on that side, or something else moved the window,
  it snaps back.
- Green button: dock ⇄ float. ⌥-click passes through to the native zoom (E1).
- When moving windows, `AXEnhancedUserInterface` is switched off on the app
  around the move and restored afterwards (E7), so apps like browsers don't
  animate or reject the move.
- Excluded apps ("Don't Arrange", E1) are never docked, and their docked
  windows float.
- **Float All** undocks every window and brings it back into view (E1).
- **Quit** brings every parked window back to a visible spot before exiting,
  and also on SIGTERM and SIGINT (E1 "Quitting returns all windows to visible
  positions"). Arrangements are saved to
  `~/Library/Application Support/Canopy/arrangements.json` and restored on
  launch. Windows are matched by window id first, then by bundle id and title
  (E1 "persist across restarts"). Windows that can't be matched are removed.

Range framing: a range visit stores the split and the ids of the framed
children. The run is laid out as if it were the whole split. Stepping out frames
the whole split. A neighbour is found from the run's first or last child.
If the run stops being contiguous, the whole split is framed.

## 9. Motion and previews

- **Threading (E8):** Accessibility is slow, so nothing on the main thread
  waits for it. Every AX call runs on a serial queue per app. Those threads reach
  the main thread only as messages. Frame writes are
  coalesced, so the newest target wins. Window frames come from `CGWindowList`
  (about 0.4 ms, against 120–170 ms for the same frames through AX). The event
  tap and the AX observers each run on their own thread with a run loop.
- **The event tap never waits for an app.** macOS holds every mouse event on the
  Mac until the tap answers, so nothing in it uses Accessibility. Each window's
  green button is read ahead of time, on its app's queue. The first click in a
  title bar looks ahead on a queue of its own; a second click that beats the
  answer counts as on the title bar.
- **Timers stay off the main thread's back.** Periodic work (pictures, the Space
  check, app icons) runs on workers. Asking whether Screen Recording is allowed
  is a round trip to another process (about 13 ms), so only the capture worker
  asks. Canopy opts out of App Nap for as long as it runs; the Mac can still sleep.
- **Nothing can leave a frozen screen.** A stage is a picture over the whole
  display, so a drag whose mouse-up never came ends by itself within 200 ms, a
  pinch after 3 s without news, and any stage nothing is moving after 4 s.
- **Stalls name themselves.** Main-thread work over 30 ms logs `SLOW` and what it
  was. A `STALL` with no `SLOW` beside it means macOS didn't run Canopy in time.
- **Live drag:** when a title-bar drag starts, the stage goes up with every
  window on that display, the dragged one on top under the pointer. As the drop
  target changes, the other pictures slide to where the drop would put them
  (0.25 s) and a slot outlines the landing place. On release the layout lands
  on the same stage.
- **Landing:** real windows are not written until the stage's first frame is on
  screen, so they never jump before their pictures cover them; the motion's clock
  starts on that frame. The stage stays up until the real windows have been
  written (at most 1 s), plus the redraw wait. A mouse press during motion ends it
  at once.
- **Pictures:** each picture keeps its top-left fixed while its window is
  resized. Past the picture, the window's own edge colour fills in, so a
  growing window never shows a solid block of colour. A picture only scales down
  when its window shrinks below half its size. Windows without a picture show
  as grey with a quarter of the app's colour (E4 `appColourAmount: 0.25`).
- **Desktop and glass:** the stage stands on the desktop as it is drawn,
  captured with every window left out (E6 `WallpaperCapture`). The wallpaper
  file macOS reports can differ from what is on screen (here
  `DefaultDesktop.heic` against a redwood forest). Behind arranged windows, and
  during motion, the desktop is shown blurred 20 pt and darkened 30% (E4
  `dimBlur`, `dimOpacity`), except the Desktop's own space, which stays clear.
  The clear area slides with each change. At rest, the glass is a panel just
  above the desktop icons.

- Layout and camera changes animate over 0.46 s (E1 "460ms layout motion").
- **Stage.** A borderless overlay over the display's visible area shows the
  wallpaper and a picture of every visible window, animated from the old rect to
  the new one. The real windows move to their final frames underneath. The
  overlay fades out after the motion plus a short redraw wait (E4 `redrawWait`).
- Pictures are ScreenCaptureKit stills of each window (E5, E1 "Motion uses still
  pictures; nothing is recorded or stored"). They're refreshed every 2 s for
  visible managed windows and kept in memory only. Without Screen Recording
  permission, pictures fall back to the app's colour and icon (E1).
- Covers for cramped windows show the latest picture tinted with the app colour
  (E4 `previewStyle: tinted`), with the app icon on top.

## 10. Spaces (desktops)

- The current Space of each display comes from the window server
  (`CGSCopyManagedDisplaySpaces`). When the user switches desktops, each
  display shows its world for the new Space. A saved arrangement comes back
  half a second later, once that Space's windows have shown up. A full-screen
  app's Space has no world, and Canopy leaves it alone.
- A world on another Space keeps its arrangement but writes nothing; it is laid
  out again when its Space returns. Floating windows count only when they are on
  screen now, or when this world parked them.
- Every 3 s Canopy checks that each docked window is still on its world's
  Space (`CGSCopySpacesForWindows`). A window moved to another desktop leaves
  the arrangement and floats on its new Space.
- Covers, tab strips, gap handles and the glass belong to their Space, as the
  windows they decorate do. The stage and the drop preview appear on whichever
  Space is current.
- Arrangements saved per display before Spaces were kept apart become the
  current Space's.

## 11. Displays

- One world per display, keyed by display UUID, rebuilt when screen parameters
  change (E5).
- When a display is disconnected, its world is kept aside and its windows float
  onto a remaining display. When the same display reconnects, the world comes back
  and any surviving windows are docked again.
- Commands act on the display of the focused window, or the display under the
  pointer if there's no focused window.

## 11. Commands by URL

Scheme `canopy://`. The command is the URL host, or the path if there's no host. Case doesn't matter.

| URL | Command |
|---|---|
| `canopy://everything` | Everything |
| `canopy://zoom-out` / `zoom-in` | Step out / in |
| `canopy://back` / `forward` | History |
| `canopy://neighbour?dir=left\|right\|up\|down` (also `neighbor`) | Neighbour |
| `canopy://frame-window` / `frame-desktop` / `toggle-desktop` | As in §6 |
| `canopy://view?n=1…9` / `save-view?n=1…9` | Saved views |
| `canopy://split-finder` | Open a new Finder window and dock it beside the focused window |
| `canopy://arrange` / `float-all` | Arrange or float all |
| `canopy://settings?pane=general\|windows\|tabs\|appearance\|gestures\|apps\|shortcuts` | Open Settings (an unknown or missing pane opens General) |

Invalid URLs are ignored: an unknown command, a missing or non-numeric `n`,
`n` outside 1–9, or a bad `dir`. Every command has a `url` that round-trips.

## 12. Shortcuts (defaults, rebindable)

⌃⌥Space everything · ⌃⌥↑/↓ step out/in · ⌃⌥←/→ neighbour left/right ·
⌃⌥⇧↑/↓ neighbour up/down · ⌃⌥[ / ] back/forward · ⌃⌥↩ frame window ·
⌃⌥D toggle desktop · ⌃⌥N split with Finder · ⌃⌥⇧1–9 save view · ⌃⌥1–9 go to view ·
⌘⇧-drag zoom to rect · ⌥-pinch zoom (E1). A shortcut needs ⌃, ⌥ or ⌘ (E7).

## 13. Menu bar and settings

- Menu-bar-only app. The controls are Back, Forward, Everything, and one numbered
  button per saved view. Any right-click opens the main menu (E1).
- Main menu: Show Everything, Back, Forward, Saved Views, Hide/Show Desktop,
  Arrange Windows, Float All Windows on Desktop, Don't Arrange "<app>" or
  Arrange "<app>" Again, Settings…, Quit (E1, E7).
- Settings panes:
  - General: permissions, macOS features, launch at login
  - Windows: gap, animation, share removed space, tab hold time
  - Tabs: close buttons
  - Appearance: menu bar controls, cover tint
  - Gestures: ⌥-pinch, ⌘⇧-drag
  - Apps: excluded apps
  - Shortcuts: rebinding
- macOS features: Canopy can switch off edge tiling, click-wallpaper-to-reveal
  and Stage Manager (`com.apple.WindowManager` keys, E7), remember the previous
  values, and switch them back on (E1).

## 14. Where Canopy deliberately differs

- **No licensing or limits.** Everything TrellisWM sells as Pro is always on:
  unlimited docked windows, tab groups, saved views and URL commands.
- **Pinch reads standard magnify events,** not raw multitouch through the
  private MultitouchSupport framework (E8), so a pan isn't part of the gesture.
- **Left out:** Mission Control replacement, Spaces swiping, the Getting Started
  lessons, Sparkle updates and the Report a Problem mailer.
- **Shortcuts** use Carbon `RegisterEventHotKey`, which doesn't need Input
  Monitoring. TrellisWM appears to use its event tap.
- **Drops on floating windows.** TrellisWM's floating panels hold their own tab
  groups and take nothing but tabs (E9). Canopy's tab groups are always docked, so
  a pause on a floating window docks it, and its edges work too: Canopy has no
  other way to put a window into a floating one.
- **Windows version: not built.** `canopy-core` (model, layout, navigation,
  commands, persistence) has no platform code, so a Windows port could reuse it.
  The macOS layer (AX, ScreenCaptureKit, AppKit) would need a Win32 counterpart.
- **Drawing is GPUI, not Core Animation.** The stage is drawn by the app on the
  main thread, once per display refresh. Core Animation played motion in the
  window server, where a busy main thread couldn't touch it; here, a main-thread
  stall drops frames. Keeping the main thread free matters more than it did.

## 15. Unknowns

- TrellisWM's exact drop-zone proportions, hold time, and edge-band formula.
  The values in §5 are Canopy's own (E4 only gives `desktopEdgeStrength: 0.05`).
- How TrellisWM measures a window's minimum size (E6 `probedMinimums`). Canopy
  probes by asking for a tiny size and reading back what the window accepted.
- Exactly what "neighbouring space" means in TrellisWM. Canopy uses the spatial
  rule in §6. It isn't macOS Spaces, because TrellisWM has an up/down variant.
- What `CGSSetConnectionProperty` is used for. Canopy doesn't use it.
