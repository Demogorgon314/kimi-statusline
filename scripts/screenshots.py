#!/usr/bin/env python3
"""Render README screenshots: real kimi-statusline output (ANSI) laid into
an HTML terminal mock, captured with headless Chrome.

    python3 scripts/screenshots.py  # writes assets/*.png

Needs: a release build, Google Chrome, ImageMagick (`magick`) and a Nerd
Font (Hack Nerd Font Mono by default). Uses a throwaway KIMI_CODE_HOME with
a synthetic session, so nothing personal ends up in the images.
"""

import datetime
import html
import json
import os
import re
import shutil
import subprocess
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "release", "kimi-statusline")
ASSETS = os.path.join(ROOT, "assets")
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
FONT = os.environ.get("SHOT_FONT", "Hack Nerd Font Mono")

CWD = "/Users/demo/code/kimi-statusline"
PAYLOAD = {
    "model": "kimi-code/k3", "cwd": CWD, "gitBranch": "feat/quota",
    "permissionMode": "yolo", "planMode": False, "contextUsage": 0.37,
    "contextTokens": 97000, "maxContextTokens": 262144,
    "sessionId": "session_demo", "version": "2.1.1",
}

XTERM16 = ["#1d1f21", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
           "#56b6c2", "#dcdfe4", "#5c6370", "#ff7b86", "#b5e890", "#ffd88a",
           "#7cc4ff", "#de95f0", "#76d4e0", "#ffffff"]


