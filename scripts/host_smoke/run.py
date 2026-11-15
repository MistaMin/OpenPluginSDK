#!/usr/bin/env python3
"""Host smoke tests for the SDK example ("Example Gain") in REAPER and OpenGrid9.

One entry point. See docs/HOST_TESTS.md.

    scripts/host_smoke/run.py --build DIR [--work-root DIR] [--hosts reaper,og9]
                              [--formats clap,vst3,au,lv2] [--install-au]

Safety rules, because a shell-out with an unquoted path once deleted part of a
disk on a machine with volumes "Developer" and "Developer 2":
  * Nothing runs through a shell. Every command is an argument list.
  * Every deletion goes through safe_remove(), which refuses an empty path, "/",
    the work root itself and anything not strictly inside the work root. The
    Audio Unit uninstall has its own exact-path guard.
  * The work root must not contain a space.
  * The Audio Unit is installed only with --install-au, never over an existing
    component, and removed again in a finally block.
"""
import argparse
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
REAPER_APP = "/Applications/REAPER.app"
REAPER_BIN = REAPER_APP + "/Contents/MacOS/REAPER"
OG9_APP = "/Applications/OpenGrid9.app"
OG9_LUA = OG9_APP + "/Contents/MacOS/opengrid9-lua"
AU_NAME = "Example Gain.component"
AU_DIR = os.path.join(os.path.expanduser("~"), "Library", "Audio", "Plug-Ins", "Components")
AU_TARGET = os.path.join(AU_DIR, AU_NAME)

WORK = None  # real path of the work root, set in main()


# ---------------------------------------------------------------- guards

def inside_work(path):
    """True only for a non-empty path strictly inside the work root."""
    if not path or WORK is None:
        return False
    rp = os.path.realpath(path)
    return rp != WORK and rp != "/" and rp.startswith(WORK + os.sep)


def safe_remove(path):
    """The only function that deletes anything under the work root."""
    if not inside_work(path):
        raise RuntimeError("refusing to delete outside the work root: %r" % (path,))
    if os.path.islink(path) or os.path.isfile(path):
        os.remove(path)
    elif os.path.isdir(path):
        shutil.rmtree(path)


def fresh_dir(path):
    if os.path.lexists(path):
        safe_remove(path)
    os.makedirs(path)
    return path


def uninstall_au():
    """Remove exactly the one component that install_au() put there."""
    if os.path.realpath(AU_TARGET) != os.path.join(os.path.realpath(AU_DIR), AU_NAME):
        raise RuntimeError("AU target path is not the expected one: " + AU_TARGET)
    if os.path.isdir(AU_TARGET) and not os.path.islink(AU_TARGET):
        shutil.rmtree(AU_TARGET)


# ---------------------------------------------------------------- helpers

def log(msg):
    print(msg, flush=True)


def run(cmd, timeout=600, env=None, cwd=None):
    """Run an argument list (no shell). Returns (returncode, output)."""
    assert isinstance(cmd, list) and all(isinstance(c, str) for c in cmd)
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, env=env, cwd=cwd)
    return p.returncode, (p.stdout or "") + (p.stderr or "")


def app_version(app):
    rc, out = run(["/usr/bin/defaults", "read", app + "/Contents/Info", "CFBundleShortVersionString"])
    v = out.strip() if rc == 0 else "unknown"
    rc, out = run(["/usr/bin/defaults", "read", app + "/Contents/Info", "CFBundleVersion"])
    return v + (" (build " + out.strip() + ")" if rc == 0 else "")


def write_sine(path, seconds=3.0, freq=1000.0, amp=0.25, rate=48000):
    n = int(seconds * rate)
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        frames = bytearray()
        for i in range(n):
            s = int(round(amp * 32767 * math.sin(2 * math.pi * freq * i / rate)))
            frames += struct.pack("<hh", s, s)
        w.writeframes(bytes(frames))


