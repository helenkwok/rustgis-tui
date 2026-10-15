"""Make the README images: drive `rustgis-tui --serve` through a few scenes, draw each frame (its
screen cells, with the map picture laid over the map area) in a window frame, add notes, and write
PNGs and GIFs to docs/screenshots/.

    python3 docs/tools/screenshots.py [path to rustgis-tui]

Needs Pillow, the Menlo font (macOS) and rustgis-cli on PATH. Nothing here is a terminal capture:
every image is drawn from the frames the Claude Code pane shows.
"""

import base64
import json
import math
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONT = "/System/Library/Fonts/Menlo.ttc"
DEFAULT_FG, DEFAULT_BG = (208, 208, 208), (30, 30, 30)
ORANGE, WHITE = (217, 119, 87), (255, 255, 255)
OUT = Path(__file__).resolve().parents[1] / "screenshots"


def font(size, bold=False):
    return ImageFont.truetype(FONT, size, index=1 if bold else 0)


# ---------------------------------------------------------------- a frame as a picture


def colour(word, default):
    return default if word & 0x0100_0000 else ((word >> 16) & 255, (word >> 8) & 255, word & 255)


def render(frame, cw):
    """The frame's cells, cw pixels wide and 2*cw tall, with the map picture laid over."""
    ch = 2 * cw
    cols, rows = frame["cols"], frame["rows"]
    words = struct.unpack(f"<{cols * rows * 3}I", base64.b64decode(frame["cells"]))
    img = Image.new("RGB", (cols * cw, rows * ch), DEFAULT_BG)
    pen = ImageDraw.Draw(img)
    face = font(round(cw / 0.602))
    for i in range(cols * rows):
        c, fg, bg = words[3 * i : 3 * i + 3]
        fg, bg = colour(fg, DEFAULT_FG), colour(bg, DEFAULT_BG)
        x, y = (i % cols) * cw, (i // cols) * ch
        if bg != DEFAULT_BG:
            pen.rectangle([x, y, x + cw - 1, y + ch - 1], fill=bg)
        c = chr(c)
        if c == "▀":  # half blocks as exact rectangles, so the map under a popup stays crisp
            pen.rectangle([x, y, x + cw - 1, y + ch // 2 - 1], fill=fg)
        elif c == "▄":
            pen.rectangle([x, y + ch // 2, x + cw - 1, y + ch - 1], fill=fg)
        elif c == "█":
            pen.rectangle([x, y, x + cw - 1, y + ch - 1], fill=fg)
        elif c != " ":
            pen.text((x + cw / 2, y + ch / 2), c, font=face, fill=fg, anchor="mm")
    m = frame.get("map")
    if m:
        pic = Image.open(m["file"]).convert("RGB").resize((m["w"] * cw, m["h"] * ch), Image.LANCZOS)
        img.paste(pic, (m["x"] * cw, m["y"] * ch))
    return img


def window(screen, title, scale):
    """The screen in a macOS-style window: title bar, traffic lights, rounded corners."""
    bar, pad, radius = round(30 * scale), round(10 * scale), round(12 * scale)
    w, h = screen.width + 2 * pad, screen.height + bar + pad
    win = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    pen = ImageDraw.Draw(win)
    pen.rounded_rectangle([0, 0, w - 1, h - 1], radius, fill=DEFAULT_BG + (255,), outline=(70, 70, 74), width=max(1, round(scale)))
    pen.rounded_rectangle([1, 1, w - 2, bar + radius], radius, fill=(44, 44, 48, 255))
    pen.rectangle([1, bar, w - 2, bar + radius], fill=DEFAULT_BG + (255,))
    for i, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        cx, r = round((18 + 20 * i) * scale), round(6 * scale)
        pen.ellipse([cx - r, bar // 2 - r, cx + r, bar // 2 + r], fill=c)
    pen.text((w / 2, bar / 2), title, font=font(round(12 * scale)), fill=(175, 175, 180), anchor="mm")
    win.paste(screen, (pad, bar))
    return win, (pad, bar)


def gradient(w, h):
    """The dark blue to plum backdrop of helenkwok's other mod READMEs."""
    a, b = (22, 25, 66), (60, 36, 42)
    small = Image.new("RGB", (64, 64))
    for y in range(64):
        for x in range(64):
            t = (x + y) / 126
            small.putpixel((x, y), tuple(round(a[k] + (b[k] - a[k]) * t) for k in range(3)))
    return small.resize((w, h), Image.BICUBIC).convert("RGBA")


def compose(screen, cw, notes, title="rustgis-tui", backdrop="clear", room=None):
    """A window on a backdrop with notes beside it. Each note is (column, row, text): a dot on that
    cell, a line out to the nearer side (left for the left two fifths of the screen, else right)
    and the text in an orange label there, so a line never crosses the whole screen."""
    scale = cw / 8
    win, (ox, oy) = window(screen, title, scale)
    margin = round(40 * scale)
    face = font(round(11 * scale), bold=True)
    padx, pady, gap = round(9 * scale), round(7 * scale), round(24 * scale)
    cols = screen.width // cw
    left = [n for n in notes if n[0] < cols * 0.4]
    right = [n for n in notes if n[0] >= cols * 0.4]

    def need(side):
        return round(max(face.getlength(t) for _, _, t in side) + 2 * padx + gap) if side else 0

    lroom, rroom = need(left), (room if room is not None and right else need(right))
    W, H = margin + lroom + win.width + rroom + margin, margin + win.height + margin
    canvas = gradient(W, H) if backdrop == "gradient" else Image.new("RGBA", (W, H), (0, 0, 0, 0))
    wx = margin + lroom
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        [wx, margin + round(8 * scale), wx + win.width, margin + win.height + round(8 * scale)],
        round(12 * scale), fill=(0, 0, 0, 140))
    canvas = Image.alpha_composite(canvas, shadow.filter(ImageFilter.GaussianBlur(round(14 * scale))))
    canvas.alpha_composite(win, (wx, margin))
    pen = ImageDraw.Draw(canvas)
    th, r, lw = round(11 * scale), round(4 * scale), max(2, round(1.6 * scale))
    for col, row, text in notes:
        px, py = wx + ox + col * cw + cw // 2, margin + oy + row * 2 * cw + cw
        tw = face.getlength(text) + 2 * padx
        lx = wx - gap - tw if (col, row, text) in left else wx + win.width + gap
        end = lx + tw if (col, row, text) in left else lx
        pen.line([px, py, end, py], fill=ORANGE, width=lw)
        pen.ellipse([px - r, py - r, px + r, py + r], fill=ORANGE)
        pen.rounded_rectangle([lx, py - th / 2 - pady, lx + tw, py + th / 2 + pady], round(6 * scale), fill=ORANGE)
        pen.text((lx + padx, py), text, font=face, fill=WHITE, anchor="lm")
    return canvas


def end_of(frame, row, start=0):
    """The column just past the text on a row, from `start` (skipping box lines on the right)."""
    line = frame["text"][row].rstrip(" │┃┐┓┘┛")
    return max(start, len(line) + 1)


def save_png(img, name):
    img.save(OUT / f"{name}.png", optimize=True)
    print("wrote", OUT / f"{name}.png")


def save_gif(frames, name):
    """frames: (RGBA image, milliseconds). One shared palette keeps the file small and steady."""
    flat = [Image.alpha_composite(Image.new("RGBA", f.size, (20, 22, 40, 255)), f).convert("RGB") for f, _ in frames]
    sheet = Image.new("RGB", (flat[0].width, flat[0].height * len(flat)))
    for i, f in enumerate(flat):
        sheet.paste(f, (0, i * f.height))
    palette = sheet.quantize(colors=255, method=Image.Quantize.MEDIANCUT)
    quant = [f.quantize(palette=palette, dither=Image.Dither.NONE) for f in flat]
    quant[0].save(OUT / f"{name}.gif", save_all=True, append_images=quant[1:], duration=[d for _, d in frames], loop=0, optimize=True)
    print("wrote", OUT / f"{name}.gif")


# ---------------------------------------------------------------- driving the app


class App:
    def __init__(self, exe, cols, rows):
        self.dir = Path(tempfile.mkdtemp(prefix="rustgis-shots-"))
        (self.dir / "in").mkdir()
        self.seq = 0
        self.send([{"size": [cols, rows]}])
        self.proc = subprocess.Popen([exe, "--serve", str(self.dir)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.wait(0, 20)

    def gen(self):
        try:
            return int((self.dir / "gen").read_text())
        except (OSError, ValueError):
            return 0

    def wait(self, after, seconds=8):
        end = time.time() + seconds
        while time.time() < end and self.gen() <= after:
            time.sleep(0.05)
        time.sleep(1.2)  # let follow-up frames (a highlight, a half-block map) land

    def send(self, events):
        self.seq += 1
        (self.dir / "in" / f"{self.seq:08d}.json").write_text(json.dumps(events))

    def act(self, events):
        g = self.gen()
        self.send(events)
        self.wait(g)

    def keys(self, *ks):
        self.act([{"key": k} for k in ks])

    def click(self, x, y, button="left"):
        self.act([{"mouse": "down", "x": x, "y": y, "button": button}, {"mouse": "up", "x": x, "y": y, "button": button}])

    def wheel(self, x, y, up, times):
        for _ in range(times):
            self.act([{"mouse": "scroll", "x": x, "y": y, "dir": "up" if up else "down"}])

    def frame(self):
        return json.loads((self.dir / "frame.json").read_text())

    def where(self, x, y):
        """lon, lat under a cell, as the app's own pointer readout (the map's bottom border) says."""
        self.act([{"mouse": "move", "x": x, "y": y}])
        m = self.frame()["map"]
        found = re.search(r"(-?\d+\.\d+), (-?\d+\.\d+)", self.frame()["text"][m["y"] + m["h"]])
        return float(found.group(1)), float(found.group(2))

    def cell_for(self, lon, lat):
        """The cell over a lon, lat: two readouts, then Web Mercator in between."""
        merc = lambda la: math.log(math.tan(math.pi / 4 + math.radians(la) / 2))
        m = self.frame()["map"]
        (x0, y0), (x1, y1) = (m["x"] + 2, m["y"] + 2), (m["x"] + m["w"] - 3, m["y"] + m["h"] - 3)
        (lo0, la0), (lo1, la1) = self.where(x0, y0), self.where(x1, y1)
        x = x0 + (lon - lo0) / (lo1 - lo0) * (x1 - x0)
        y = y0 + (merc(lat) - merc(la0)) / (merc(la1) - merc(la0)) * (y1 - y0)
        return round(x), round(y)

    def close(self):
        self.send([{"quit": True}])
        self.proc.wait(10)
        shutil.rmtree(self.dir, ignore_errors=True)


def find(frame, text):
    """(column, row) of the first place `text` is on screen."""
    for y, line in enumerate(frame["text"]):
        if text in line:
            return line.index(text), y
    raise SystemExit(f"not on screen: {text}")


# ---------------------------------------------------------------- the images


def stills(exe):
    """Large PNGs with notes, and the banner."""
    cw = 14
    app = App(exe, 132, 40)
    try:
        app.keys("BackTab", "Down", "Down", "Down")  # Countries
        f = app.frame()
        m = f["map"]
        x, y = find(f, "● ■ Countries")
        ty = m["y"] + m["h"]  # the map's bottom border: the scale and the scale bar
        dx, dy = find(f, "■ Countries  Polygon")
        notes = [
            (x + 2, y, "each layer has its own colour"),
            (m["x"] + m["w"] * 2 // 3, m["y"] + m["h"] // 3, "the chosen layer, highlighted"),
            (end_of(f, ty), ty, "the scale and a scale bar"),
            (end_of(f, dy), dy, "details, messages, attributes"),
        ]
        shot = render(f, cw)
        save_png(compose(shot, cw, notes), "overview")
        banner(shot, f)

        app.keys("Up")  # Lakes
        app.click(*app.cell_for(33.0, -1.0))  # Lake Victoria
        f = app.frame()
        x, y = find(f, "Lakes · row")
        save_png(compose(render(f, cw), cw, [(x + 14, y, "click: every layer there, the chosen one first")]), "identify")
        app.keys("`")

        app.keys("BackTab", "Up", "3")  # the click made the map the active pane
        f = app.frame()
        x, y = find(f, "row 1 of")
        save_png(compose(render(f, cw), cw, [(x + 12, y, "3: the whole table, every row")]), "attributes")

        app.keys(":")
        app.act([{"key": c} for c in "buffer"])
        f = app.frame()
        x, y = find(f, " of ")
        save_png(compose(render(f, cw), cw, [(x + 6, y, ": every command and tool")]), "palette")
        app.keys("`")

        app.keys("?")
        f = app.frame()
        x, y = find(f, "Keys ·")
        save_png(compose(render(f, cw), cw, [(x + 4, y, "built from the keys' own table")]), "help")
        app.keys("`")
    finally:
        app.close()


def banner(shot, f):
    """2560x1280: the name and what it is on the left, the map from the overview on the right."""
    W, H = 2560, 1280
    img = gradient(W, H)
    pen = ImageDraw.Draw(img)
    pen.text((128, 380), "TERMINAL UI · CLAUDE CODE MOD", font=font(34, bold=True), fill=(170, 165, 215))
    pen.text((124, 430), "rustgis-tui", font=font(118, bold=True), fill=ORANGE)
    for i, line in enumerate(["RustGIS in your terminal:", "layers, the real map, every tool.", "And a pane you and Claude", "drive together."]):
        pen.text((128, 620 + i * 66), line, font=font(46), fill=(236, 236, 236))
    pen.text((128, 930), "unofficial · MIT · RustGIS 0.2.0", font=font(32), fill=(150, 150, 165))
    m = f["map"]
    cw = shot.width // f["cols"]
    ch = 2 * cw  # the map and the panel under it: no header, no footer
    crop = shot.crop((m["x"] * cw - cw, m["y"] * ch - ch, (m["x"] + m["w"]) * cw + cw, (f["rows"] - 1) * ch))
    crop.thumbnail((1180, 900))
    win, _ = window(crop, "rustgis-tui", 1.6)
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    x0, y0 = W - win.width - 110, (H - win.height) // 2
    ImageDraw.Draw(shadow).rounded_rectangle([x0, y0 + 14, x0 + win.width, y0 + win.height + 14], 20, fill=(0, 0, 0, 160))
    img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(26)))
    img.alpha_composite(win, (x0, y0))
    save_png(img, "banner")


def tour(exe):
    """A short tour: layers and highlights, identify, the wheel, the table, the palette."""
    cw = 8
    app = App(exe, 120, 36)
    frames = []

    def snap(ms, note, at):
        f = app.frame()
        frames.append((compose(render(f, cw), cw, [(*at(f), note)], backdrop="gradient", room=290), ms))

    try:
        snap(1800, "4 layers, each with its own colour", lambda f: find(f, "■ Cities"))
        app.keys("BackTab", "Down")
        snap(2200, "choose one: it lights up on the map", lambda f: find(f, "■ Rivers"))
        app.keys("Down")
        snap(2200, "Lakes", lambda f: find(f, "■ Lakes"))
        x, y = app.cell_for(33.0, -1.0)
        app.click(x, y)
        snap(3000, "click: what is there", lambda f: (find(f, "Lakes · row")[0] + 12, find(f, "Lakes · row")[1]))
        app.keys("`")
        app.wheel(x, y, True, 4)
        snap(2600, "wheel: zoom where you point", lambda f: (x, y))
        app.keys("BackTab", "3")
        snap(3000, "3: the attribute table", lambda f: (find(f, "row 1 of")[0] + 10, find(f, "row 1 of")[1]))
        app.keys(":")
        app.act([{"key": c} for c in "buffer"])
        snap(2600, ": all 352 commands and tools", lambda f: (find(f, "buffer")[0] + 6, find(f, "buffer")[1]))
    finally:
        app.close()
    save_gif(frames, "demo-tour")


def together(exe):
    """The Claude Code pane: the person and Claude drive the same running app."""
    cw = 8
    app = App(exe, 118, 36)
    frames = []

    def snap(ms, note, at):
        f = app.frame()
        frames.append((compose(render(f, cw), cw, [(*at(f), note)], title="claude · /rustgis pane", backdrop="gradient", room=330), ms))

    try:
        snap(1800, "/rustgis: the app in a pane", lambda f: find(f, "RustGIS"))
        app.click(*find(app.frame(), "■ Countries"))
        snap(2400, "you click Countries", lambda f: find(f, "■ Countries"))
        app.keys("Tab", "e")
        snap(2400, "you press Tab, e: the map, full screen", lambda f: find(f, "Map ·"))
        app.keys("e", "BackTab", "Up", "Up", "Up")
        snap(2600, "Claude sends Up Up Up: Cities", lambda f: find(f, "■ Cities"))
        app.keys("3")
        snap(3000, "Claude sends 3: the cities table", lambda f: (find(f, "row 1 of")[0] + 10, find(f, "row 1 of")[1]))
    finally:
        app.close()
    save_gif(frames, "demo-together")


def main():
    exe = sys.argv[1] if len(sys.argv) > 1 else shutil.which("rustgis-tui")
    OUT.mkdir(parents=True, exist_ok=True)
    stills(exe)
    tour(exe)
    together(exe)


if __name__ == "__main__":
    main()
