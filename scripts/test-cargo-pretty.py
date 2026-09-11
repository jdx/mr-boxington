#!/usr/bin/env python3
"""Real-terminal regression checks for the Cargo presentation adapter (Unix).
Run after cargo build -p mbx. Uses only Python's standard library.
"""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import shutil
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('MBX_BIN', ROOT / 'target/debug/mbx')).resolve()


def terminal_run(root, env, args, *, respond=None, limit=90):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 110, 0, 0))
    before = termios.tcgetattr(slave)
    def attach_terminal():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    child = subprocess.Popen([str(BINARY), *args], cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=attach_terminal)
    data = bytearray()
    deadline = time.monotonic() + limit
    sent = False
    dismissed = False
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], .1)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError as e:
                    if e.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                data.extend(chunk)
            if respond and not sent and respond[0] in data:
                os.write(master, respond[1])
                sent = True
            # Browse warnings and failed tests without leaving a test hung.
            if not dismissed and (b'Esc dismiss' in data or b'Enter inspect failure' in data):
                os.write(master, b'q')
                dismissed = True
            if child.poll() is not None and not ready:
                break
        else:
            with tempfile.NamedTemporaryFile(prefix='mbx-pretty-timeout-', suffix='.log', delete=False) as log:
                log.write(data)
                log_path = log.name
            raise AssertionError(f'Timed out: {args}; transcript: {log_path}')
        code = child.wait(timeout=5)
        after = termios.tcgetattr(master)
        assert before == after, f'terminal mode was not restored: {args}: {before!r} != {after!r}'
        return code, bytes(data)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)