def read_wav_mono(path):
    """Left channel of a 16/24-bit PCM or 32-bit float WAV as floats, plus the rate."""
    with open(path, "rb") as f:
        data = f.read()
    pos, fmt, pcm = 12, None, None
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            fmt = struct.unpack("<HHIIHH", body[:16])
        elif cid == b"data":
            pcm = body
        pos += 8 + size + (size & 1)
    tag, ch, rate, _, _, bits = fmt
    step = ch * bits // 8
    out = []
    for i in range(0, len(pcm) - step + 1, step):
        if tag == 3 and bits == 32:
            out.append(struct.unpack("<f", pcm[i:i + 4])[0])
        elif bits == 24:
            out.append(int.from_bytes(pcm[i:i + 3], "little", signed=True) / 8388608.0)
        elif bits == 16:
            out.append(struct.unpack("<h", pcm[i:i + 2])[0] / 32768.0)
        else:
            raise ValueError("unsupported wav %r" % (fmt,))
    return out, rate


def peak(xs):
    return max((abs(x) for x in xs), default=0.0)


# ---------------------------------------------------------------- REAPER

def reaper_config(cfgdir, build):
    """A REAPER configuration of its own, whose plug-in search paths are the build folder."""
    os.makedirs(cfgdir, exist_ok=True)  # kept between runs: REAPER caches its plug-in scan here
    ini = os.path.join(cfgdir, "reaper.ini")
    if os.path.exists(ini):
        with open(ini) as f:
            if build in f.read():
                return ini
    with open(ini, "w") as f:
        f.write("[REAPER]\n")
        f.write("vstpath=%s\n" % build)
        f.write("vst3path=%s\n" % build)
        f.write("vstpath_arm64=%s\n" % build)
        f.write("clap_path=%s\n" % build)
        f.write("clap_path_macos=%s\n" % build)
        f.write("clap_path_arm64=%s\n" % build)
        f.write("splashimg=0\n")
        f.write("verchk=0\n")
        f.write("projdefsave=0\n")
        f.write("lastproject=\n")
        f.write("loadlastproj=0\n")
    return ini


def dismiss_dialogs(pid, stop):
    """A fresh REAPER configuration asks once whether to pick an audio device. Answer No for this one
    process (by pid, so no other REAPER is touched). Needs Accessibility access for the terminal."""
    script = (
        'tell application "System Events"\n'
        '  try\n'
        '    tell (first process whose unix id is %d)\n'
        '      repeat with w in windows\n'
        '        try\n'
        '          set t to (name of every static text of w) as string\n'
        '          if t contains "audio device" then click button "No" of w\n'
        '        end try\n'
        '      end repeat\n'
        '    end tell\n'
        '  end try\n'
        'end tell\n' % pid)
    while not stop.is_set():
        try:
            run(["/usr/bin/osascript", "-e", script], timeout=20)
        except Exception:
            pass
        stop.wait(2.0)


def reaper_format(fmt, build, root):
    d = fresh_dir(os.path.join(root, "reaper_" + fmt))
    wav_in = os.path.join(d, "input_sine.wav")
    write_sine(wav_in)
    ini = reaper_config(os.path.join(root, "reaper_cfg"), build)
    env = dict(os.environ)
    env.update(OPSMOKE_DIR=d, OPSMOKE_FORMAT=fmt.upper(), OPSMOKE_WAV=wav_in,
               CLAP_PATH=build, VST3_PATH=build)
    # REAPER runs Scripts/__startup.lua of its resource folder at start; this one only loads our script
    startup = os.path.join(os.path.dirname(ini), "Scripts", "__startup.lua")
    os.makedirs(os.path.dirname(startup), exist_ok=True)
    with open(startup, "w") as f:
        f.write("dofile([==[%s]==])\n" % os.path.join(HERE, "reaper_smoke.lua"))
    cmd = [REAPER_BIN, "-newinst", "-nosplash", "-cfgfile", ini]
    log("REAPER %s: %s" % (fmt, " ".join(cmd)))
    p = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    stop = threading.Event()
    watcher = threading.Thread(target=dismiss_dialogs, args=(p.pid, stop), daemon=True)
    watcher.start()
    try:
        out, _ = p.communicate(timeout=600)
    except subprocess.TimeoutExpired:
        p.kill()  # our own child only
        out, _ = p.communicate()
        out += "\n[killed after timeout]"
    stop.set()
    safe_remove(startup)  # the configuration must not run the script on its own later
    with open(os.path.join(d, "reaper.log"), "w") as f:
        f.write(out or "")
    jp = os.path.join(d, "reaper_%s.json" % fmt.upper())
    if not os.path.exists(jp):
        return {"format": fmt, "checks": [{"name": "script ran", "ok": False,
                                           "detail": "no result file; REAPER exit %s" % p.returncode}]}
    res = json.load(open(jp))
    res["checks"] += audio_checks(d, fmt.upper())
    res["exit"] = p.returncode
    return res