def xterm256(n):
    if n < 16:
        return XTERM16[n]
    if n < 232:
        n -= 16
        step = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (step[n // 36], step[n // 6 % 6], step[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


def setup_home():
    home = tempfile.mkdtemp(prefix="ksl-shot-")
    cache = os.path.join(home, "kimi-statusline-cache")
    session = os.path.join(home, "sessions", "wd_demo", "session_demo")
    os.makedirs(os.path.join(session, "agents", "main", "tasks"))
    os.makedirs(os.path.join(session, "agents", "agent-0"))
    os.makedirs(cache)
    now_ms = int(time.time() * 1000)
    with open(os.path.join(session, "state.json"), "w") as f:
        json.dump({"id": "session_demo", "cwd": CWD, "createdAt": now_ms - 4_380_000,
                   "custom": {"goal": {"goalId": "g1", "status": "active", "turnsUsed": 7,
                                       "wallClockMs": 252_000, "budget": {}}}}, f)

    def wire(path, model, alias, n, usage, effort=None):
        req = {"type": "llm.request", "model": model, "modelAlias": alias}
        if effort:
            req["thinkingEffort"] = effort
        recs = [req] + [{"type": "usage.record", "model": alias, "usage": usage}] * n
        with open(path, "w") as f:
            f.writelines(json.dumps(r, separators=(",", ":")) + "\n" for r in recs)

    wire(os.path.join(session, "agents", "main", "wire.jsonl"), "k3", "kimi-code/k3", 38,
         {"inputOther": 2100, "output": 3900, "inputCacheRead": 96000, "inputCacheCreation": 0}, "high")
    wire(os.path.join(session, "agents", "agent-0", "wire.jsonl"), "k3-256k", "kimi-code/k3-256k", 9,
         {"inputOther": 5200, "output": 1400, "inputCacheRead": 61000, "inputCacheCreation": 0})
    with open(os.path.join(session, "agents", "main", "tasks", "bash-1.json"), "w") as f:
        json.dump({"status": "running"}, f)
    with open(os.path.join(home, "session_index.jsonl"), "w") as f:
        f.write(json.dumps({"sessionId": "session_demo", "sessionDir": session, "workDir": CWD}) + "\n")
    with open(os.path.join(home, "config.toml"), "w") as f:
        f.write('[models."kimi-code/k3"]\nmodel = "k3"\ndisplay_name = "K3"\n'
                'support_efforts = ["low","high","max"]\n'
                '[models."kimi-code/k3-256k"]\nmodel = "k3-256k"\ndisplay_name = "K3-256k"\n')
    now = datetime.datetime.now(datetime.timezone.utc)
    far = time.time() + 10**7
    with open(os.path.join(cache, "quota.json"), "w") as f:
        json.dump({"t": far, "fetched_at": time.time(), "error": None, "v": {
            "limit_5h": {"used_ratio": 0.42, "reset_at": (now + datetime.timedelta(hours=1, minutes=20)).isoformat()},
            "limit_7d": {"used_ratio": 0.63, "reset_at": (now + datetime.timedelta(days=3, hours=2)).isoformat()},
            "month": {"used_ratio": 0.31, "reset_at": (now + datetime.timedelta(days=20)).isoformat()}}}, f)
    # one run to learn the cache file names, then pin git + PR values
    render(home, "kimi", 300)
    for name in os.listdir(cache):
        p = os.path.join(cache, name)
        if name.startswith("git-"):
            json.dump({"t": far, "v": {"dirty": True, "conflicts": False, "ahead": 1, "behind": 0,
                                       "added": 128, "deleted": 17}}, open(p, "w"))
        elif name.startswith("pr-") and name.endswith(".json"):
            json.dump({"t": far, "branch": "feat/quota", "v": {
                "number": 42, "url": "https://github.com/Demogorgon314/kimi-statusline/pull/42"}}, open(p, "w"))
        elif name.endswith(".out"):
            os.remove(p)
    return home


def render(home, theme, width, payload=None, extra_env=None):
    env = dict(os.environ, KIMI_CODE_HOME=home, TERM="xterm-256color")
    env.pop("KIMI_STATUSLINE_NO_COLOR", None)
    env.update(extra_env or {})
    out = subprocess.run([BIN, "-t", theme, "--width", str(width)],
                         input=json.dumps(payload or PAYLOAD).encode(), capture_output=True, env=env)
    return out.stdout.decode().rstrip("\n")


def ansi_to_html(s, default_fg):
    s = re.sub(r"\x1b\]8;;[^\x07]*\x07", "", s)  # hyperlinks
    out, fg, bg, bold = [], None, None, False
    pos = 0
    for m in re.finditer(r"\x1b\[([0-9;]*)m", s):
        text = s[pos:m.start()]
        if text:
            style = []
            if fg:
                style.append("color:" + fg)
            if bg:
                style.append("background:" + bg)
            if bold:
                style.append("font-weight:700")
            out.append('<span style="%s">%s</span>' % (";".join(style), html.escape(text)))
        pos = m.end()
        codes = [int(c) if c else 0 for c in m.group(1).split(";")]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                fg, bg, bold = None, None, False
            elif c == 1:
                bold = True
            elif c == 22:
                bold = False
            elif c == 39:
                fg = None
            elif c == 49:
                bg = None
            elif c == 2:
                fg = "#7f848e"
            elif 30 <= c <= 37:
                fg = XTERM16[c - 30]
            elif 90 <= c <= 97:
                fg = XTERM16[c - 90 + 8]
            elif 40 <= c <= 47:
                bg = XTERM16[c - 40]
            elif 100 <= c <= 107:
                bg = XTERM16[c - 100 + 8]
            elif c in (38, 48) and i + 1 < len(codes):
                if codes[i + 1] == 2 and i + 4 < len(codes):
                    col = "#%02x%02x%02x" % tuple(codes[i + 2:i + 5])
                    i += 4
                elif codes[i + 1] == 5 and i + 2 < len(codes):
                    col = xterm256(codes[i + 2])
                    i += 2
                else:
                    col = None
                if c == 38:
                    fg = col
                else:
                    bg = col
            i += 1
    tail = s[pos:]
    if tail:
        out.append(html.escape(tail))
    return "".join(out)


PAGE = """<!doctype html><meta charset="utf-8"><style>
body{margin:0;background:transparent;font-family:'%(font)s',monospace;font-size:15px}
.win{margin:18px;border-radius:10px;overflow:hidden;background:%(bg)s;
     box-shadow:0 10px 30px rgba(0,0,0,.35);display:inline-block;min-width:%(minw)spx}
.bar{height:30px;background:%(bar)s;display:flex;align-items:center;padding-left:12px;gap:8px}
.dot{width:12px;height:12px;border-radius:50%%}
.t{color:#8b8f98;font-family:-apple-system,sans-serif;font-size:12px;margin-left:12px}
.body{padding:14px 18px 16px;color:%(fg)s;white-space:pre;line-height:1.55}
.body.tight{line-height:1.2}
.dim{color:#6b6b6b}.label{color:#8b8f98;font-family:-apple-system,sans-serif;font-size:12px;
     margin:10px 0 4px}
.rule{border-top:1px solid %(rule)s;margin:6px 0}
</style><div class="win"><div class="bar">
<span class="dot" style="background:#ff5f57"></span><span class="dot" style="background:#febc2e"></span>
<span class="dot" style="background:#28c840"></span><span class="t">%(title)s</span></div>
<div class="body">%(body)s</div></div>"""


def page(body, title, light=False, minw=900):
    return PAGE % {"font": FONT, "bg": "#f6f6f6" if light else "#16181d",
                   "bar": "#e4e4e4" if light else "#22252b", "fg": "#1a1a1a" if light else "#e0e0e0",
                   "rule": "#d0d0d0" if light else "#2c2f36", "title": html.escape(title),
                   "body": body, "minw": minw}


def shoot(html_text, name, width):
    tmp = tempfile.mkdtemp()
    src = os.path.join(tmp, "p.html")
    with open(src, "w") as f:
        f.write(html_text)
    raw = os.path.join(tmp, "raw.png")
    subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    "--force-device-scale-factor=2", "--default-background-color=00000000",
                    f"--window-size={width},1600", f"--screenshot={raw}", "file://" + src],
                   capture_output=True, check=True)
    dst = os.path.join(ASSETS, name)
    subprocess.run(["magick", raw, "-trim", "+repage", "-bordercolor", "none", "-border", "24", dst], check=True)
    shutil.rmtree(tmp)
    print("wrote", os.path.relpath(dst, ROOT))