def environment(root):
    env = dict(os.environ, TERM='xterm-256color', MBX_CACHE_DIR=str(root/'cache'), MBX_TARGET_VIEWS='false', MBX_GC_AUTO='false', MBX_SUMMARY='off', MBX_SAVINGS='off', MBX_STATS_REPORT=str(root/'stats.json'))
    for key in ['CI', 'GITHUB_ACTIONS', 'MBX_DISPLAY', 'NO_COLOR', 'CARGO_TERM_COLOR', 'CARGO_TERM_PROGRESS_WHEN', 'CARGO_TERM_PROGRESS_WIDTH', 'MBX_DISABLE', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER']:
        env.pop(key, None)
    stamp = root/'cache/actions/notice/v1/explained'
    stamp.parent.mkdir(parents=True)
    stamp.touch()
    return env


def main():
    with tempfile.TemporaryDirectory(prefix='mbx-pretty-tests-') as tmp:
        root = Path(tmp)
        (root/'src').mkdir()
        (root/'tests').mkdir()
        (root/'dep/src').mkdir(parents=True)
        (root/'Cargo.toml').write_text('[package]\nname="pretty-check"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ndep={path="dep"}\n[[test]]\nname="custom"\nharness=false\n')
        (root/'dep/Cargo.toml').write_text('[package]\nname="dep"\nversion="0.1.0"\nedition="2024"\n')
        (root/'dep/src/lib.rs').write_text('pub fn value() -> u32 { 42 }\n')
        (root/'src/main.rs').write_text('''use std::io::{IsTerminal, Write};
fn main() {
 assert!(std::io::stdin().is_terminal());
 assert!(std::io::stdout().is_terminal());
 assert!(std::io::stderr().is_terminal());
 assert!(std::env::var_os("CARGO_TERM_PROGRESS_WHEN").is_none());
 assert!(std::env::var_os("CARGO_TERM_PROGRESS_WIDTH").is_none());
 println!("ARG:{}", std::env::args().nth(1).unwrap_or_default());
 println!("INPUT_READY"); std::io::stdout().flush().unwrap();
 let mut line = String::new(); std::io::stdin().read_line(&mut line).unwrap();
 println!("INPUT:{}", line.trim());
 assert_eq!(dep::value(), 42);
}
''')
        (root/'src/lib.rs').write_text('''/// ```
/// assert_eq!(pretty_check::answer(), 42);
/// ```
pub fn answer() -> u32 { dep::value() }
#[test] fn works() { assert_eq!(answer(), 42); }
#[test] fn fails_when_requested() { assert!(std::env::var_os("PRETTY_FAIL").is_none(), "visible failure detail"); }
''')
        (root/'tests/custom.rs').write_text('fn main() { println!("CUSTOM_HARNESS_OUTPUT"); }\n')
        env = environment(root)
        toolchain = subprocess.check_output(['rustup', 'show', 'active-toolchain'], text=True).split()[0]
        for args in [['build'], ['check'], ['clippy'], ['+' + toolchain, 'build']]:
            code, output = terminal_run(root, env, args)
            assert code == 0, output[-3000:]
            assert b'Build / cache' in output, output[-3000:]
            assert b'\x1b[?2026h' in output
            assert output.count(b'\x1b[?2026l') >= output.count(b'\x1b[?2026h')
        code, output = terminal_run(root, env, ['build'])
        assert code == 0 and b'fresh' in output
        shutil.rmtree(root/'target')
        code, output = terminal_run(root, env, ['build'])
        stats = json.loads((root/'stats.json').read_text())
        assert code == 0 and stats['hits'] > 0, (stats, output[-3000:])
        assert f"{stats['hits']} hits".encode() in output
        code, output = terminal_run(root, env, ['run', '--', 'argument with spaces'], respond=(b'INPUT_READY', b'hello\n'))
        assert code == 0 and b'ARG:argument with spaces' in output and b'INPUT:hello' in output, output[-3000:]
        code, output = terminal_run(root, env, ['test'])
        assert code == 0 and b'CUSTOM_HARNESS_OUTPUT' in output and b'Doc-tests' in output, output[-3000:]
        # A custom harness may leave a descendant holding Cargo's PTY open.
        # It must not hold mbx in raw mode, and the harness tail must survive.
        (root/'tests/custom.rs').write_text('''fn main() {
 let child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
 std::fs::write("background.pid", child.id().to_string()).unwrap();
 for _ in 0..100000 { println!("TRAILING_HARNESS_OUTPUT"); }
 println!("HARNESS_TAIL_COMPLETE");
}''')
        try:
            code, output = terminal_run(root, env, ['test', '--test', 'custom'], limit=20)
            assert code == 0 and b'HARNESS_TAIL_COMPLETE' in output, output[-3000:]
        finally:
            if (root/'background.pid').exists():
                try:
                    os.kill(int((root/'background.pid').read_text()), signal.SIGTERM)
                except ProcessLookupError:
                    pass
        code, output = terminal_run(root, dict(env, PRETTY_FAIL='1'), ['test', '--lib'])
        assert code == 101 and b'visible failure detail' in output, output[-3000:]
        host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
        runner = root/'runner.sh'
        runner.write_text('#!/bin/sh\nprintf "RUNNER_USED\\n"\nexec "$@"\n')
        runner.chmod(0o755)
        runner_env = dict(env, **{f'CARGO_TARGET_{host.replace("-", "_").upper()}_RUNNER': str(runner)})
        code, output = terminal_run(root, runner_env, ['test', '--lib', 'works'])
        assert code == 0 and b'RUNNER_USED' in output, output[-3000:]
        code, output = terminal_run(root, env, ['run'], respond=(b'INPUT_READY', b'\x03'))
        # Native application handoff may terminate mbx with SIGINT directly.
        assert code in (130, -signal.SIGINT), (code, output[-3000:])
        result = subprocess.run([str(BINARY), 'build', '--message-format=json'], cwd=root, env=env, capture_output=True)
        assert result.returncode == 0 and b'compiler-artifact' in result.stdout and b'Build / cache' not in result.stderr
        (root/'src/lib.rs').write_text('pub fn bad() { let unused = 1; }\n')
        code, output = terminal_run(root, env, ['check'])
        assert code == 0 and b'Esc dismiss' not in output and b'unused' in output, output[-3000:]
        assert not re.search(rb'(?<!\r)\n', output), 'warning diagnostics need CRLF in raw mode'
        code, output = terminal_run(root, dict(env, MBX_PRETTY_INSPECT='1'), ['check'])
        assert code == 0 and b'Build completed with warnings' in output
        for level in ['debug', 'mbx_cache_core=trace']:
            code, output = terminal_run(root, dict(env, MBX_LOG=level), ['check'])
            assert code == 0 and b'\x1b[?2026h' not in output and b'Build / cache' not in output
        code, output = terminal_run(root, dict(env, MBX_DISPLAY='plain'), ['run', '--', 'plain mode'], respond=(b'INPUT_READY', b'hello\n'))
        assert code == 0 and b'ARG:plain mode' in output
        code, output = terminal_run(root, dict(env, MBX_DISPLAY='plain'), ['check'])
        assert code == 0 and b'\x1b[?2026h' not in output and b'Build / cache' not in output
        (root/'src/lib.rs').write_text('this does not compile\n')
        code, output = terminal_run(root, env, ['run'])
        assert code == 101 and b'error' in output, output[-3000:]
        assert not re.search(rb'(?<!\r)\n', output), 'error diagnostics need CRLF in raw mode'
        agent = root/'agent'
        (agent/'src').mkdir(parents=True)
        (agent/'Cargo.toml').write_text('[package]\nname="agent-output"\nversion="0.1.0"\nedition="2024"\n')
        (agent/'src/lib.rs').write_text('pub fn value() {}\n')
        (agent/'build.rs').write_text('fn main() { std::thread::sleep(std::time::Duration::from_secs(16)); }\n')
        agent_env = dict(environment(agent), MBX_SUMMARY='short')
        result = subprocess.run([str(BINARY), 'check'], cwd=agent, env=agent_env, capture_output=True, timeout=60)
        assert result.returncode == 0 and b'mbx[progress]:' in result.stderr, result.stderr
        assert b'\x1b' not in result.stderr and b'\r' not in result.stderr
        assert not result.stdout
        assert result.stderr.count(b'mr boxington') == 1, result.stderr
        assert '◀◆▶'.encode() in result.stderr
        result = subprocess.run([str(BINARY), 'check', '--message-format=json'], cwd=agent, env=agent_env, capture_output=True, timeout=60)
        assert result.returncode == 0 and b'mbx[progress]:' not in result.stderr
        assert b'mr boxington' not in result.stderr
        assert all(isinstance(json.loads(line), dict) for line in result.stdout.splitlines())
        print('Passed: build/check/clippy, fresh/warm cache counts, interactive run, tests/doctests, custom harness, runner, failure diagnostics, cancellation, JSON passthrough, warning browser, terminal restoration.')


if __name__ == '__main__':
    main()