def audio_checks(d, fmt):
    def load(tag):
        path = os.path.join(d, "render_%s_%s.wav" % (fmt, tag))
        if not os.path.exists(path):
            return None
        return read_wav_mono(path)[0]

    def near(a, b, tol):
        return abs(a - b) <= tol * abs(b)

    checks = []
    r = {t: load(t) for t in ("g0", "g6", "gm12", "g12", "g12warm", "env", "env_reopen")}
    if any(r[t] is None for t in ("g0", "g6", "gm12", "g12", "g12warm")):
        return [{"name": "level measurements", "ok": False, "detail": "missing renders"}]
    p = {t: peak(v) for t, v in r.items() if v}
    checks.append({"name": "level: Gain 0 dB is unity", "ok": near(p["g0"], 0.25, 0.03),
                   "detail": "peak %.4f, input 0.25" % p["g0"]})
    for tag, db in (("g6", 6.0), ("gm12", -12.0), ("g12", 12.0)):
        want = p["g0"] * 10 ** (db / 20)
        want = min(want, 1.0)
        checks.append({"name": "level: Gain %+g dB" % db, "ok": near(p[tag], want, 0.04),
                       "detail": "peak %.4f, expected %.4f" % (p[tag], want)})
    want = math.tanh(0.25 * 10 ** (12 / 20))
    checks.append({"name": "level: Warm on (tanh) at +12 dB", "ok": near(p["g12warm"], want, 0.04),
                   "detail": "peak %.4f, expected %.4f, Warm off was %.4f" % (p["g12warm"], want, p["g12"])})
    for tag in ("env", "env_reopen"):
        v = r.get(tag)
        if not v:
            checks.append({"name": "level: Gain envelope (%s)" % tag, "ok": False, "detail": "missing render"})
            continue
        n = len(v) // 10
        first, last = peak(v[:n]), peak(v[-n:])
        ok = first > 0 and last / first > 2.5
        checks.append({"name": "level: Gain envelope ramps 0 to +12 dB (%s)" % tag, "ok": ok,
                       "detail": "first tenth peak %.4f, last tenth %.4f, ratio %.2f (expect about 3.5)" % (
                           first, last, last / first if first else 0)})
    return checks


# ---------------------------------------------------------------- OpenGrid9

OG9_TYPES = {"au": 0, "lv2": 2, "vst3": 7}
PLUGIN_NAME = "Editor Gain"  # the name the plug-in reports (examples/editor); the bundles are "Example Gain"
OG9_VST3_SCANNER = OG9_APP + "/Contents/lib/ardour-vst3-scanner"
OG9_AU_SCANNER = OG9_APP + "/Contents/lib/ardour-au-scanner"


def og9_run_phase(args, env, outjson, sdir=None, tries=4):
    last = None
    for t in range(1, tries + 1):
        if os.path.lexists(outjson):
            safe_remove(outjson)
        if sdir and os.path.lexists(sdir):  # "create" needs a session folder that does not exist yet
            safe_remove(sdir)
        cmd = [OG9_LUA, os.path.join(HERE, "og9_worker.lua")] + args
        try:
            rc, out = run(cmd, timeout=300, env=env)
        except subprocess.TimeoutExpired:
            rc, out = -1, "timeout"
        last = (rc, out)
        if os.path.exists(outjson):
            try:
                d = json.load(open(outjson))
                d["attempts"] = t
                return d
            except ValueError:
                pass
        time.sleep(2)
    return {"ok": False, "step": "process", "err": "no verdict after %d tries; last exit %s: %s" % (
        tries, last[0], last[1][-400:])}