def kimi_footer(line):
    """The status line in context: a fake Kimi Code transcript tail with
    our line as footer line 1 and the built-in context line below it."""
    prompt = ('<span style="color:#4FA8FF">›</span> add a quota segment and show reset times\n'
              '<span class="dim">  ● Edited src/quota.rs (+212 -0)\n'
              '  ● Ran cargo test — 12 passed</span>\n'
              '<div class="rule"></div>')
    ctx = '<span class="dim">                                                                    context 37% · 97k / 256k</span>'
    return prompt + line + "\n" + ctx


def main():
    os.makedirs(ASSETS, exist_ok=True)
    home = setup_home()
    try:
        hero = ansi_to_html(render(home, "nord", 205), "#e0e0e0")
        shoot(page(kimi_footer(hero), "kimi — ~/code/kimi-statusline", minw=1180), "hero.png", 2400)

        rows = []
        for theme in ["kimi", "cometix", "minimal", "gruvbox", "nord", "powerline-dark",
                      "powerline-light", "powerline-rose-pine", "powerline-tokyo-night"]:
            line = ansi_to_html(render(home, theme, 175), "#e0e0e0")
            rows.append('<div class="label">%s</div>%s' % (theme, line))
        shoot(page("\n".join(rows), "kimi-statusline themes", minw=1000), "themes.png", 2400)

        zh_cfg_env = {}
        segs = []
        for w, label in [(205, "205 columns"), (150, "150 columns"), (110, "110 columns"), (80, "80 columns")]:
            segs.append('<div class="label">%s</div>%s' % (label, ansi_to_html(render(home, "kimi", w, extra_env=zh_cfg_env), "")))
        shoot(page("\n".join(segs), "adaptive width", minw=1000), "adaptive.png", 2400)
    finally:
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()


