#!/usr/bin/env python3 -I
"""End-to-end test of Canopy against real windows.

Bundles a throwaway app with four windows (W2 answers as slowly as Finder,
W3 refuses to be smaller than 700x600), runs `Canopy --harness` so that only those windows are managed, and
checks each behaviour in SPEC.md against real Accessibility frames.

    scripts/build-app.sh && python3 -I Tests/E2E/e2e.py

Needs: build/Canopy.app and target/release/CanopyTestWindows (the script builds
both), and a terminal that has the Accessibility permission.
Your other windows are never touched; the test windows move around for ~30 s.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CANOPY = os.path.join(ROOT, "build", "Canopy.app", "Contents", "MacOS", "Canopy")
BUNDLE = "app.canopy.e2e.windows"
GAP = 12
STRIP = 36

passed, failed = 0, 0


def check(name, ok, detail=""):
    global passed, failed
    if ok:
        passed += 1
        print(f"  ✔ {name}")
    else:
        failed += 1
        print(f"  ✘ {name}  {detail}")


def build_test_app(tmp):
    app = os.path.join(tmp, "CanopyTestWindows.app", "Contents")
    os.makedirs(os.path.join(app, "MacOS"))
    with open(os.path.join(app, "Info.plist"), "w") as f:
        f.write(f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE}</string>
<key>CFBundleName</key><string>CanopyTestWindows</string>
<key>CFBundleExecutable</key><string>CanopyTestWindows</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>""")
    exe = os.path.join(app, "MacOS", "CanopyTestWindows")
    shutil.copy(os.path.join(ROOT, "target", "release", "CanopyTestWindows"), exe)
    return exe


class Proc:
    def __init__(self, args, env=None):
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  text=True, env=env)

    def send(self, line):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def readline(self, timeout=10):
        start = time.time()
        while time.time() - start < timeout:
            line = self.p.stdout.readline()
            if line:
                return line.strip()
        raise TimeoutError("no reply")

    def ask(self, line):
        self.send(line)
        return json.loads(self.readline())

    def close(self):
        try:
            self.send("quit")
        except BrokenPipeError:
            pass
        try:
            self.p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.p.kill()


