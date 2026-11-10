"""Exercise the packaged Mac helper's orphan exit without any display APIs."""
import json
import os
import pathlib
import queue
import signal
import struct
import subprocess
import sys
import threading

PARENT = """
import os, subprocess, sys
child = subprocess.Popen([sys.argv[1], 'self-test-parent'], stdin=subprocess.DEVNULL,
                         stdout=sys.stdout.buffer, stderr=subprocess.DEVNULL)
sys.stdin.buffer.read(1)
os._exit(0 if child.poll() is None else 2)
"""


def exact(stream, count):
    result = bytearray()
    while len(result) < count:
        data = stream.read(count - len(result))
        if not data:
            raise RuntimeError('Helper closed before its readiness record')
        result.extend(data)
    return bytes(result)


def main():
    helper = pathlib.Path(sys.argv[1]).resolve(strict=True)
    parent = subprocess.Popen([sys.executable, '-c', PARENT, str(helper)],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL)
    ready, closed = queue.Queue(), queue.Queue()
    helper_pid = None

    def read():
        try:
            size = struct.unpack('>I', exact(parent.stdout, 4))[0]
            if not 1 <= size <= 4097:
                raise RuntimeError('Unexpected helper readiness size')
            data = exact(parent.stdout, size)
            if data[0] != 1:
                raise RuntimeError('Helper did not enter its no-display test mode')
            ready.put(json.loads(data[1:]))
            if parent.stdout.read(1):
                raise RuntimeError('Parent-watch test unexpectedly emitted more output')
            closed.put(None)
        except Exception as error:
            ready.put(error)
            closed.put(error)

    threading.Thread(target=read, daemon=True).start()
    try:
        state = ready.get(timeout=8)
        if isinstance(state, Exception):
            raise state
        if state.get('mode') != 'parent-watch' or state.get('parent') != parent.pid:
            raise RuntimeError('Helper parent identity did not match the fixture')
        helper_pid = state.get('pid')
        if not isinstance(helper_pid, int) or helper_pid <= 1:
            raise RuntimeError('Helper did not report its test process identity')
        # Allow at least one watchdog tick, then have the real parent poll its
        # child before exiting. Pipe EOF alone cannot prove liveness here,
        # because the parent also holds an open copy of the output descriptor.
        try:
            closed.get(timeout=1.3)
            raise RuntimeError('Helper exited while its owner was still alive')
        except queue.Empty:
            pass
        parent.stdin.write(b'x')
        parent.stdin.flush()
        parent.wait(timeout=3)
        if parent.returncode != 0:
            raise RuntimeError('Helper died before its owner exited')
        result = closed.get(timeout=5)
        if isinstance(result, Exception):
            raise result
        print('PASS: packaged Mac helper exits after owner death without video frames or display access')
    finally:
        if parent.poll() is None:
            parent.kill()
        parent.wait(timeout=5)
        # Failure cleanup is restricted to the fixture helper's exact executable.
        # No user apps or unrelated process IDs are terminated.
        if helper_pid:
            command = subprocess.run(['ps', '-ww', '-p', str(helper_pid), '-o', 'comm='],
                                     capture_output=True, text=True, check=False).stdout.strip()
            if command == str(helper):
                try:
                    os.kill(helper_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass


if __name__ == '__main__':
    main()
