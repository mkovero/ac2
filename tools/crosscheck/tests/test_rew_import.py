"""The REW import step against a fake REW API that numbers measurements by list position, as REW
does: everything the suite makes is found, divided and deleted by UUID, and the operator's own
measurements stay."""
import json
import uuid

import numpy as np

from crosscheck import rewimport, stages, wav
from crosscheck.rew import Rew


class FakeRew(Rew):
    def __init__(self, own=("rec1_in2", "rec1_in5")):
        super().__init__("http://fake")
        self.ms = [{"title": t, "uuid": str(uuid.uuid4())} for t in own]
        self.calls = []
        self.input_cal = {"calDataAllInputs": {"dBFSAt94dBSPL": -20.0, "fullScaleSineVrms": 1.0, "calFilePath": ""}}
        self.result = {"processName": "", "message": "There is no process"}
        self.n = 0

    def _find(self, key):
        if key.isdigit():
            return int(key) - 1
        return next(i for i, m in enumerate(self.ms) if m["uuid"] == key)

    def _req(self, method, path, body=None):
        self.calls.append((method, path, body))
        path = path.split("?")[0]
        if path == "/measurements" and method == "GET":
            return {str(i + 1): m for i, m in enumerate(self.ms)}
        if path == "/import/sweep-recordings/response":
            self.ms.append({"title": body["path"].rsplit("/", 1)[-1][:-4], "uuid": str(uuid.uuid4())})
            return {}
        if path == "/measurements/process-measurements":
            assert body["processName"] == "Arithmetic" and body["parameters"] == {"function": "A / B"}
            assert all(not u.isdigit() for u in body["measurementUUIDs"])
            self.n += 1
            m = {"title": "A over B", "uuid": str(uuid.uuid4())}
            self.ms.append(m)
            self.result = {"processName": f"Arithmetic ID {self.n}", "message": "Completed",
                           "results": {str(len(self.ms)): {"New measurement": "A over B", "UUID": m["uuid"]}}}
            return {"message": f"Arithmetic ID {self.n} in progress"}
        if path == "/measurements/process-result":
            return self.result
        if path == "/audio/input-cal":
            if method == "PUT":
                self.input_cal = json.loads(json.dumps(body))
                return {}
            return json.loads(json.dumps(self.input_cal))
        if path == "/version":
            return {"message": "fake 0.9.8"}
        if path.startswith("/measurements/"):
            key = path.split("/")[2]
            i = self._find(key)
            if method == "DELETE":
                self.ms.pop(i)  # every later index moves down by one
                return {}
            if method == "PUT":
                self.ms[i].update(body)
                return {}
            return {"uuid": self.ms[i]["uuid"], "title": self.ms[i]["title"]}
        return {}


def _recordings(d):
    d.mkdir(parents=True)
    x = np.zeros((960, 1))
    x[10] = 0.5
    wav.write(d / "rec_meas.wav", 96000.0, x)
    wav.write(d / "rec_ref.wav", 96000.0, x)


def test_import_divides_and_deletes_only_its_own(tmp_path):
    r = FakeRew()
    own = [m["uuid"] for m in r.ms]
    d = tmp_path / "xone" / "rew"
    _recordings(d)
    ids = stages.rew_import(r, d, "/stim.wav", "xone", speaker=False, keep=False)
    assert [m["uuid"] for m in r.ms] == own
    assert sorted(ids["deleted"]) == sorted([ids["meas"], ids["ref"], ids["div"]])
    assert json.loads((d / "rew_ids.json").read_text()) == ids
    for p in ("meas", "ref", "div"):
        assert (d / f"{p}_fr.json").exists() and (d / f"{p}_ir.json").exists() and (d / f"{p}_gd.json").exists()
    assert not (d / "div_dist.json").exists()  # a quotient has no distortion data
    imports = [b for m, p, b in r.calls if p == "/import/sweep-recordings/response"]
    assert [b["applyCal"] for b in imports] == [False, False]


def test_keep_measurements_keeps_them(tmp_path):
    r = FakeRew()
    d = tmp_path / "genelec" / "rew"
    _recordings(d)
    ids = stages.rew_import(r, d, "/stim.wav", "genelec", speaker=True, keep=True)
    assert len(r.ms) == 5 and "deleted" not in ids
    imports = [b for m, p, b in r.calls if p == "/import/sweep-recordings/response"]
    assert [b["applyCal"] for b in imports] == [True, False]  # the mic channel only


def test_reimport_sets_the_runs_cal_and_restores(tmp_path):
    run = tmp_path / "run"
    _recordings(run / "xone" / "rew")
    (run / "xone" / "rew" / "play.json").write_text(json.dumps({"stimulus": "/stim.wav", "speaker": False}))
    (run / "cal").mkdir()
    (run / "cal" / "store_excerpt.json").write_text(json.dumps({"rew_dbfs_at_94": -26.468, "full_scale_vrms": 0.27176}))
    (run / "cal" / "mic_curve_rew.txt").write_text("1000 0\n")
    (run / "manifest.json").write_text(json.dumps({"rew_version": "x"}))
    (run / "rig.toml").write_text('[rew]\napi = "http://fake"\nkeep_measurements = false\n')
    r = FakeRew()
    before = json.loads(json.dumps(r.input_cal))
    seen = []
    orig = stages.rew_import

    def spy(rew, *a, **k):
        seen.append(dict(rew.input_cal["calDataAllInputs"]))
        return orig(rew, *a, **k)

    stages.rew_import, saved = spy, stages.rew_import
    try:
        rec = rewimport.reimport(run, None, rew=r, suite_commit="abc1234")
    finally:
        stages.rew_import = saved
    assert seen[0]["dBFSAt94dBSPL"] == -26.468 and seen[0]["calFilePath"].endswith("mic_curve_rew.txt")
    assert r.input_cal == before  # put back
    assert rec["rew_restored"] and rec["suite_commit"] == "abc1234" and list(rec["stages"]) == ["xone"]
    side = json.loads((run / rewimport.SIDECAR).read_text())
    assert side[-1]["stages"]["xone"]["rew_ids"]["div"]
    assert len(r.ms) == 2
