"""32-bit float WAV (RIFF and RF64, which ac2's raw capture files become past 4 GiB)."""
from __future__ import annotations

import struct

import numpy as np


def read(path) -> tuple[int, np.ndarray]:
    """Returns (sample rate, frames x channels float64)."""
    with open(path, "rb") as fh:
        b = fh.read()
    if b[:4] not in (b"RIFF", b"RF64") or b[8:12] != b"WAVE":
        raise ValueError(f"{path}: not a WAV file")
    i, fmt, data, ds64 = 12, None, None, None
    while i + 8 <= len(b):
        cid, n = b[i:i + 4], struct.unpack("<I", b[i + 4:i + 8])[0]
        if cid == b"ds64":
            ds64 = struct.unpack("<QQQ", b[i + 8:i + 32])
        if cid == b"data" and n == 0xFFFFFFFF and ds64 is not None:
            n = ds64[1]
        if cid == b"fmt ":
            fmt = struct.unpack("<HHIIHH", b[i + 8:i + 24])
        elif cid == b"data":
            data = b[i + 8:i + 8 + n]
        i += 8 + n + (n & 1)
    if fmt is None or data is None:
        raise ValueError(f"{path}: no fmt or data chunk")
    tag, ch, fs, _, _, bits = fmt
    if bits == 32 and tag in (3, 0xFFFE):
        x = np.frombuffer(data, dtype="<f4")
    elif bits == 16 and tag in (1, 0xFFFE):
        x = np.frombuffer(data, dtype="<i2") / 32768.0
    elif bits == 24:
        u = np.frombuffer(data, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        v = u[:, 0] | (u[:, 1] << 8) | (u[:, 2] << 16)
        x = np.where(v >= 1 << 23, v - (1 << 24), v) / float(1 << 23)
    else:
        raise ValueError(f"{path}: format tag {tag}, {bits} bit not supported")
    x = np.asarray(x, dtype=np.float64)
    return fs, x[: len(x) // ch * ch].reshape(-1, ch)


def write(path, fs: int, x: np.ndarray) -> None:
    x = np.ascontiguousarray(np.atleast_2d(x.T).T if x.ndim == 1 else x, dtype="<f4")
    if x.ndim == 1:
        x = x[:, None]
    ch = x.shape[1]
    data = x.tobytes()
    hdr = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE"
    hdr += b"fmt " + struct.pack("<IHHIIHH", 16, 3, ch, fs, fs * ch * 4, ch * 4, 32)
    hdr += b"data" + struct.pack("<I", len(data))
    with open(path, "wb") as fh:
        fh.write(hdr + data)
