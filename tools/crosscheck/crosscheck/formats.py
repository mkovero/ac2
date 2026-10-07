"""Readers for what ac2 and REW export."""
from __future__ import annotations

import base64
import json
from dataclasses import dataclass, field

import numpy as np


@dataclass
class Ac2Trace:
    """An `ac2 trace export --csv` file: the header (`# key: value`, JSON where it is JSON)
    and its data blocks by header row (`freq_hz,...`, `t_s,...`)."""

    meta: dict = field(default_factory=dict)
    blocks: dict[str, dict[str, np.ndarray]] = field(default_factory=dict)

    @property
    def freq(self) -> dict[str, np.ndarray]:
        return self.blocks["freq_hz"]

    @property
    def ir(self) -> dict[str, np.ndarray] | None:
        return self.blocks.get("t_s")

    def complex_response(self):
        b = self.freq
        f, m, p = b["freq_hz"], b["mag_db"], b["phase_deg"]
        ok = np.isfinite(m) & np.isfinite(p)
        return f[ok], 10 ** (m[ok] / 20) * np.exp(1j * np.deg2rad(p[ok])), ok

    @property
    def sweep_info(self) -> dict | None:
        return self.meta.get("sweep_info")

    @property
    def room_metrics(self) -> dict | None:
        return self.meta.get("room_metrics")


def _num(s: str) -> float:
    s = s.strip()
    try:
        return float(s)
    except ValueError:
        return np.nan


def read_ac2_csv(path) -> Ac2Trace:
    t = Ac2Trace()
    cols: list[str] | None = None
    rows: list[list[float]] = []

    def flush():
        if cols is not None and rows:
            a = np.array(rows, dtype=float)
            t.blocks[cols[0]] = {c: a[:, k] for k, c in enumerate(cols) if k < a.shape[1]}

    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line:
                continue
            if line.startswith("#"):
                body = line[1:].strip()
                k, _, v = body.partition(":")
                k, v = k.strip(), v.strip()
                if _ in (":",) and k.replace("_", "").isalpha() and k.islower():
                    if v[:1] in ("{", "["):
                        try:
                            v = json.loads(v)
                        except json.JSONDecodeError:
                            pass
                    t.meta.setdefault(k, v)
                continue
            if line[0].isdigit() or line[0] in "-+.":
                rows.append([_num(x) for x in line.split(",")])
            else:
                flush()
                cols, rows = [c.strip() for c in line.split(",")], []
    flush()
    return t


def _f32(s: str) -> np.ndarray:
    return np.frombuffer(base64.b64decode(s), dtype=">f4").astype(np.float64)


@dataclass
class RewCurve:
    f: np.ndarray
    mag: np.ndarray
    phase: np.ndarray | None
    unit: str

    @property
    def H(self) -> np.ndarray:
        ph = np.zeros_like(self.mag) if self.phase is None else np.deg2rad(self.phase)
        return 10 ** (self.mag / 20) * np.exp(1j * ph)


def read_rew_curve(obj_or_path) -> RewCurve:
    """REW's frequency-response and group-delay JSON: base64 big-endian float32 arrays on a
    linear (startFreq, freqStep) or a listed (`freqs`) frequency axis."""
    j = _load(obj_or_path)
    mag = _f32(j["magnitude"])
    ph = _f32(j["phase"]) if j.get("phase") else None
    if "freqs" in j and j["freqs"]:
        f = _f32(j["freqs"]) if isinstance(j["freqs"], str) else np.asarray(j["freqs"], float)
    elif "freqStep" in j and j.get("freqStep"):
        f = j["startFreq"] + j["freqStep"] * np.arange(len(mag))
    else:  # log spacing
        f = j["startFreq"] * 2 ** (np.arange(len(mag)) / j["ppo"])
    return RewCurve(f=f, mag=mag, phase=ph, unit=j.get("unit", ""))


@dataclass
class RewIR:
    t: np.ndarray
    h: np.ndarray
    fs: float
    delay: float | None
    meta: dict


def read_rew_ir(obj_or_path) -> RewIR:
    j = _load(obj_or_path)
    if j.get("unit", "").lower() == "dbfs":
        raise ValueError("REW IR exported in dBFS is a log magnitude: export with the default unit")
    h = _f32(j["data"])
    dt = j["sampleInterval"]
    t = j["startTime"] + dt * np.arange(len(h))
    meta = {k: v for k, v in j.items() if k != "data"}
    return RewIR(t=t, h=h, fs=j.get("sampleRate", 1 / dt), delay=j.get("delay"), meta=meta)


@dataclass
class RewDistortion:
    f: np.ndarray
    cols: dict[str, np.ndarray]
    meta: dict


def read_rew_distortion(obj_or_path) -> RewDistortion:
    """`/measurements/{id}/distortion?unit=dBr`: columnHeaders + rows."""
    j = _load(obj_or_path)
    hdr = j["columnHeaders"]
    rows = j["data"]
    a = np.full((len(rows), len(hdr)), np.nan)
    for i, r in enumerate(rows):
        v = [np.nan if x is None else float(x) for x in r[: len(hdr)]]
        a[i, : len(v)] = v
    cols = {}
    for k, h in enumerate(hdr):
        key = h.split(" (")[0].strip()
        cols[key] = a[:, k]
    return RewDistortion(f=a[:, 0], cols=cols, meta={k: v for k, v in j.items() if k != "data"})


def _load(obj_or_path):
    if isinstance(obj_or_path, dict):
        return obj_or_path
    with open(obj_or_path, encoding="utf-8") as fh:
        return json.load(fh)
