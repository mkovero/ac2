"""Play and record through JACK in the same cycles (numpy + JACK-Client only, so it runs on
the rig's venv). Every signal is checked against its ceiling before the client starts,
faded in and out, and clipped to the ceiling in the callback as a backstop. Ctrl-C (or any
exception while waiting) fades the outputs out over the fade time before the client stops.

The suite's emitting stages call `play_record` only after `levels.Policy.check` has passed
for the channels; ports come from the rig config, never from the command line."""
from __future__ import annotations

import threading
from dataclasses import dataclass

import numpy as np

from .levels import PolicyError


@dataclass
class Play:
    port: str
    signal: np.ndarray  # float, already at its level
    ceiling: float  # largest |sample| allowed (peak amplitude of the permitted level)


def fade(x: np.ndarray, n: int) -> np.ndarray:
    """Raised-cosine fade in and out over n samples: a step at the start or end of a tone
    spreads its energy across the band (a click on a speaker, a false harmonic floor)."""
    x = np.array(x, dtype=np.float64, copy=True)
    n = min(n, len(x) // 2)
    if n > 0:
        r = 0.5 - 0.5 * np.cos(np.pi * np.arange(n) / n)
        x[:n] *= r
        x[-n:] *= r[::-1]
    return x


def sine(f: float, amplitude: float, seconds: float, fs: float, fade_s: float) -> np.ndarray:
    n = int(round(seconds * fs))
    return fade(amplitude * np.cos(2 * np.pi * f * np.arange(n) / fs), int(fade_s * fs))


def check_plays(plays: list[Play]) -> None:
    for p in plays:
        if not np.all(np.isfinite(p.signal)):
            raise PolicyError(f"{p.port}: signal is not finite")
        pk = float(np.max(np.abs(p.signal))) if len(p.signal) else 0.0
        if pk > p.ceiling * (1 + 1e-9):
            raise PolicyError(f"{p.port}: signal peaks at {20*np.log10(pk):.2f} dBFS, above its ceiling "
                              f"{20*np.log10(p.ceiling):.2f} dBFS")


def play_record(plays: list[Play], inputs: list[str], *, pre_s: float = 0.5, post_s: float = 1.0,
                fade_s: float = 0.05, client_name: str = "crosscheck", expect_fs: float | None = None,
                on_start=None, max_xruns: int = 0) -> tuple[float, np.ndarray]:
    """Plays every `Play` from pre_s on and records `inputs` from the first sample; returns
    (fs, frames × inputs). With no plays it records pre_s + post_s seconds."""
    import jack

    check_plays(plays)
    client = jack.Client(client_name, no_start_server=True)
    try:
        fs = float(client.samplerate)
        if expect_fs and abs(fs - expect_fs) > 0.5:
            raise PolicyError(f"JACK runs at {fs:g} Hz, the rig config says {expect_fs:g} Hz")
        pre, post, nf = int(pre_s * fs), int(post_s * fs), max(int(fade_s * fs), 1)
        length = max((len(p.signal) for p in plays), default=0)
        total = pre + length + post
        bufs = []
        for p in plays:
            b = np.zeros(total, np.float32)
            b[pre:pre + len(p.signal)] = p.signal
            bufs.append(b)
        ceil = np.array([p.ceiling for p in plays], np.float32)
        rec = np.zeros((total, len(inputs)), np.float32)
        outs = [client.outports.register(f"out_{k}") for k in range(len(plays))]
        ins = [client.inports.register(f"in_{k}") for k in range(len(inputs))]
        fade_out = (0.5 + 0.5 * np.cos(np.pi * np.arange(nf) / nf)).astype(np.float32)
        st = {"pos": 0, "abort_at": None, "xruns": 0}
        done = threading.Event()

        @client.set_process_callback
        def process(frames):
            p = st["pos"]
            n = max(0, min(frames, total - p))
            g = None
            if st["abort_at"] is not None:
                k = np.arange(p, p + frames) - st["abort_at"]
                g = np.where(k < nf, fade_out[np.clip(k, 0, nf - 1)], 0.0).astype(np.float32)
            for j, o in enumerate(outs):
                a = o.get_array()
                a[:] = 0
                if n:
                    s = bufs[j][p:p + n]
                    if g is not None:
                        s = s * g[:n]
                    a[:n] = np.clip(s, -ceil[j], ceil[j])
            for j, ip in enumerate(ins):
                rec[p:p + n, j] = ip.get_array()[:n]
            st["pos"] = p + n
            if st["pos"] >= total or (st["abort_at"] is not None and p + frames - st["abort_at"] >= nf):
                done.set()

        @client.set_xrun_callback
        def xrun(delay):
            st["xruns"] += 1

        client.activate()
        try:
            for j, o in enumerate(outs):
                client.connect(o, plays[j].port)
            for j, ip in enumerate(ins):
                client.connect(inputs[j], ip)
            if on_start:
                on_start()
            while not done.wait(0.2):
                pass
        except BaseException:
            # silence first, then let the caller see what happened
            st["abort_at"] = st["pos"]
            done.clear()
            done.wait(fade_s + 1.0)
            raise
        finally:
            client.deactivate()
        if st["xruns"] > max_xruns:
            raise RuntimeError(f"{st['xruns']} JACK xrun(s) during the take: discarded")
        return fs, rec[:st["pos"]].astype(np.float64)
    finally:
        client.close()
