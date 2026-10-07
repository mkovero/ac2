"""REW's REST API (5.40, API 0.9.8) over urllib: the calls the suite needs, and save /
restore of every setting it changes. REW plays nothing here: its sweep is played by the
suite's JACK client and imported as a recording (live measurement needs REW Pro)."""
from __future__ import annotations

import json
import time
import urllib.error
import urllib.request
from pathlib import Path


class RewError(Exception):
    pass


class Rew:
    def __init__(self, api: str, timeout: float = 30.0):
        self.api = api.rstrip("/")
        self.timeout = timeout

    def _req(self, method: str, path: str, body=None):
        data = None if body is None else json.dumps(body).encode()
        r = urllib.request.Request(self.api + path, data=data, method=method,
                                   headers={"Content-Type": "application/json", "Accept": "application/json"})
        try:
            with urllib.request.urlopen(r, timeout=self.timeout) as f:
                txt = f.read().decode()
        except urllib.error.HTTPError as e:
            raise RewError(f"{method} {path}: HTTP {e.code} {e.read().decode(errors='replace')[:300]}") from e
        except urllib.error.URLError as e:
            raise RewError(f"{method} {path}: {e.reason}") from e
        return json.loads(txt) if txt.strip() else None

    def get(self, path):
        return self._req("GET", path)

    def put(self, path, body):
        return self._req("PUT", path, body)

    def post(self, path, body):
        return self._req("POST", path, body)

    def delete(self, path):
        return self._req("DELETE", path)

    # ---------------------------------------------------------------- state
    def version(self) -> str:
        return (self.get("/version") or {}).get("message", "?")

    def spl_meters(self) -> list[str]:
        """Ids of the SPL meters REW has open; the API cannot open more."""
        ids = []
        for m in ("1", "2", "3"):
            try:
                self.get(f"/spl-meter/{m}/configuration")
                ids.append(m)
            except RewError:
                pass
        return ids

    def snapshot(self) -> dict:
        """Everything the suite may change, to put back afterwards."""
        s = {"input_cal": self.get("/audio/input-cal"), "stimulus": self.get("/import/sweep-recordings/stimulus"),
             "spl": {}, "blocking": self.get("/application/blocking")}
        for m in ("1", "2", "3"):
            try:
                s["spl"][m] = self.get(f"/spl-meter/{m}/configuration")
            except RewError:
                pass
        try:
            s["rta"] = self.get("/rta/configuration")
        except RewError:
            pass
        return s

    def restore(self, s: dict) -> list[str]:
        errs = []
        for what, fn in (("input-cal", lambda: self.put("/audio/input-cal", s["input_cal"])),
                         ("stimulus", lambda: s.get("stimulus") and self.post("/import/sweep-recordings/stimulus", s["stimulus"])),
                         ("rta", lambda: s.get("rta") and self.put("/rta/configuration", s["rta"]))):
            try:
                fn()
            except RewError as e:
                errs.append(f"{what}: {e}")
        for m, c in s.get("spl", {}).items():
            try:
                self.command(f"/spl-meter/{m}/command", "Stop")
                self.put(f"/spl-meter/{m}/configuration", c)
            except RewError as e:
                errs.append(f"spl-meter {m}: {e}")
        return errs

    def command(self, path: str, cmd: str, params=None):
        return self.post(path, {"command": cmd, **({"parameters": params} if params else {})})

    # ---------------------------------------------------------------- calibration
    def set_input_cal(self, dbfs_at_94: float, fs_sine_vrms: float, cal_file: str = "") -> dict:
        """All inputs share one cal (REW's import applies it with applyCal)."""
        cur = self.get("/audio/input-cal") or {}
        cur["separateCalFileForEachInput"] = False
        cur["calDataAllInputs"] = {"dBFSAt94dBSPL": dbfs_at_94, "fullScaleSineVrms": fs_sine_vrms,
                                   "calFilePath": cal_file}
        self.put("/audio/input-cal", cur)
        back = self.get("/audio/input-cal")
        got = (back or {}).get("calDataAllInputs") or {}
        if abs(got.get("dBFSAt94dBSPL", 1e9) - dbfs_at_94) > 1e-3 or abs(got.get("fullScaleSineVrms", 1e9) - fs_sine_vrms) > 1e-6:
            raise RewError(f"REW did not take the input calibration: {got}")
        return back

    # ---------------------------------------------------------------- import
    def ids(self) -> list[str]:
        return list((self.get("/measurements") or {}).keys())

    def import_response(self, stimulus: str, response: str, channel: int, apply_cal: bool,
                        wait_s: float = 60.0) -> str:
        """Imports one channel of a recording against the stimulus; returns the new id."""
        before = set(self.ids())
        self.post("/import/sweep-recordings/stimulus", stimulus)
        self.post("/import/sweep-recordings/response", {"path": response, "channels": str(channel),
                                                        "applyCal": apply_cal})
        t0 = time.monotonic()
        while time.monotonic() - t0 < wait_s:
            new = [i for i in self.ids() if i not in before]
            if new:
                return new[-1]
            time.sleep(0.5)
        raise RewError(f"REW made no measurement from {response} ch {channel} in {wait_s:g} s "
                       f"(last error: {self.get('/application/last-error')})")

    def fetch(self, mid: str, out: Path, prefix: str, spl: bool = False) -> None:
        """The exports the analysis reads, as REW returns them."""
        q = {f"{prefix}_fr": "/frequency-response?unit=dBFS&smoothing=None",
             f"{prefix}_ir": "/impulse-response?normalised=false&unit=percent",
             f"{prefix}_gd": "/group-delay?smoothing=None",
             f"{prefix}_dist": "/distortion?unit=dBr&ppo=48",
             f"{prefix}_summary": ""}
        if spl:
            q[f"{prefix}_fr_spl"] = "/frequency-response?unit=SPL&smoothing=None"
            try:
                # REW computes decay data on request only
                self.command(f"/measurements/{mid}/command", "Generate RT60")
                time.sleep(2.0)
            except RewError:
                pass
            q[f"{prefix}_rt60"] = "/rt60?octaveFrac=1"
            q[f"{prefix}_rt60_settings"] = "/rt60-settings"
        for name, sub in q.items():
            try:
                j = self.get(f"/measurements/{mid}{sub}")
            except RewError as e:
                j = {"error": str(e)}
            (out / f"{name}.json").write_text(json.dumps(j))
