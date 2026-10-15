"""Make the README screenshots: drive `rustgis-tui --serve` through a few scenes and draw each frame
(its screen cells, with the map picture laid over the map area) to a PNG.

    python3 docs/tools/screenshots.py [path to rustgis-tui]     # writes docs/screenshots/*.png

Needs Pillow and rustgis-cli on PATH. Nothing here is a real terminal capture: the images are drawn
from the same frames the Claude Code pane shows.
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

from PIL import Image, ImageDraw, ImageFont

FONT = "/System/Library/Fonts/Menlo.ttc"
CW, CH, SIZE = 12, 24, 19  # cell width and height in pixels, font size
DEFAULT_FG, DEFAULT_BG = (208, 208, 208), (30, 30, 30)
OUT = Path(__file__).resolve().parents[1] / "screenshots"


def colour(word, default):
    return default if word & 0x0100_0000 else ((word >> 16) & 255, (word >> 8) & 255, word & 255)


def draw(frame, path):
    cols, rows = frame["cols"], frame["rows"]
    words = struct.unpack(f"<{cols * rows * 3}I", base64.b64decode(frame["cells"]))
    img = Image.new("RGB", (cols * CW, rows * CH), DEFAULT_BG)
    pen = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, SIZE)
    for i in range(cols * rows):
        ch, fg, bg = words[3 * i : 3 * i + 3]
        fg, bg = colour(fg, DEFAULT_FG), colour(bg, DEFAULT_BG)
        x, y = (i % cols) * CW, (i // cols) * CH
        pen.rectangle([x, y, x + CW - 1, y + CH - 1], fill=bg)
        c = chr(ch)
        if c == "▀":  # half blocks as exact rectangles, so the map under a popup stays crisp
            pen.rectangle([x, y, x + CW - 1, y + CH // 2 - 1], fill=fg)
        elif c == "▄":
            pen.rectangle([x, y + CH // 2, x + CW - 1, y + CH - 1], fill=fg)
        elif c == "█":
            pen.rectangle([x, y, x + CW - 1, y + CH - 1], fill=fg)
        elif c != " ":
            pen.text((x + CW / 2, y + CH / 2), c, font=font, fill=fg, anchor="mm")
    m = frame.get("map")
    if m:
        pic = Image.open(m["file"]).convert("RGB").resize((m["w"] * CW, m["h"] * CH), Image.LANCZOS)
        img.paste(pic, (m["x"] * CW, m["y"] * CH))
    img.save(path)
    print("wrote", path)


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

    def shot(self, name):
        draw(json.loads((self.dir / "frame.json").read_text()), OUT / f"{name}.png")

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


def main():
    exe = sys.argv[1] if len(sys.argv) > 1 else shutil.which("rustgis-tui")
    OUT.mkdir(parents=True, exist_ok=True)
    app = App(exe, 150, 44)
    try:
        # Countries chosen: highlighted on the map in its own colour, the rest dimmed.
        app.keys("BackTab", "Down", "Down", "Down")
        app.shot("overview")
        # Lakes chosen, then a click on Lake Victoria: every visible layer at the spot, Lakes first.
        app.keys("Up")
        app.click(*app.cell_for(33.0, -1.0))  # Lake Victoria
        app.shot("identify")
        app.keys("`")
        # The attribute table of Rivers in the panel, the rivers highlighted.
        app.keys("BackTab", "Up", "3")  # the click made the map the active pane
        app.shot("attributes")
        # The command palette.
        app.keys(":")
        app.act([{"key": c} for c in "buffer"])
        app.shot("palette")
        app.keys("`")
        # The keys, built from the same table the keys run from.
        app.keys("?")
        app.shot("help")
        app.keys("`")
    finally:
        app.close()


if __name__ == "__main__":
    main()
