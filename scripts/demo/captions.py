#!/usr/bin/env python3
"""Adds key captions under the demo video and writes the GIF.

usage: captions.py <tape> <video.mp4> <out.gif>

Times each "#cap <keys> | <text>" line in the tape by adding up the Sleep,
key, and Type commands after Show, then draws the caption on a bar below
the terminal: each key as a filled key cap, then what it does.
"""

import re
import subprocess
import sys
import tempfile
from pathlib import Path

FONT = "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf"
FONT_BOLD = "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf"
FONT_SIZE = 22
CHAR_WIDTH = FONT_SIZE * 0.6  # JetBrains Mono advance width
BAR_HEIGHT = 64
BAR_COLOR = "0x16161e"
KEY_BG = "0x7aa2f7"
KEY_FG = "0x16161e"
TEXT_FG = "0xc0caf5"
KEY_PAD = 7
GAP = 12

KEY_COMMANDS = {"Tab", "Escape", "Space", "Enter", "Left", "Right", "Up", "Down", "Backspace"}


def seconds(value):
    match = re.fullmatch(r"([\d.]+)(ms|s)?", value)
    if not match:
        raise ValueError(f"bad duration: {value}")
    amount = float(match.group(1))
    return amount / 1000 if match.group(2) == "ms" else amount


def captions(tape):
    """Returns ([(start, end, keys, text)], end of tape) in seconds after Show."""
    typing = 0.05
    shown = False
    now = 0.0
    result = []
    current = None
    for raw in tape.splitlines():
        line = raw.strip()
        if line.startswith("Set TypingSpeed"):
            typing = seconds(line.split()[-1])
        elif line == "Show":
            shown = True
        elif not shown:
            continue
        elif line.startswith("#cap"):
            if current:
                result.append((current[0], now, current[1], current[2]))
            body = line[len("#cap"):].strip()
            if body == "-":
                current = None
            else:
                keys, _, text = body.partition("|")
                current = (now, keys.split(), text.strip())
        elif line.startswith("Sleep "):
            now += seconds(line.split()[1])
        elif line.startswith("Type "):
            now += typing * len(line[len("Type "):].strip().strip('"'))
        elif line.split("+")[0] in {"Ctrl", "Alt"} or line in KEY_COMMANDS:
            now += typing
    if current:
        result.append((current[0], now, current[1], current[2]))
    return result, now


def drawtext(textfile, x, y, font, color, start, end, box=None):
    options = [
        f"fontfile={font}",
        f"textfile={textfile}",
        f"fontsize={FONT_SIZE}",
        f"fontcolor={color}",
        f"x={x:.0f}",
        f"y={y}",
        f"enable='between(t,{start:.2f},{end:.2f})'",
    ]
    if box:
        options += ["box=1", f"boxcolor={box}", f"boxborderw={KEY_PAD}"]
    return "drawtext=" + ":".join(options)


def main():
    tape_path, video, out = sys.argv[1:4]
    width, height = map(
        int,
        subprocess.check_output(
            ["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
             "stream=width,height", "-of", "csv=p=0", video],
            text=True,
        ).strip().split(","),
    )
    text_y = height + (BAR_HEIGHT - FONT_SIZE) // 2
    filters = [f"pad=iw:ih+{BAR_HEIGHT}:0:0:color={BAR_COLOR}"]
    with tempfile.TemporaryDirectory() as tmp:
        files = 0

        def write(text):
            nonlocal files
            files += 1
            path = Path(tmp) / f"t{files}.txt"
            path.write_text(text)
            return path

        timeline, tape_end = captions(Path(tape_path).read_text())
        duration = float(subprocess.check_output(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of",
             "csv=p=0", video], text=True))
        # VHS spends a little less than TypingSpeed on each key, so the tape's
        # own times run long; stretch them to the video's real length.
        scale = duration / tape_end
        for start, end, keys, text in timeline:
            start, end = start * scale, end * scale
            key_widths = [len(key) * CHAR_WIDTH + 2 * KEY_PAD for key in keys]
            total = sum(key_widths) + GAP * len(keys) + len(text) * CHAR_WIDTH
            x = (width - total) / 2
            for key, key_width in zip(keys, key_widths):
                filters.append(drawtext(write(key), x + KEY_PAD, text_y, FONT_BOLD,
                                        KEY_FG, start, end, box=KEY_BG))
                x += key_width + GAP
            if text:
                filters.append(drawtext(write(text), x, text_y, FONT, TEXT_FG, start, end))
        graph = ",".join(filters) + (
            ",split[a][b];[a]palettegen=stats_mode=diff[p];"
            "[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle"
        )
        subprocess.run(
            ["ffmpeg", "-v", "error", "-y", "-i", video, "-filter_complex", graph, out],
            check=True,
        )


if __name__ == "__main__":
    main()