def near(a, b, tol=3):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def main():
    others = subprocess.run(["pgrep", "-x", "TrellisWM"], capture_output=True).stdout.strip()
    if others:
        sys.exit("TrellisWM is running and will fight over the test windows: quit it first")
    if not os.path.exists(CANOPY):
        sys.exit("build/Canopy.app is missing: run scripts/build-app.sh first")
    tmp = tempfile.mkdtemp(prefix="canopy-e2e-")
    env = dict(os.environ, CANOPY_HARNESS_DIR=os.path.join(tmp, "state"))
    windows = Proc([build_test_app(tmp), "W1", "W2:slow", "W3:700x600", "W4"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE], env=env)
    ready = json.loads(canopy.readline())
    if not ready.get("ready"):
        windows.close()
        sys.exit(f"harness did not start: {ready}")
    time.sleep(1.5)

    def run(cmd, wait=0.6):
        r = canopy.ask(cmd)
        if not r.get("ok"):
            check(f"command accepted: {cmd}", False, r)
        time.sleep(wait)
        return dump()

    def dump():
        d = canopy.ask("dump")
        d["w"] = {w["title"]: w for w in d["windows"]}
        d["world"] = d["worlds"][0]
        return d

    def frames_follow_plan(d):
        bad = []
        for title, rect in d["world"].get("visibleWindows", {}).items():
            if not near(d["w"][title]["frame"], rect):
                bad.append((title, d["w"][title]["frame"], rect))
        return bad

    d = dump()
    vx, vy, vw, vh = d["world"]["visible"]
    bx, by, bw, bh = vx + GAP, vy + GAP, vw - 2 * GAP, vh - 2 * GAP
    canopy.ask("stall")  # start measuring after launch

    print("Floating by default (§8)")
    check("four test windows are tracked", sorted(d["w"]) == ["W1", "W2", "W3", "W4"], sorted(d["w"]))
    check("nothing is docked until you dock it", d["world"]["tree"] == "D", d["world"]["tree"])

    print("Docking (§4) and layout (§3)")
    d = run("dock W1 screen left")
    check("screen edge splits with the Desktop", d["world"]["tree"] == "H(W1 D)", d["world"]["tree"])
    f = d["w"]["W1"]["frame"]
    check("W1 fills the left half inside the gap", near(f, [bx, by, (bw - GAP) / 2, bh], 2), f)
    d = run("dock W2 W1 right")
    check("beside a window: neighbours make room", d["world"]["tree"] == "H(W1 W2 D)", d["world"]["tree"])
    third = (bw - 2 * GAP) / 3
    check("three equal columns", near([d["w"]["W1"]["frame"][2], d["w"]["W2"]["frame"][2]], [third, third], 2),
          [d["w"]["W1"]["frame"][2], d["w"]["W2"]["frame"][2], third])
    check("real frames follow the plan", not frames_follow_plan(d), frames_follow_plan(d))
    check("a gap handle for each seam", d["world"]["seams"] == 2, d["world"]["seams"])

    print("Nesting and cramped windows (§2)")
    d = run("dock W3 W2 bottom", wait=1.2)
    check("cross-axis dock nests a split", d["world"]["tree"] == "H(W1 V(W2 W3) D)", d["world"]["tree"])
    check("W3 is too small for its minimum: shown as a cover", d["world"].get("covers") == ["W3"], d["world"].get("covers"))
    check("…and its real window is parked out of view", d["w"]["W3"]["parked"], d["w"]["W3"])
    overlays = canopy.ask("overlays")["overlays"]
    check("every overlay draws at the size of its panel",
          overlays and all(near(o["viewport"], o["window"], 0.5) for o in overlays), overlays)
    d = run("frame W3", wait=1.0)
    check("framing W3 gives it the screen", d["world"]["framed"] == "W3", d["world"]["framed"])
    check("W3 is real again, at full size", near(d["w"]["W3"]["frame"], [bx, by, bw, bh], 2) and not d["w"]["W3"]["parked"],
          d["w"]["W3"])
    check("windows outside the view are parked", d["w"]["W1"]["parked"] and d["w"]["W2"]["parked"])

    print("Back and forward (§6)")
    d = run("url canopy://back", wait=1.0)
    check("back returns to everything", d["world"]["framed"] == "H(W1 V(W2 W3) D)", d["world"]["framed"])
    check("W1 is back in place", not d["w"]["W1"]["parked"] and not frames_follow_plan(d), frames_follow_plan(d))
    d = run("url canopy://forward", wait=1.0)
    check("forward goes to W3 again", d["world"]["framed"] == "W3", d["world"]["framed"])
    d = run("url canopy://everything", wait=1.0)

    print("Tab groups (§7)")
    d = run("tab W4 W1")
    check("W4 joins W1's group", d["world"]["tree"] == "H([W1,W4] V(W2 W3) D)", d["world"]["tree"])
    f4 = d["w"]["W4"]["frame"]
    check("the new tab is shown below the strip", abs(f4[1] - (by + STRIP)) <= 2 and not d["w"]["W4"]["parked"], f4)
    check("the other tab is parked", d["w"]["W1"]["parked"])
    check("…behind the new one, at the same frame", near(d["w"]["W1"]["frame"], f4, 1), [d["w"]["W1"]["frame"], f4])
    canopy.ask("log")
    d = run("select W1")
    check("selecting a tab swaps them", not d["w"]["W1"]["parked"] and d["w"]["W4"]["parked"])
    moves = [l for l in canopy.ask("log")["log"] if " WRITE " in l or " PARK " in l]
    check("switching tabs moves no window at all, so nothing blinks", not moves, moves)
    check("…the selected tab comes to the front", d["w"]["W1"]["depth"] < d["w"]["W4"]["depth"], d["windows"])

    print("Saved views (§6)")
    run("frame W2", wait=1.0)
    run("url canopy://save-view?n=3", wait=0.2)
    run("url canopy://everything", wait=1.0)
    d = run("url canopy://view?n=3", wait=1.0)
    check("view 3 comes straight back", d["world"]["framed"] == "W2" and d["world"]["savedViews"] == [3],
          [d["world"]["framed"], d["world"]["savedViews"]])
    run("url canopy://everything", wait=1.0)

    print("Neighbouring spaces (§6)")
    run("frame W1", wait=1.0)
    d = run("url canopy://neighbour?dir=right", wait=1.0)
    check("right of W1's group is the W2/W3 column", d["world"]["framed"] == "V(W2 W3)", d["world"]["framed"])
    d = run("url canopy://neighbour?dir=right", wait=1.0)
    check("then the Desktop", d["world"]["framed"] == "D", d["world"]["framed"])
    run("url canopy://everything", wait=1.0)

    print("Hide the Desktop (§6)")
    d = run("url canopy://toggle-desktop")
    check("Desktop hidden", d["world"]["desktopHidden"])
    half = (bw - GAP) / 2
    check("docked windows take its space", abs(d["w"]["W1"]["frame"][2] - half) <= 2, d["w"]["W1"]["frame"])
    d = run("url canopy://toggle-desktop")
    check("Desktop shown again", not d["world"]["desktopHidden"] and abs(d["w"]["W1"]["frame"][2] - third) <= 2,
          d["w"]["W1"]["frame"])

    print("Minimise returns to its spot (§4)")
    d = run("minimize W2", wait=1.5)
    check("minimised: neighbours take the space", d["world"]["tree"] == "H([W1,W4] W3 D)", d["world"]["tree"])
    d = run("restore W2", wait=1.5)
    check("restored to its spot", d["world"]["tree"] == "H([W1,W4] V(W2 W3) D)", d["world"]["tree"])

    print("Green button: dock ⇄ float (§8)")
    d = run("toggle W2", wait=1.0)
    check("floats out of the layout", d["world"]["tree"] == "H([W1,W4] W3 D)" and not d["w"]["W2"]["docked"],
          d["world"]["tree"])
    d = run("toggle W2", wait=1.2)
    check("docks back where it was", d["world"]["tree"] == "H([W1,W4] V(W2 W3) D)", d["world"]["tree"])

    print("A tab too big for its group (§7)")
    run("tab W3 W1", wait=1.2)
    d = run("select W1", wait=0.8)
    check("it joins the group", d["world"]["tree"] == "H([W1,W4,W3] W2 D)", d["world"]["tree"])
    check("…but waits out of sight, not sticking out behind it",
          d["w"]["W3"]["parked"] and not near(d["w"]["W3"]["frame"][:2], d["w"]["W1"]["frame"][:2], 40), d["w"]["W3"])
    d = run("dock W3 W2 bottom", wait=1.2)
    check("…and docks back out", d["world"]["tree"] == "H([W1,W4] V(W2 W3) D)", d["world"]["tree"])

    print("Closing shares the space (§4)")
    d = run("close W3", wait=1.5)
    check("W3 closed: its split collapses", d["world"]["tree"] == "H([W1,W4] W2 D)", d["world"]["tree"])
    d = run("float W2", wait=1.0)
    check("dropping on the Desktop floats", d["world"]["tree"] == "H([W1,W4] D)", d["world"]["tree"])

    print("Responsiveness")
    stall = canopy.ask("stall")
    check("the main thread never stalled over 100 ms while arranging, even with a slow app",
          stall["worstStallMs"] < 100, stall)
    print(f"    (worst main-thread stall: {stall['worstStallMs']} ms)")

    print("Commands by URL (§11)")
    before = dump()["world"]
    d = run("url canopy://view?n=0", wait=0.3)
    check("an invalid URL changes nothing", d["world"]["tree"] == before["tree"] and d["world"]["framed"] == before["framed"])

    print("Quit returns every window to view (§8)")
    canopy.close()
    time.sleep(0.8)
    windows.send("frames")
    frames = json.loads(windows.readline())
    for title in ["W1", "W2", "W4"]:
        x, y, w, h = frames[title]["frame"]
        on = max(0, min(x + w, vx + vw) - max(x, vx)) * max(0, min(y + h, vy + vh) - max(y, vy))
        check(f"{title} is on screen after quit", on >= 0.5 * w * h, frames[title]["frame"])

    print("Arrangements persist across restarts (§8)")
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--keep-state"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.5)
    d = dump()
    check("the tab group is docked again", d["world"]["tree"] == "H([W1,W4] D)", d["world"]["tree"])
    check("and placed", not frames_follow_plan(d), frames_follow_plan(d))
    d = run("url canopy://float-all", wait=1.0)
    check("Float All empties the layout", d["world"]["tree"] == "D", d["world"]["tree"])
    check("…and nothing stays parked", not any(w["parked"] for w in d["windows"]), d["windows"])

    canopy.close()
    windows.close()
    tabs_phase(tmp, env)
    nesting_phase(tmp, env)
    if "--input" in sys.argv:
        input_phase(tmp, env)
    if "--spaces" in sys.argv:
        spaces_phase(tmp, env)
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


