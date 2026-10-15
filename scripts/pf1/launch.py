from contextlib import contextmanager
import os
import signal
import subprocess

@contextmanager
def owned_child(command, **options):
    process = subprocess.Popen(command, start_new_session=True, **options)
    try:
        yield process
    finally:
        for sig in [signal.SIGTERM, signal.SIGKILL]:
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=1)
                os.killpg(process.pid, 0)
            except ProcessLookupError:
                break
            except subprocess.TimeoutExpired:
                pass
        process.wait(timeout=1)
