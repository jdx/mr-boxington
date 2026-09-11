#!/usr/bin/env python3
"""Record real mbx output as the README/home GIF (macOS/Linux).

Requires Pillow and pyte. Run after `cargo build -p mbx`:
  python3 scripts/record-cargo-pretty.py --font /path/to/monospace.ttf

A local multi-crate fixture uses non-incremental builds for repeatable cache results.
This is a UI demonstration, not a benchmark. Output is captured from a real PTY
and rendered without inventing build messages or changing playback speed.
"""
import argparse
import errno
import fcntl
import os
from pathlib import Path
import pty
import re
import json
import select
import struct
import subprocess
import tempfile
import termios
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT, COLS, ROWS = 820, 550, 78, 27
COLORS = {"default": "#d9e4e6", "cyan": "#70d7cb", "green": "#9fce88", "yellow": "#e9c778", "red": "#ef8b86"}


def render(screen, font, title_font, stage="mixed rebuild"):
    image = Image.new("RGB", (WIDTH, HEIGHT), "#111a20")
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((1, 1, WIDTH - 2, HEIGHT - 2), radius=15, outline="#35434a", width=2)
    for x, color in [(28, "#ed807a"), (50, "#e6bf6c"), (72, "#82c39a")]:
        draw.ellipse((x, 22, x + 10, 32), fill=color)
    draw.text((WIDTH // 2, 19), f"mr boxington  /  {stage}", font=title_font, fill="#91a3aa", anchor="mt")
    draw.line((1, 52, WIDTH - 2, 52), fill="#29363e")
    cell = font.getlength("M")
    for y in range(ROWS):
        for x in range(COLS):
            char = screen.buffer[y][x]
            draw.text((25 + x * cell, 65 + y * 19), char.data, font=font, fill=COLORS.get(char.fg, "#" + char.fg if re.fullmatch(r"[0-9a-fA-F]{6}", char.fg) else COLORS["default"]))
    return image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/mbx")
    args = parser.parse_args()
    font = ImageFont.truetype(args.font, 16)
    title_font = ImageFont.truetype(args.font, 14)
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    pending_frame = bytearray()
    def feed_frame(data):
        pending_frame.extend(data)
        begin, end = b"\x1b[?2026h", b"\x1b[?2026l"
        while pending_frame:
            start = pending_frame.find(begin)
            if start >= 0:
                finish = pending_frame.find(end, start + len(begin))
                if finish < 0:
                    stream.feed(bytes(pending_frame[:start]))
                    del pending_frame[:start]
                    break
                finish += len(end)
            else:
                keep = next((n for n in range(len(begin) - 1, 0, -1) if pending_frame.endswith(begin[:n])), 0)
                finish = len(pending_frame) - keep
                if not finish: break
            stream.feed(bytes(pending_frame[:finish]))
            del pending_frame[:finish]
    stream.feed(b"$ mbx build -j 4\r\n")
    frames = [render(screen, font, title_font)]
    durations = [700]
    with tempfile.TemporaryDirectory(prefix="mbx-pretty-demo-") as tmp:
        root = Path(tmp)
        (root / "src").mkdir()
        crates = ["boxer-core", "boxer-config", "boxer-store", "boxer-format", "boxer-protocol", "boxer-ui", "boxer-parser", "boxer-codegen", "boxer-network", "boxer-index", "boxer-query", "boxer-runtime", "boxer-scheduler", "boxer-archive", "boxer-checksum", "boxer-diagnostics", "boxer-transport", "boxer-render"]
        dependencies = "\n".join(f'{name} = {{ path = "{name}" }}' for name in crates)
        (root / "Cargo.toml").write_text('[package]\nname="hello-boxington"\nversion="0.1.0"\nedition="2024"\n[dependencies]\n' + dependencies + "\n")
        (root / "src/main.rs").write_text('fn main() { println!("Hello, Boxington!"); }\n')
        for index, name in enumerate(crates):
            folder = root / name
            (folder / "src").mkdir(parents=True)
            (folder / "Cargo.toml").write_text(f'[package]\nname="{name}"\nversion="0.1.0"\nedition="2024"\n')
            (folder / "src/lib.rs").write_text('pub fn value() -> u32 { 42 }\n' + "\n".join(f"pub fn f{i}(value: u64) -> u64 {{ value.rotate_left({i % 64}) ^ {i} }}" for i in range(30000 + index * 1000)))
        (root / "shared.rs").write_text("pub fn revision() -> u32 { 1 }\n")
        for name in crates[-8:]:
            with (root / name / "src/lib.rs").open("a") as source:
                source.write('\ninclude!("../../shared.rs");\n')
        stamp = root / "cache/actions/notice/v1/explained"
        stamp.parent.mkdir(parents=True)
        stamp.touch()
        env = dict(os.environ, TERM="xterm-256color", CARGO_HOME=str(root / "cargo-home"), MBX_CACHE_DIR=str(root / "cache"), MBX_TARGET_VIEWS="false", MBX_GC_AUTO="false", MBX_SUMMARY="off", MBX_INCREMENTAL="false", MBX_LEARNED_INCREMENTAL="false", MBX_SAVINGS="off", MBX_STATS_REPORT=str(root / "stats.json"))
        for key in ["CI", "GITHUB_ACTIONS", "NO_COLOR", "CARGO_TERM_COLOR", "CARGO_TERM_PROGRESS_WHEN", "MBX_DISABLE", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]:
            env.pop(key, None)
        # Seed actual cache entries, then force a mixed rebuild in a clean target.
        warm_master, warm_slave = pty.openpty()
        fcntl.ioctl(warm_slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        def attach_warm_terminal():
            os.setsid()
            fcntl.ioctl(warm_slave, termios.TIOCSCTTY, 0)

        warm = subprocess.Popen([str(args.binary.resolve()), "build", "-j", "4"], cwd=root, env=env, stdin=warm_slave, stdout=warm_slave, stderr=warm_slave, preexec_fn=attach_warm_terminal)
        os.close(warm_slave)
        warm_output = bytearray()
        last_warm_frame = time.monotonic()
        warm_deadline = last_warm_frame + 120
        try:
            while time.monotonic() < warm_deadline:
                ready, _, _ = select.select([warm_master], [], [], .1)
                if ready:
                    try:
                        chunk = os.read(warm_master, 65536)
                    except OSError as error:
                        if error.errno == errno.EIO: break
                        raise
                    if not chunk: break
                    warm_output.extend(chunk)
                if warm.poll() is not None and not ready: break
            assert warm.wait(timeout=5) == 0, warm_output
        finally:
            if warm.poll() is None: warm.kill(); warm.wait()
            os.close(warm_master)
        # Retain two Cargo-fresh crates; clean the others so mbx can restore
        # unchanged results and compile the crates affected by the source edit.
        cargo = subprocess.check_output(["rustup", "which", "cargo"], text=True).strip()
        packages = ["hello-boxington", *crates[2:]]
        subprocess.run([cargo, "clean", *[arg for name in packages for arg in ("-p", name)]], cwd=root, env=env, check=True, capture_output=True)
        cold_stats = json.loads((root / "stats.json").read_text())
        assert cold_stats["hits"] == 0 and cold_stats["misses"] + cold_stats.get("unconsulted", 0) > 0
        # One actual shared-source edit invalidates its eight consumers.
        (root / "shared.rs").write_text("pub fn revision() -> u32 { 2 }\n")
        poster = None
        poster_score = -1
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        def attach_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        child = subprocess.Popen([str(args.binary.resolve()), "build", "-j", "4"], cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=attach_terminal)
        os.close(slave)
        transcript = bytearray()
        pending_frame = bytearray()
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
                    feed_frame(data)
                now = time.monotonic()
                if now - last_frame >= 0.08:
                    frame = render(screen, font, title_font)
                    visible = "\n".join(screen.display)
                    if "Compiled (" in visible and re.search(r"[1-9][0-9]* hits", visible) and re.search(r"[1-9][0-9]* misses", visible):
                        score = visible.count("●") * 10 + visible.count("✓")
                        if score >= poster_score:
                            poster = frame.copy()
                            poster_score = score
                    frames.append(frame)
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
        stats = json.loads((root / "stats.json").read_text())
        assert stats["hits"] > 0 and stats["misses"] > 0, stats
        assert b"Compiled (" in transcript and b"Build / cache" in transcript, transcript
        assert poster is not None, "No mixed-cache live frame was captured"
        assert re.search(rb"[1-9][0-9]* fresh", transcript), "missing Cargo-fresh artifacts"
        assert b"\x1b[J" in transcript, "missing terminal redraw"
    stream.feed(b"$ ")
    frames.append(render(screen, font, title_font))
    durations.append(2500)
    output = ROOT / "docs/public/screenshots"
    output.mkdir(parents=True, exist_ok=True)
    poster.save(output / "cargo-pretty.png")
    frames[0].save(output / "cargo-pretty.gif", save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)
    print(f"Recorded {len(frames)} frames ({sum(durations) / 1000:.1f}s; {stats['hits']} hits, {stats['misses']} misses) to {output / 'cargo-pretty.gif'}")


if __name__ == "__main__":
    main()