def nesting_phase(tmp, env):
    """Getting a window into other windows, with the stage on. `hover` and
    `release` run the real drag code on the main thread without posting a single
    mouse event, so this is safe with someone at the machine."""
    print("Floating windows as drop targets (§5)")
    exe = os.path.join(tmp, "CanopyTestWindows.app", "Contents", "MacOS", "CanopyTestWindows")
    windows = Proc([exe, "N1", "N2", "N3"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--animate"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.5)

    def dump():
        d = canopy.ask("dump")
        d["w"] = {w["title"]: w for w in d["windows"]}
        d["world"] = d["worlds"][0]
        return d

    def ask(cmd, wait=0.0):
        r = canopy.ask(cmd)
        if not r.get("ok"):
            check(f"command accepted: {cmd}", False, r)
        time.sleep(wait)

    def target():
        return canopy.ask("session").get("target")

    def wait_for(title):
        for _ in range(60):
            if title in dump()["w"]:
                return True
            time.sleep(0.1)
        return False

    d = dump()
    check("every window's green button is known before any click",
          all(w["greenButton"] for w in d["windows"]), [w["greenButton"] for w in d["windows"]])
    check("nothing is docked yet", d["world"]["tree"] == "D", d["world"]["tree"])

    ask("raise N1", 0.6)
    x, y, w, h = dump()["w"]["N1"]["frame"]
    mid_y = y + h / 2

    # Slower than the pause, but never still.
    for i in range(10):
        ask(f"hover N2 {x + 90 + i * 30} {mid_y}", 0.12)
    moving = target()
    ask(f"release {x + 360} {mid_y}", 0.5)
    check("moving a window across another docks nothing, however slowly",
          moving not in ("floating(tab)", "floating(left)", "floating(right)") and dump()["world"]["tree"] == "D", moving)

    ask("raise N1", 0.6)
    x, y, w, h = dump()["w"]["N1"]["frame"]
    mid_x, mid_y = x + w / 2, y + h / 2
    ask(f"hover N2 {mid_x} {mid_y}")
    check("arriving over a floating window changes nothing yet", not str(target()).startswith("floating"), target())
    time.sleep(0.8)
    check("a pause in its middle offers to make tabs", target() == "floating(tab)", target())
    ask(f"hover N2 {x + w - 20} {mid_y}", 0.1)
    check("…and its edge, to dock beside it", target() == "floating(right)", target())
    check("the dragged window shrinks so the target shows", canopy.ask("session").get("thumbnail"), canopy.ask("session"))
    ask(f"release {x + w - 20} {mid_y}", 1.4)
    d = dump()
    check("letting go docks them side by side", d["world"]["tree"] == "H(N1 N2 D)", d["world"]["tree"])
    check("…where the plan says", all(near(d["w"][t]["frame"], r) for t, r in d["world"]["visibleWindows"].items()),
          d["world"]["visibleWindows"])

    print("A new window into the space in view (§4)")
    ask("frame N1", 1.2)
    d = dump()
    vx, vy, vw, vh = d["world"]["visible"]
    check("zoomed in on N1", d["world"]["framed"] == "N1", d["world"]["framed"])
    windows.send("new N4")
    windows.readline()
    check("a window opened while zoomed in is tracked", wait_for("N4"), sorted(dump()["w"]))
    ask(f"hover N4 {vx + vw / 2} {vy + vh / 2}", 0.2)
    ask(f"hover N4 {vx + vw / 2} {vy + vh - 6}", 0.2)
    check("at the screen edge it offers to dock inside the view", str(target()).endswith("bottom)") and "beside" in str(target()), target())
    ask(f"release {vx + vw / 2} {vy + vh - 6}", 1.4)
    d = dump()
    check("it nests in the space", d["world"]["tree"] == "H(V(N1 N4) N2 D)", d["world"]["tree"])
    check("and the view stays on that space", d["world"]["framed"] == "V(N1 N4)", d["world"]["framed"])
    check("the rest stays out of view", d["w"]["N2"]["parked"], d["w"]["N2"])

    windows.send("new N5")
    windows.readline()
    check("another one is tracked", wait_for("N5"), sorted(dump()["w"]))
    ask(f"hover N5 {vx + vw / 2} {vy + vh / 2}", 0.2)
    ask(f"hover N5 {vx + vw - 6} {vy + vh / 2}", 0.2)
    ask(f"release {vx + vw - 6} {vy + vh / 2}", 1.4)
    d = dump()
    check("along the row it joins the row", d["world"]["tree"] == "H(V(N1 N4) N5 N2 D)", d["world"]["tree"])
    check("and the view takes it in, leaving the rest of the row out",
          d["world"]["framed"] == "range:V(N1 N4),N5" and d["w"]["N2"]["parked"], d["world"]["framed"])
    ask("url canopy://everything", 1.2)
    d = dump()
    check("zooming out shows it all, nested", d["world"]["framed"] == "H(V(N1 N4) N5 N2 D)" and not d["w"]["N2"]["parked"],
          d["world"]["framed"])

    print("A lost mouse-up can't leave a frozen screen")
    canopy.ask("log")
    ask(f"hover N3 {vx + 300} {vy + 300}", 0.3)
    check("the drag has begun", canopy.ask("session").get("dragging"), canopy.ask("session"))
    ask("lose-mouse-up", 0.8)
    log = canopy.ask("log")["log"]
    check("the drag ends by itself", not canopy.ask("session").get("dragging"), log[-6:])
    check("…and it says why in the log", any("mouse-up never came" in l for l in log), log[-6:])
    time.sleep(1.4)
    check("…and the stage comes down", not canopy.ask("stageShown").get("shown"), log[-6:])

    canopy.close()
    windows.close()


def tabs_phase(tmp, env):
    """Closing a tab hands focus to the window behind it, which needn't be the
    next tab in the strip. Whichever tab ends up selected must be the one in
    front, or the strip and the screen would disagree."""
    print("Closing the selected tab (§7)")
    exe = os.path.join(tmp, "CanopyTestWindows.app", "Contents", "MacOS", "CanopyTestWindows")
    windows = Proc([exe, "T1", "T2", "T3"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE], env=env)
    json.loads(canopy.readline())
    time.sleep(1.5)

    def ask(cmd, wait=0.8):
        r = canopy.ask(cmd)
        if not r.get("ok"):
            check(f"command accepted: {cmd}", False, r)
        time.sleep(wait)
        d = canopy.ask("dump")
        d["w"] = {w["title"]: w for w in d["windows"]}
        d["world"] = d["worlds"][0]
        return d

    ask("dock T1 screen left")
    ask("tab T2 T1")
    ask("tab T3 T1")
    ask("select T3")
    d = ask("select T1")
    frame = d["w"]["T1"]["frame"]
    d = ask("close T1", 1.2)
    check("the group carries on without it", d["world"]["tree"] == "H([T2,T3] D)", d["world"]["tree"])
    shown = [t for t in ("T2", "T3") if not d["w"][t]["parked"]]
    check("one tab is selected", len(shown) == 1, shown)
    hidden = "T3" if shown == ["T2"] else "T2"
    check("…and it is the one in front", d["w"][shown[0]]["depth"] < d["w"][hidden]["depth"], d["windows"])
    check("…at the same frame", near(d["w"][shown[0]]["frame"], frame, 1), [d["w"][shown[0]]["frame"], frame])
    canopy.close()
    windows.close()


def spaces_phase(tmp, env):
    """One arrangement per desktop. Needs a second desktop; switches to it
    with ⌃→ and back with ⌃← (about 10 s)."""
    print("Desktops (Spaces): one arrangement each")
    exe = os.path.join(tmp, "CanopyTestWindows.app", "Contents", "MacOS", "CanopyTestWindows")
    windows = Proc([exe, "One", "Two"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--animate"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.2)

    def dump():
        d = canopy.ask("dump")
        d["w"] = {w["title"]: w for w in d["windows"]}
        d["world"] = [w for w in d["worlds"] if w["active"]][0]
        return d

    canopy.ask("dock One screen left"); time.sleep(1.0)
    canopy.ask("dock Two One right"); time.sleep(1.2)
    d = dump()
    first_space = d["world"]["space"]
    home = {t: d["w"][t]["frame"] for t in ["One", "Two"]}
    check("desktop 1 is arranged", d["world"]["tree"] == "H(One Two D)", d["world"]["tree"])

    canopy.ask("key 124 ctrl"); time.sleep(1.8)
    d = dump()
    check("on the next desktop, its own empty arrangement", d["world"]["space"] != first_space and d["world"]["tree"] == "D",
          [d["world"]["space"], d["world"]["tree"]])
    check("…and desktop 1's windows are out of sight, untouched",
          not d["w"]["One"]["onScreen"] and near(d["w"]["One"]["frame"], home["One"]) and near(d["w"]["Two"]["frame"], home["Two"]),
          [d["w"]["One"], d["w"]["Two"]])
    windows.send("new Three")
    assert windows.readline() == "ok"
    time.sleep(1.2)
    canopy.ask("dock Three screen right"); time.sleep(1.2)
    d = dump()
    check("a window docks on desktop 2 by itself", d["world"]["tree"] == "H(D Three)", d["world"]["tree"])
    three = d["w"]["Three"]["frame"]

    canopy.ask("key 123 ctrl"); time.sleep(1.8)
    d = dump()
    check("back on desktop 1, its arrangement is as it was", d["world"]["space"] == first_space and
          d["world"]["tree"] == "H(One Two D)", [d["world"]["space"], d["world"]["tree"]])
    check("…with its windows in place", near(d["w"]["One"]["frame"], home["One"]) and near(d["w"]["Two"]["frame"], home["Two"]),
          [d["w"]["One"]["frame"], d["w"]["Two"]["frame"]])
    check("…and desktop 2's window left where it was", not d["w"]["Three"]["onScreen"] and near(d["w"]["Three"]["frame"], three),
          d["w"]["Three"])
    others = [w for w in d["worlds"] if not w["active"]]
    check("desktop 2's arrangement is kept", any(w["tree"] == "H(D Three)" for w in others), [w["tree"] for w in others])

    canopy.close()
    windows.close()


def input_phase(tmp, env):
    """Real mouse input through the event tap. Moves the pointer for ~15 s."""
    print("Real input: drag, green button, gap handle, double-click, hold-to-tab")
    exe = os.path.join(tmp, "CanopyTestWindows.app", "Contents", "MacOS", "CanopyTestWindows")
    windows = Proc([exe, "W1", "W2"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--tap", "--animate"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.5)

    def ask(cmd, wait=0.8):
        r = canopy.ask(cmd)
        if not r.get("ok"):
            check(f"command accepted: {cmd}", False, r)
        time.sleep(wait)
        d = canopy.ask("dump")
        d["w"] = {w["title"]: w for w in d["windows"]}
        d["world"] = d["worlds"][0]
        return d

    d = canopy.ask("dump")
    vx, vy, vw, vh = d["worlds"][0]["visible"]
    canopy.ask("stall")
    ask("raise W2", 0.5)
    d = ask(f"drag W2 {vx + 6} {vy + vh / 2}", 1.2)
    check("dragging a title bar to the screen edge docks it", d["world"]["tree"] == "H(W2 D)", d["world"]["tree"])
    ask("raise W1", 0.5)
    x, y, w, h = d["w"]["W2"]["frame"]
    d = ask(f"drag W1 {x + w * 0.9} {y + h / 2}", 1.2)
    check("dragging onto a window's right edge docks beside it", d["world"]["tree"] == "H(W2 W1 D)", d["world"]["tree"])
    d = ask("green W1", 1.2)
    check("the green button floats a docked window", d["world"]["tree"] == "H(W2 D)", d["world"]["tree"])
    ask("raise W1", 0.5)
    d = ask("green W1", 1.2)
    check("…and docks it back", d["world"]["tree"] == "H(W2 W1 D)", d["world"]["tree"])
    before = d["w"]["W2"]["frame"][2]
    d = ask("seam 0 80", 1.0)
    grew = d["w"]["W2"]["frame"][2] - before
    check("dragging the gap resizes both neighbours", 60 <= grew <= 100, f"grew {grew}")
    d = ask("dblclick W1", 1.2)
    check("double-clicking a title bar frames that window", d["world"]["framed"] == "W1", d["world"]["framed"])
    d = ask("dblclick W1", 1.2)
    check("…and again goes back", d["world"]["framed"] == "H(W2 W1 D)", d["world"]["framed"])
    ask("raise W1", 0.5)
    x, y, w, h = d["w"]["W2"]["frame"]
    # Mid-drag, before letting go: the tab preview must be showing.
    canopy.send(f"drag W1 {x + w / 2} {y + h / 2} 0.9")
    time.sleep(1.25)
    s = canopy.ask("session")
    check("over a window's middle, the tab preview shows before you let go",
          s.get("dragging") and s.get("target", "").startswith("tab") and s.get("slot") == "tab" and s.get("thumbnail"), s)
    json.loads(canopy.readline())
    time.sleep(1.2)
    d = canopy.ask("dump")
    d["world"] = d["worlds"][0]
    check("holding over a window's middle makes a tab group", d["world"]["tree"] == "H([W2,W1] D)", d["world"]["tree"])
    print("Frame it (⌘⇧-drag a rectangle)")
    canopy.close()
    windows.close()
    windows = Proc([exe, "M1", "M2", "M3"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--tap"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.2)
    canopy.ask("stall")
    canopy.ask("dock M1 screen left"); time.sleep(0.8)
    canopy.ask("dock M2 M1 right"); time.sleep(0.8)
    d = ask("dock M3 M2 right", 1.0)
    f1, f2 = d["w"]["M1"]["frame"], d["w"]["M2"]["frame"]
    # From inside M1 to inside M2: the camera fits those two.
    d = ask(f"marquee {f1[0] + f1[2] * 0.4} {f1[1] + f1[3] * 0.3} {f2[0] + f2[2] * 0.6} {f2[1] + f2[3] * 0.7}", 1.2)
    check("⌘⇧-dragging over two windows frames just those two", d["world"]["framed"] == "range:M1,M2", d["world"]["framed"])
    check("…and they fill the screen", abs(d["w"]["M1"]["frame"][0] - (vx + GAP)) <= 2 and
          abs(d["w"]["M2"]["frame"][0] + d["w"]["M2"]["frame"][2] - (vx + vw - GAP)) <= 2, [d["w"]["M1"]["frame"], d["w"]["M2"]["frame"]])
    check("…and the third is out of view", d["w"]["M3"]["parked"], d["w"]["M3"])
    print("Pinch: back out, and pinch in")
    canopy.close()
    windows.close()
    windows = Proc([exe, "P1", "P2", "P3"])
    assert windows.readline() == "ready"
    time.sleep(1)
    canopy = Proc([CANOPY, "--harness", BUNDLE, "--tap", "--animate"], env=env)
    json.loads(canopy.readline())
    time.sleep(1.2)
    canopy.ask("stall")
    for cmd in ["dock P1 screen left", "dock P2 P1 right", "dock P3 P2 right"]:
        canopy.ask(cmd)
        time.sleep(1.0)
    d = ask("frame P1", 1.4)
    check("framed P1 to start", d["world"]["framed"] == "P1", d["world"]["framed"])
    cx, cy = vx + vw / 2, vy + vh / 2
    d = ask(f"pinch {cx} {cy} -1.3", 1.6)
    check("pinching out settles on everything", d["world"]["framed"] == "H(P1 P2 P3 D)", d["world"]["framed"])
    p3 = d["w"]["P3"]["frame"]
    d = ask(f"pinch {p3[0] + p3[2] / 2} {p3[1] + p3[3] / 2} 1.0", 1.6)
    check("pinching in over P3 settles on P3", d["world"]["framed"] == "P3", d["world"]["framed"])
    check("…which now fills the screen", abs(d["w"]["P3"]["frame"][2] - (vw - 2 * GAP)) <= 2, d["w"]["P3"]["frame"])
    d = ask(f"pinch {cx} {cy} 0.05", 1.0)
    check("a tiny pinch stays where it is", d["world"]["framed"] == "P3", d["world"]["framed"])
    stall = canopy.ask("stall")
    check("the event tap ran on its own thread", stall["tap"], stall)
    check("the main thread never stalled over 100 ms during motion and pinches", stall["worstStallMs"] < 100, stall)
    print(f"    (worst main-thread stall with motion on: {stall['worstStallMs']} ms)")
    canopy.close()
    windows.close()


if __name__ == "__main__":
    main()
