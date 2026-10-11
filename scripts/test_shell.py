#!/usr/bin/env python3
"""Real zsh PTY checks against a local fake daemon; does not require GUI/TCC.
Run: python3 scripts/test_shell.py
Use the native IPC helper: RIGA_TEST_BIN=target/debug/riga python3 scripts/test_shell.py
"""
import fcntl
import os
from pathlib import Path
import pty
import queue
import select
import shlex
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent


def check_installer(tmp):
    rc = tmp / "test rc"
    original = "# existing settings\nexport MY_SETTING=hello\n"
    rc.write_text(original)
    installer = str(ROOT / "scripts/install.sh")
    subprocess.run([installer, "--rc", str(rc)], check=True, capture_output=True)
    installed = rc.read_bytes()
    subprocess.run([installer, "--rc", str(rc)], check=True, capture_output=True)
    assert rc.read_bytes() == installed, "install is not idempotent"
    subprocess.run([installer, "--uninstall", "--rc", str(rc)], check=True, capture_output=True)
    assert rc.read_text() == original, "uninstall did not preserve original settings"
    subprocess.run([installer, "--uninstall", "--rc", str(rc)], check=True, capture_output=True)
    assert rc.read_text() == original
    assert Path(str(rc) + ".riga-backup").read_text() == original
    print("PASS installer repeat installation/removal and backup")


def decode(value):
    return value.replace("%09", "\t").replace("%0A", "\n").replace("%0D", "\r").replace("%25", "%")


def encode(value):
    return value.replace("%", "%25").replace("\t", "%09").replace("\n", "%0A").replace("\r", "%0D")


