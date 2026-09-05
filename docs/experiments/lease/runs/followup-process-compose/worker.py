#!/usr/bin/env python3
import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

p = argparse.ArgumentParser()
p.add_argument("--mode", choices=["ordinary", "ignore-term", "tree", "escaped"], default="ordinary")
p.add_argument("--lifetime", type=float, default=120)
a = p.parse_args()
if not 0 < a.lifetime <= 600:
    p.error("lifetime must be in (0, 600] seconds")
root = Path.cwd()
if not (root / "synthetic-project.json").is_file():
    p.error("run only inside a generated synthetic project")
end = time.monotonic() + a.lifetime
stopping = False

def stop(sig, frame):
    global stopping
    stopping = True

signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, signal.SIG_IGN if a.mode == "ignore-term" else stop)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        ready = (root / "allow-ready").exists()
        body = json.dumps({"project": root.name, "pid": os.getpid(), "ready": ready}).encode()
        self.send_response(200 if ready else 503)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

child = None
if a.mode in ("tree", "escaped"):
    child_root = root / "child"
    child = subprocess.Popen(
        [sys.executable, str(Path(__file__).resolve()), "--lifetime", str(a.lifetime)],
        cwd=child_root,
        start_new_session=(a.mode == "escaped"),
    )

server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
server.timeout = 0.2
identity = {"pid": os.getpid(), "pgid": os.getpgrp(), "sid": os.getsid(0),
            "port": server.server_port, "mode": a.mode,
            "child_pid": child.pid if child else None}
(root / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
print(json.dumps({"event": "started", **identity}), flush=True)
while not stopping and time.monotonic() < end:
    server.handle_request()
server.server_close()
print(json.dumps({"event": "exiting", "pid": os.getpid(),
                  "reason": "signal" if stopping else "fixture-time-limit"}), flush=True)
# Deliberately do not clean up the child: cancellation coverage belongs to the
# incumbent under observation. Every child has its own bounded lifetime.
