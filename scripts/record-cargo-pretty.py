#!/usr/bin/env python3
"""Record real mbx output as the README/home GIF (macOS/Linux).

Requires Pillow and pyte. Run after `cargo build -p mbx`:
  python3 scripts/record-cargo-pretty.py --font /path/to/monospace.ttf

A small local fixture sleeps in its build script to make the animation visible.
This is a UI demonstration, not a benchmark. Output is captured from a real PTY
and rendered without inventing build messages or changing playback speed.
"""
import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT, COLS, ROWS = 1120, 240, 80, 5
COLORS = {"default": "#d9e4e6", "cyan": "#70d7cb", "green": "#9fce88", "yellow": "#e9c778", "red": "#ef8b86"}


def render(screen, font, title_font):
    image = Image.new("RGB", (WIDTH, HEIGHT), "#111a20")
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((1, 1, WIDTH - 2, HEIGHT - 2), radius=15, outline="#35434a", width=2)
    for x, color in [(28, "#ed807a"), (50, "#e6bf6c"), (72, "#82c39a")]:
        draw.ellipse((x, 22, x + 10, 32), fill=color)
    draw.text((WIDTH // 2, 19), "mbx build · mr boxington", font=title_font, fill="#91a3aa", anchor="mt")
    draw.line((1, 52, WIDTH - 2, 52), fill="#29363e")
    cell = font.getlength("M")
    for y in range(ROWS):
        for x in range(COLS):
            char = screen.buffer[y][x]
            draw.text((25 + x * cell, 76 + y * 25), char.data, font=font, fill=COLORS.get(char.fg, COLORS["default"]))
    return image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/mbx")
    args = parser.parse_args()
    font = ImageFont.truetype(args.font, 21)
    title_font = ImageFont.truetype(args.font, 14)
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    stream.feed(b"$ mbx build\r\n")
    frames = [render(screen, font, title_font)]
    durations = [700]
    with tempfile.TemporaryDirectory(prefix="mbx-pretty-demo-") as tmp:
        root = Path(tmp)
        (root / "src").mkdir()
        (root / "Cargo.toml").write_text('[package]\nname = "hello-boxington"\nversion = "0.1.0"\nedition = "2024"\n')
        (root / "src/main.rs").write_text('fn main() { println!("Hello, Boxington!"); }\n')
        (root / "build.rs").write_text('fn main() { std::thread::sleep(std::time::Duration::from_secs(2)); }\n')
        stamp = root / "cache/actions/notice/v1/explained"
        stamp.parent.mkdir(parents=True)
        stamp.touch()
        env = dict(os.environ, TERM="xterm-256color", MBX_CACHE_DIR=str(root / "cache"), MBX_TARGET_VIEWS="false", MBX_GC_AUTO="false", MBX_SUMMARY="off", MBX_SAVINGS="off")
        for key in ["CI", "NO_COLOR", "CARGO_TERM_COLOR", "CARGO_TERM_PROGRESS_WHEN", "MBX_DISABLE", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]:
            env.pop(key, None)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        child = subprocess.Popen([str(args.binary.resolve()), "build"], cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        transcript = bytearray()
        last_frame = time.monotonic()
        deadline = last_frame + 120
        try:
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.08)
                if ready:
                    try:
                        data = os.read(master, 65536)
                    except OSError as error:
                        if error.errno == errno.EIO:
                            break
                        raise
                    if not data:
                        break
                    transcript.extend(data)
                    stream.feed(data)
                now = time.monotonic()
                if now - last_frame >= 0.08:
                    frames.append(render(screen, font, title_font))
                    durations.append(round((now - last_frame) * 1000))
                    last_frame = now
            else:
                raise TimeoutError("demo build exceeded 120 seconds")
            assert child.wait(timeout=5) == 0, transcript.decode(errors="replace")
        finally:
            os.close(master)
            if child.poll() is None:
                child.kill()
                child.wait()
        assert b"Compiling hello-boxington" in transcript, transcript
        assert b"Finished" in transcript, transcript
        assert b"\x1b[2K" in transcript, transcript
    stream.feed(b"$ ")
    frames.append(render(screen, font, title_font))
    durations.append(2200)
    output = ROOT / "docs/public/screenshots"
    output.mkdir(parents=True, exist_ok=True)
    frames[-1].save(output / "cargo-pretty.png")
    frames[0].save(output / "cargo-pretty.gif", save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)
    print(f"Recorded {len(frames)} frames to {output / 'cargo-pretty.gif'}")


if __name__ == "__main__":
    main()