def check_zle(tmp):
    endpoint = tmp / "socket"
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(endpoint))
    listener.listen(1)
    helper = tmp / "riga-helper"
    helper.write_text("#!/usr/bin/env python3\nimport fcntl,os,socket,sys\nfd=int(sys.argv[2])\nsock=socket.socket(fileno=fd)\nsock.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,262144)\nfcntl.fcntl(fd,fcntl.F_SETFL,fcntl.fcntl(fd,fcntl.F_GETFL)|os.O_NONBLOCK)\n")
    helper.chmod(0o755)
    if os.environ.get("RIGA_TEST_BIN"):
        helper = Path(os.environ["RIGA_TEST_BIN"]).resolve(strict=True)
    messages = queue.Queue()
    connection = []

    def receive():
        while True:
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            connection.append(conn)
            pending = b""
            while True:
                try:
                    chunk = conn.recv(65536)
                except OSError:
                    break
                if not chunk:
                    break
                pending += chunk
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    messages.put(line.decode().split("\t"))

    thread = threading.Thread(target=receive, daemon=True)
    thread.start()
    pid, terminal = pty.fork()
    if pid == 0:
        os.environ.update(TERM="xterm-256color", TERM_PROGRAM="iTerm.app", ITERM_SESSION_ID="w0t0p0:test-pane", RIGA_SOCKET=str(endpoint), RIGA_BIN=str(helper))
        os.execvp("zsh", ["zsh", "-f"])
    os.set_blocking(terminal, False)
    captured = bytearray()

    def drain(duration=0.05):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            ready, _, _ = select.select([terminal], [], [], max(0, deadline - time.monotonic()))
            if ready:
                try:
                    captured.extend(os.read(terminal, 65536))
                except (BlockingIOError, OSError):
                    pass

    def send(keys):
        os.write(terminal, keys.encode())

    def expect(kind, predicate=lambda fields: True):
        deadline = time.monotonic() + 4
        seen = []
        while time.monotonic() < deadline:
            try:
                fields = messages.get_nowait()
            except queue.Empty:
                drain(0.02)
                continue
            seen.append(fields[:7])
            seen = seen[-10:]
            if fields[0] == kind and predicate(fields):
                return fields
        raise AssertionError(f"Missing {kind}; observed {seen}; terminal={captured[-3000:]!r}")

    def state(buffer, cursor=None):
        return expect("S", lambda f: decode(f[6]) == buffer and (cursor is None or int(f[4]) == cursor))

    def reply(line):
        connection[-1].sendall((line + "\n").encode())
        drain(0.1)

    try:
        drain(0.15)
        send("PS1='TEST> '; _riga_test_path() { PATH=\"/tmp/riga path%:$PATH\"; }; zle -N _riga_test_path; bindkey '^Xp' _riga_test_path; source " + shlex.quote(str(ROOT / "shell/riga.zsh")) + "\n")
        assert expect("K") == ["K", "^N", "^P", "^I", "^G"]
        initial = state("")
        assert initial[2] == "w0t0p0:test-pane"
        assert len(initial) == 8 and decode(initial[7])
        send("\x18p")
        path_change = state("", 0)
        assert decode(path_change[7]) == "/tmp/riga path%:" + decode(initial[7])
        assert "%25" in path_change[7]
        assert int(path_change[3]) > int(initial[3])
        send("git ch")
        typed = state("git ch", 6)
        assert int(typed[3]) > int(initial[3])
        send("\x02\x02X")
        state("git Xch", 5)
        send("\x15")
        state("")
        send("\x1b[200~echo pasted words\x1b[201~")
        state("echo pasted words", 17)
        send("\x15echo hé😊")
        unicode = state("echo hé😊", 8)
        reply(f"P\t{unicode[3]}\t2")
        send("\t")
        action = expect("A", lambda f: f[3] == "accept")
        assert action[2] == unicode[3]
        reply(f"I\t{unicode[3]}\t5\t8\t" + encode("'hé😊 folder'"))
        state("echo 'hé😊 folder'", 17)
        # A stale acceptance cannot edit a newer buffer, even if it is in flight.
        send("x")
        current = state("echo 'hé😊 folder'x", 18)
        reply(f"I\t{unicode[3]}\t0\t8\tBAD")
        send("y")
        state("echo 'hé😊 folder'xy", 19)
        send("\x15echo history-item\n")
        expect("E")
        state("")
        # Popup hidden: Ctrl-P must retain zsh's ordinary history behavior.
        send("\x10")
        history = state("echo history-item", 17)
        reply(f"P\t{history[3]}\t2")
        send("\x0e")
        expect("A", lambda f: f[3] == "next")
        send("\x07")
        expect("A", lambda f: f[3] == "dismiss")
        # New state activates acceptance; text containing shell syntax is only inserted.
        send("x")
        before = state("echo history-itemx", 18)
        marker = tmp / "must-not-exist"
        insertion = "echo executed > " + shlex.quote(str(marker))
        reply(f"P\t{before[3]}\t1")
        send("\t")
        expect("A", lambda f: f[3] == "accept")
        reply(f"I\t{before[3]}\t0\t18\t" + encode(insertion))
        state(insertion, len(insertion))
        assert not marker.exists(), "completion executed command"
        # A `# request` line: an AI command replaces the whole buffer and is never executed.
        ai_marker = tmp / "ai-must-not-exist"
        prompt = "# make a marker file here 🌍"
        send("\x15" + prompt)
        request = state(prompt, len(prompt))
        command = "touch " + shlex.quote(str(ai_marker)) + " && echo done"
        reply(f"P\t{request[3]}\t1")
        send("\t")
        expect("A", lambda f: f[3] == "accept")
        reply(f"I\t{request[3]}\t0\t{len(prompt)}\t" + encode(command))
        state(command, len(command))
        drain(0.3)
        assert not ai_marker.exists(), "AI suggestion executed"
        # Reload must discard saved macro metadata before capturing a new widget.
        source = shlex.quote(str(ROOT / "shell/riga.zsh"))
        send("\x15riga-unload; bindkey -s '^N' 'macro-original'; source " + source + "\n")
        state("")
        send("\x0e")
        state("macro-original", 14)
        send("\x15riga-unload; _riga_test_fallback() { BUFFER='ordinary-widget'; CURSOR=${#BUFFER}; }; zle -N _riga_test_fallback; bindkey '^N' _riga_test_fallback; source " + source + "\n")
        state("")
        send("\x0e")
        state("ordinary-widget", 15)
        restored_binding = tmp / "restored-binding"
        send("\x15riga-unload; bindkey -L -M emacs '^N' > " + shlex.quote(str(restored_binding)) + "\n")
        deadline = time.monotonic() + 4
        while not restored_binding.exists() and time.monotonic() < deadline:
            drain(0.02)
        assert restored_binding.read_text().strip() == 'bindkey -M emacs "^N" _riga_test_fallback'
        print("PASS unload/reload from macro to ordinary widget, hidden fallback, and restored binding")
        # Clear input before leaving the test shell.
        send("\x15exit\n")
        print("PASS ZLE PATH changes, typing, middle editing, bracketed paste, Unicode, history, stale replies, navigation/dismissal, insertion without execution")
    finally:
        try:
            # Interactive zsh may ignore SIGTERM; always reap this test-owned child.
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
        os.close(terminal)
        os.waitpid(pid, 0)
        for conn in connection:
            conn.close()
        listener.close()


with tempfile.TemporaryDirectory(prefix="riga-zle-", dir="/tmp") as directory:
    temporary = Path(directory)
    check_installer(temporary)
    check_zle(temporary)