def configurator_shot():
    """Drive `kimi-statusline config` in a pty, replay the screen through
    pyte (a VT100 emulator), and render the cells as HTML."""
    import fcntl
    import pty
    import select
    import struct
    import termios

    import pyte

    cols, rows = 132, 40
    home = setup_home()
    try:
        env = dict(os.environ, KIMI_CODE_HOME=home, TERM="xterm-256color", COLUMNS=str(cols), LINES=str(rows))
        # a cwd whose tail reads like a real project, tied to the demo session
        work = os.path.join(home, "demo", "code", "kimi-statusline")
        os.makedirs(work)
        state = os.path.join(home, "sessions", "wd_demo", "session_demo", "state.json")
        st = json.load(open(state))
        st["cwd"] = work
        json.dump(st, open(state, "w"))
        with open(os.path.join(home, "session_index.jsonl"), "w") as f:
            f.write(json.dumps({"sessionId": "session_demo", "sessionDir": os.path.dirname(state),
                                "workDir": work}) + "\n")
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(work)
            os.execve(BIN, [BIN, "config"], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        screen = pyte.Screen(cols, rows)
        stream = pyte.ByteStream(screen)

        def pump(t):
            end = time.time() + t
            while time.time() < end:
                r, _, _ = select.select([fd], [], [], 0.05)
                if r:
                    try:
                        stream.feed(os.read(fd, 1 << 16))
                    except OSError:
                        return

        pump(1.5)
        # select the Quota segment and open its settings
        for key in [b"\x1b[B"] * 10 + [b"\t"]:
            os.write(fd, key)
            pump(0.15)
        pump(0.5)
        body = screen_html(screen)
        try:
            os.kill(pid, 9)
            os.waitpid(pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass
    finally:
        shutil.rmtree(home, ignore_errors=True)
    shoot(page(body, "kimi-statusline config", minw=1000).replace('class="body"', 'class="body tight"'),
          "configurator.png", 2400)


PYTE_COLORS = {"black": 0, "red": 1, "green": 2, "brown": 3, "yellow": 3, "blue": 4, "magenta": 5,
               "cyan": 6, "white": 7, "brightblack": 8, "brightred": 9, "brightgreen": 10,
               "brightbrown": 11, "brightyellow": 11, "brightblue": 12, "brightmagenta": 13,
               "brightcyan": 14, "brightwhite": 15}


def pyte_color(c, fallback):
    if c == "default":
        return fallback
    if c in PYTE_COLORS:
        return XTERM16[PYTE_COLORS[c]]
    if re.fullmatch(r"[0-9a-fA-F]{6}", c):
        return "#" + c
    return fallback


def screen_html(screen):
    lines = []
    for y in range(screen.lines):
        row = screen.buffer[y]
        out, run, style = [], [], None
        for x in range(screen.columns):
            ch = row[x]
            fg = pyte_color(ch.fg, "#e0e0e0")
            bg = pyte_color(ch.bg, None)
            if ch.reverse:
                fg, bg = (bg or "#16181d"), fg
            st = "color:%s;%s%s" % (fg, "background:%s;" % bg if bg else "",
                                    "font-weight:700;" if ch.bold else "")
            if st != style and run:
                out.append('<span style="%s">%s</span>' % (style, html.escape("".join(run))))
                run = []
            style = st
            run.append(ch.data or " ")
        if run:
            out.append('<span style="%s">%s</span>' % (style, html.escape("".join(run))))
        lines.append("".join(out))
    return "\n".join(lines)


if __name__ == "__main__" and os.environ.get("SHOT_CONFIGURATOR", "1") == "1":
    try:
        configurator_shot()
    except ImportError:
        print("skip configurator.png: pip install pyte")
