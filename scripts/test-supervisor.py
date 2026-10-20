"""Opt-in isolated crash recovery test. No SSH server or user profiles needed."""
import argparse
import json
import os
import pathlib
import signal
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--agent", default="build/native/Connecty.Agent.exe")
parser.add_argument("--data", required=True)
args = parser.parse_args()
exe, data = pathlib.Path(args.agent).resolve(), pathlib.Path(args.data).resolve()
if data.exists():
    raise SystemExit("Use a fresh test directory")
data.mkdir(parents=True)
launcher = subprocess.Popen([str(exe), "--watchdog", "--data-dir", str(data), "--no-tray"],
                            creationflags=subprocess.CREATE_NO_WINDOW)

def ctl(op):
    p = subprocess.run([str(exe), "--data-dir", str(data), "--ctl", json.dumps({"op": op})],
                       capture_output=True, text=True, timeout=10, check=True)
    return json.loads(p.stdout)["snapshot"]

def wait_for_child(previous=None):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            snapshot = ctl("snapshot")
            pid = snapshot["agentPid"]
            if pid != previous:
                assert pid not in (os.getpid(), launcher.pid)
                return pid
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass
        time.sleep(.1)  # Bounded test orchestration; no production polling.
    raise RuntimeError("Agent recovery deadline expired")

try:
    first = wait_for_child()
    duplicate = subprocess.run([str(exe), "--watchdog", "--data-dir", str(data), "--no-tray"],
                               timeout=5, check=True, creationflags=subprocess.CREATE_NO_WINDOW)
    assert ctl("snapshot")["agentPid"] == first
    began = time.monotonic()
    os.kill(first, signal.SIGTERM)
    second = wait_for_child(first)
    print(f"PASS abnormal child exit recovered in {time.monotonic() - began:.2f}s; launcher stayed alive")
    assert launcher.poll() is None and second != first
    ctl("shutdown")
    assert launcher.wait(timeout=10) == 0
    print("PASS duplicate launch is harmless; clean shutdown exits agent and launcher")
    # A malformed saved file must remain intact even after a configure request.
    profile_file = data / "profiles.json"
    malformed = "{invalid saved configuration"
    profile_file.write_text(malformed, encoding="utf-8")
    launcher = subprocess.Popen([str(exe), "--watchdog", "--data-dir", str(data), "--no-tray"],
                                creationflags=subprocess.CREATE_NO_WINDOW)
    wait_for_child()
    assert ctl("snapshot")["error"]["code"] == "configuration"
    rejected = subprocess.run([str(exe), "--data-dir", str(data), "--ctl",
                               json.dumps({"op": "configure", "profiles": []})],
                              capture_output=True, text=True, timeout=10)
    assert rejected.returncode == 2
    assert json.loads(rejected.stdout)["ok"] is False
    assert profile_file.read_text(encoding="utf-8") == malformed
    ctl("shutdown")
    assert launcher.wait(timeout=10) == 0
    print("PASS corrupt saved profiles are reported and cannot be silently overwritten")
finally:
    if launcher.poll() is None:
        try:
            ctl("shutdown")
            launcher.wait(timeout=10)
        except Exception:
            launcher.terminate()
