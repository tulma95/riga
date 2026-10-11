#!/usr/bin/env python3
"""Exercise the real native daemon's IPC lifecycle without displaying a popup.

Uses synthetic pane UUIDs, so no test completion can target a real iTerm2 pane.
Build first: cargo build; python3 scripts/test_daemon.py
"""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def state(session, revision, pane="w0t0p0:riga-test-nonexistent-pane"):
    return f"S\t{session}\t{pane}\t{revision}\t6\t/tmp\tgit st\t/usr/bin:/bin\n".encode()


def expect(sock, expected):
    data = b""
    while not data.endswith(b"\n"):
        chunk = sock.recv(4096)
        assert chunk, "daemon disconnected unexpectedly"
        data += chunk
    assert data == expected, (data, expected)


def quiet(sock):
    sock.settimeout(0.15)
    try:
        data = sock.recv(4096)
        assert not data, f"unexpected reply {data!r}"
    except socket.timeout:
        pass
    finally:
        sock.settimeout(4)


def main():
    with tempfile.TemporaryDirectory(prefix="riga-ipc-") as tmp:
        endpoint = Path(tmp) / "socket"
        environment = dict(os.environ, RIGA_SOCKET=str(endpoint))
        daemon = subprocess.Popen(
            [str(ROOT / "target/debug/riga"), "serve"],
            env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        clients = []
        try:
            deadline = time.monotonic() + 5
            while not endpoint.exists():
                assert daemon.poll() is None, "daemon failed during startup"
                assert time.monotonic() < deadline, "socket startup timed out"
                time.sleep(0.02)
            assert endpoint.stat().st_mode & 0o777 == 0o600
            assert Path(tmp).stat().st_mode & 0o777 == 0o700
            for _ in range(2):
                client = socket.socket(socket.AF_UNIX)
                client.settimeout(4)
                client.connect(str(endpoint))
                clients.append(client)
            first, second = clients
            # Fragmented stream frames must be reassembled before parsing.
            frame = state("one", 2)
            first.sendall(frame[:9])
            time.sleep(0.03)
            first.sendall(frame[9:])
            expect(first, b"P\t2\t0\n")
            first.sendall(state("one", 1))
            quiet(first)
            # A second connection may neither hijack a session nor end it.
            second.sendall(state("one", 3))
            quiet(second)
            second.sendall(b"E\tone\t100\n")
            quiet(second)
            first.sendall(state("one", 3))
            expect(first, b"P\t3\t0\n")
            second.sendall(state("two", 1, "w0t1p0:another-test-pane"))
            expect(second, b"P\t1\t0\n")
            first.sendall(b"E\tone\t4\n")
            expect(first, b"P\t4\t0\n")
            first.sendall(state("one", 3))
            quiet(first)
            # Acceptance with an unverified pane must never produce an edit.
            second.sendall(b"A\ttwo\t1\taccept\n")
            expect(second, b"P\t1\t0\n")
            # Disconnect removes ownership so a reconnect can start afresh.
            first.close()
            time.sleep(0.15)
            replacement = socket.socket(socket.AF_UNIX)
            replacement.settimeout(4)
            replacement.connect(str(endpoint))
            clients.append(replacement)
            replacement.sendall(state("one", 1))
            expect(replacement, b"P\t1\t0\n")
            duplicate = subprocess.run(
                [str(ROOT / "target/debug/riga"), "serve"],
                env=environment, capture_output=True, timeout=5,
            )
            assert duplicate.returncode != 0, "duplicate server replaced live socket"
            assert b"already running" in duplicate.stderr
            assert daemon.poll() is None
        finally:
            for client in clients:
                client.close()
            daemon.terminate()
            out, err = daemon.communicate(timeout=5)
            assert b"git st" not in out + err, "command buffer leaked into logs"
        print("PASS real daemon: private socket, fragmented frames, revisions, session ownership, end/disconnect, fail-closed acceptance, duplicate startup, no buffer logs")


if __name__ == "__main__":
    main()
