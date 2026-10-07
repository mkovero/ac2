"""The ac2 CLI as the suite drives it. Foreground commands that emit (`sweep run`,
`gen pink`) take Enter on stdin to fire and `q` to stop; stdin reaching EOF also quits, so
the pipe stays open until the command has finished. Every call has a deadline."""
from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from pathlib import Path


class Ac2Error(Exception):
    pass


def _expand(cmd: list[str]) -> list[str]:
    return [os.path.expanduser(c) if c.startswith("~/") else c for c in cmd]


class Ac2:
    def __init__(self, cmd: list[str], timeout: str = "5s", log: Path | None = None):
        self.cmd = _expand(cmd)
        self.timeout = timeout
        self.log = log

    def _log(self, line: str):
        if self.log:
            with open(self.log, "a") as f:
                f.write(line.rstrip("\n") + "\n")

    def argv(self, *args) -> list[str]:
        return self.cmd + ["--timeout", self.timeout] + [str(a) for a in args]

    def run(self, *args, json_out: bool = True, deadline: float = 60.0, check: bool = True):
        argv = self.argv(*args, *(["--json"] if json_out else []))
        self._log("$ " + " ".join(argv))
        p = subprocess.run(argv, capture_output=True, text=True, timeout=deadline, stdin=subprocess.DEVNULL)
        self._log(p.stdout[-4000:] + p.stderr[-2000:])
        if check and p.returncode != 0:
            raise Ac2Error(f"ac2 {' '.join(map(str, args))}: exit {p.returncode}: {p.stderr.strip()[-500:]}")
        if not json_out:
            return p.stdout
        txt = p.stdout.strip()
        if not txt:
            return None
        try:
            return json.loads(txt)
        except json.JSONDecodeError:
            return [json.loads(x) for x in txt.splitlines() if x.strip().startswith("{")]

    # ---------------------------------------------------------------- queries
    def status(self):
        return self.run("status")

    def ceiling(self) -> dict:
        return self.run("gen", "ceiling")

    def cal_list(self):
        return self.run("cal", "list")

    def meas_list(self):
        return self.run("meas", "list") or []

    def meas_id(self, name: str):
        for m in self.meas_list():
            if m.get("config", {}).get("name") == name:
                return m["id"]
        return None

    def meas_rm(self, name: str):
        if self.meas_id(name) is not None:
            self.run("meas", "rm", name, "--delete-traces", json_out=False, check=False)

    def export_trace(self, trace, path: Path):
        path.write_text(self.run("trace", "export", str(trace), "--csv", "-", json_out=False, deadline=120))

    def capture(self, meas: str, name: str):
        return self.run("trace", "capture", meas, "--name", name)

    # ---------------------------------------------------------------- recordings
    def rec_start(self, inputs: list[int], seconds: float, name: str) -> dict:
        return self.run("rec", "start", "--in", ",".join(map(str, inputs)), "--max", f"{seconds:.1f}s", "--name", name)

    def rec_stop(self) -> dict | None:
        return self.run("rec", "stop", check=False)

    # ---------------------------------------------------------------- foreground emitters
    def foreground(self, args: list, until_event: str | None, deadline: float, events_path: Path,
                   hold_s: float | None = None, before_stop=None) -> list[dict]:
        """Starts a foreground command, fires it with Enter, collects its JSON lines; ends at
        `until_event` (sweep) or after hold_s (generator), then sends q and waits for exit.
        On any error or Ctrl-C it sends q, then terminates: the daemon fades its stimulus out
        when the client goes."""
        argv = self.argv(*args, "--json")
        self._log("$ " + " ".join(argv))
        p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             bufsize=1)
        events: list[dict] = []
        got = threading.Event()

        def reader():
            with open(events_path, "a") as f:
                for line in p.stdout:
                    f.write(line)
                    s = line.strip()
                    if s.startswith("{"):
                        try:
                            e = json.loads(s)
                        except json.JSONDecodeError:
                            continue
                        events.append(e)
                        if until_event and e.get("event") == until_event:
                            got.set()

        t = threading.Thread(target=reader, daemon=True)
        t.start()
        try:
            p.stdin.write("\n")
            p.stdin.flush()
            if until_event:
                if not got.wait(deadline):
                    raise Ac2Error(f"ac2 {args[0]} {args[1]}: no '{until_event}' within {deadline:g} s")
            else:
                t0 = time.monotonic()
                while time.monotonic() - t0 < hold_s:
                    if p.poll() is not None:
                        raise Ac2Error(f"ac2 {' '.join(map(str, args))} exited early: {p.stderr.read()[-500:]}")
                    time.sleep(0.2)
                if before_stop:
                    before_stop()
            return events
        finally:
            try:
                p.stdin.write("q\n")
                p.stdin.flush()
                p.stdin.close()
            except (BrokenPipeError, ValueError, OSError):
                pass
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.terminate()
                p.wait(5)
            err = p.stderr.read()
            if err.strip():
                self._log(err[-2000:])
            for e in events:
                if e.get("event") in ("error", "refused"):
                    raise Ac2Error(f"ac2 {' '.join(map(str, args))}: {e}")