def og9_home(build, root, formats):
    """A HOME of its own for OpenGrid9, so its plug-in lists and caches never touch the real one.
    The headless Lua backend only knows plug-ins in the default folders and in the scanner caches:
    the bundles are linked into the default folders of this HOME and scanned with OpenGrid9's own
    scanners. The real plug-in folders are not touched."""
    home = os.path.join(root, "og9_home")
    if os.path.lexists(home):
        safe_remove(home)
    plug = os.path.join(home, "Library", "Audio", "Plug-Ins")
    env = dict(os.environ, HOME=home)
    if "vst3" in formats:
        os.makedirs(os.path.join(plug, "VST3"))
        link = os.path.join(plug, "VST3", "Example Gain.vst3")
        os.symlink(os.path.join(build, "Example Gain.vst3"), link)
        run([OG9_VST3_SCANNER, "-q", "-f", link], env=env)
    if "lv2" in formats:
        os.makedirs(os.path.join(plug, "LV2"))
        os.symlink(os.path.join(build, "ExampleGain.lv2"), os.path.join(plug, "LV2", "ExampleGain.lv2"))
    if "au" in formats:
        run([OG9_AU_SCANNER, "-q", "-f", "aufx", "ExGn", "OpGd"], env=env)
    return home


def og9_format(fmt, build, root, home):
    d = fresh_dir(os.path.join(root, "og9_" + fmt))
    env = dict(os.environ, HOME=home)
    sdir = os.path.join(d, "session")
    cj, lj = os.path.join(d, "create.json"), os.path.join(d, "load.json")
    pat = os.path.join(d, "pattern.txt")
    create = og9_run_phase(["create", sdir, "smoke", PLUGIN_NAME, str(OG9_TYPES[fmt]), cj, pat], env, cj, sdir=sdir)
    load = None
    if create.get("ok"):
        load = og9_run_phase(["load", sdir, "smoke", PLUGIN_NAME, str(OG9_TYPES[fmt]), lj, pat], env, lj)
    return {"format": fmt, "create": create, "load": load}


def og9_checks(r):
    c, l = r["create"], r["load"]
    out = []

    def add(name, ok, detail=""):
        out.append({"name": name, "ok": bool(ok), "detail": detail})

    if not c.get("ok"):
        add("create session and insert plug-in", False, "%s: %s" % (c.get("step"), c.get("err")))
        return out
    add("create session and insert plug-in", True, "plug-in '%s'" % c.get("plugin_name"))
    add("parameters Gain and Warm", c.get("gain_index") is not None and c.get("warm_index") is not None,
        "count=%s labels=%s" % (c.get("parameter_count"), "|".join(str(x) for x in c.get("labels", []))))
    add("sweep Gain over its range", c.get("sweep_ok"), c.get("sweep_detail", ""))
    add("set Gain and Warm", c.get("set_ok"), c.get("set_detail", ""))
    add("save session", c.get("persist"), "")
    if l is None:
        add("reopen session", False, "not run")
    elif not l.get("ok") and l.get("step"):
        add("reopen session", False, "%s: %s" % (l.get("step"), l.get("err")))
    else:
        add("reopen: values kept", l.get("ok"), l.get("detail", ""))
    return out


# ---------------------------------------------------------------- Audio Unit

def install_au(build):
    src = os.path.join(build, AU_NAME)
    if not os.path.isdir(src):
        raise RuntimeError("no Audio Unit in the build folder: " + src)
    if os.path.lexists(AU_TARGET):
        raise RuntimeError("%s already exists; not touching it" % AU_TARGET)
    os.makedirs(AU_DIR, exist_ok=True)
    shutil.copytree(src, AU_TARGET, symlinks=True)
    log("installed " + AU_TARGET)


def register_au():
    run(["/usr/bin/killall", "-9", "AudioComponentRegistrar"])
    t0 = time.time()
    while time.time() - t0 < 600:
        rc, out = run(["/usr/bin/auval", "-a"], timeout=300)
        if "aufx ExGn OpGd" in out:
            return True
        time.sleep(5)
    return False


def auval(strict):
    cmd = ["/usr/bin/auval"] + (["-strict"] if strict else []) + ["-v", "aufx", "ExGn", "OpGd"]
    rc, out = run(cmd, timeout=600)
    last = [ln for ln in out.splitlines() if ln.strip()][-3:]
    return {"name": "auval" + (" -strict" if strict else ""),
            "ok": rc == 0 and "AU VALIDATION SUCCEEDED" in out,
            "detail": " / ".join(last), "full": out}


# ---------------------------------------------------------------- main

def table(results):
    lines = []
    for host, fmt, checks in results:
        for c in checks:
            lines.append("%-4s %-5s %-4s %-46s %s" % (host, fmt, "PASS" if c["ok"] else "FAIL", c["name"], c.get("detail", "")))
    return "\n".join(lines)


def main():
    global WORK
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", required=True, help="folder with Example Gain.clap/.vst3/.component and ExampleGain.lv2")
    ap.add_argument("--work-root", default=os.path.join(tempfile.gettempdir(), "openplugin-host-smoke"),
                    help="folder for sessions, configurations and renders (no space in the path)")
    ap.add_argument("--hosts", default="reaper,og9")
    ap.add_argument("--formats", default="clap,vst3,au,lv2")
    ap.add_argument("--install-au", action="store_true",
                    help="install Example Gain.component into ~/Library/Audio/Plug-Ins/Components for the run, then remove it")
    a = ap.parse_args()

    if " " in a.work_root:
        sys.exit("work root must not contain a space: %r" % a.work_root)
    os.makedirs(a.work_root, exist_ok=True)
    WORK = os.path.realpath(a.work_root)
    if WORK in ("/", "") or " " in WORK:
        sys.exit("unusable work root " + WORK)
    build = os.path.realpath(a.build)
    hosts = a.hosts.split(",")
    formats = a.formats.split(",")
    use_au = "au" in formats
    if use_au and not a.install_au:
        log("note: 'au' skipped (needs --install-au)")
        formats.remove("au")
        use_au = False

    meta = {"date": time.strftime("%Y-%m-%d %H:%M:%S"),
            "reaper": app_version(REAPER_APP), "opengrid9": app_version(OG9_APP),
            "macos": run(["/usr/bin/sw_vers", "-productVersion"])[1].strip()}
    log(json.dumps(meta))
    results, extra = [], []
    installed = False
    try:
        if use_au:
            install_au(build)
            installed = True
            log("registering the Audio Unit (killall AudioComponentRegistrar, then auval -a)")
            if not register_au():
                raise RuntimeError("the system did not list aufx ExGn OpGd")
            for strict in (False, True):
                r = auval(strict)
                extra.append(r)
                log("%s: %s" % (r["name"], "PASS" if r["ok"] else "FAIL"))
                with open(os.path.join(WORK, "%s.txt" % r["name"].replace(" ", "_")), "w") as f:
                    f.write(r["full"])
        if "reaper" in hosts:
            for fmt in [f for f in formats if f in ("clap", "vst3", "au")]:
                r = reaper_format(fmt, build, WORK)
                results.append(("REAPER", fmt, r["checks"]))
                if "param_names" in r:
                    meta.setdefault("reaper_details", {})[fmt] = {k: r.get(k) for k in (
                        "param_names", "gain_text", "warm_text", "gain_6_text", "reopen_gain_text", "reaper_version")}
        if "og9" in hosts:
            og9_fmts = [f for f in formats if f in OG9_TYPES]
            home = og9_home(build, WORK, og9_fmts)
            for fmt in og9_fmts:
                r = og9_format(fmt, build, WORK, home)
                results.append(("OG9", fmt, og9_checks(r)))
                meta.setdefault("og9_details", {})[fmt] = r
    finally:
        if installed:
            try:
                uninstall_au()
                log("removed " + AU_TARGET)
                run(["/usr/bin/killall", "-9", "AudioComponentRegistrar"])
            except Exception as e:  # report, never hide
                log("FAILED to remove the Audio Unit: %s -- remove it by hand: %s" % (e, AU_TARGET))
    for e in extra:
        results.append(("auval", "au", [{"name": e["name"], "ok": e["ok"], "detail": e["detail"]}]))
    report = table(results)
    log(report)
    meta["results"] = [{"host": h, "format": f, "checks": c} for h, f, c in results]
    with open(os.path.join(WORK, "report.json"), "w") as f:
        json.dump(meta, f, indent=1)
    log("report: " + os.path.join(WORK, "report.json"))
    sys.exit(0 if all(c["ok"] for _, _, cs in results for c in cs) else 1)


if __name__ == "__main__":
    main()
