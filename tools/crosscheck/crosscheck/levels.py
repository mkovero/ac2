"""Levels and the emission policy.

One convention everywhere, the one ac2 and REW share: a level in dBFS is the level of a
full-scale sine, so a sine or a sweep at L dBFS peaks at 10^(L/20) and has an RMS of
10^(L/20)/sqrt(2) (`ac2_core::generator::dbfs_to_rms`). The hand scripts of 2026-10-07
(`jsine.py`, `jphase.py`, the sweep replica) used amplitude 10^(L/20)*sqrt(2) instead: their
"-30 dBFS" was -27 dBFS here. Everything this package plays is scaled by `peak_amplitude`.

The policy is pure so the tests can prove every refusal without a sound card.
"""
from __future__ import annotations

import math
import re
from dataclasses import dataclass

# Backstops that no config can raise: the speaker never above -50 dBFS, nothing above -30.
SPEAKER_HARD_MAX_DBFS = -50.0
ELECTRICAL_HARD_MAX_DBFS = -30.0
# REW's stimulus file: refuse a file whose peak (after scaling) is above this. A -50 dBFS
# sweep in the RMS convention of the hand scripts peaks at -47; in ours at -50.
STIMULUS_PEAK_HARD_MAX_DBFS = -46.0

_LEVEL = re.compile(r"^\s*([+-]?\d+(?:\.\d+)?)\s*dbfs\s*$", re.IGNORECASE)


class PolicyError(Exception):
    """An emission the policy refuses. The message says why and what would allow it."""


def parse_dbfs(text: str) -> float:
    """Parses a typed level such as `-50dbfs`. A bare number is refused: the unit is the
    operator's statement that this is a digital level, not a voltage or an SPL."""
    m = _LEVEL.match(text)
    if not m:
        raise PolicyError(f"level {text!r}: write it with its unit, e.g. -50dbfs")
    v = float(m.group(1))
    if not math.isfinite(v) or v > 0:
        raise PolicyError(f"level {text!r} is not a level below full scale")
    return v


def peak_amplitude(level_dbfs: float) -> float:
    """Peak of a sine (or sweep) at `level_dbfs` in the full-scale-sine convention."""
    return 10.0 ** (level_dbfs / 20.0)


def rms_of_sine(level_dbfs: float) -> float:
    return peak_amplitude(level_dbfs) / math.sqrt(2.0)


def dbfs_of_rms(rms: float) -> float:
    """dBFS of a sine-like signal of this RMS (full-scale sine = 0 dBFS)."""
    return 20.0 * math.log10(max(rms, 1e-30) * math.sqrt(2.0))


@dataclass(frozen=True)
class Output:
    channel: int
    port: str
    role: str  # "electrical" (no speaker anywhere downstream), "loopback", "speaker"
    note: str = ""

    @property
    def electrical_only(self) -> bool:
        return self.role in ("electrical", "loopback")


@dataclass(frozen=True)
class Policy:
    """What the rig config says plus what the operator typed on the command line."""

    outputs: dict[int, Output]
    forbidden: frozenset[int]  # never, whatever the flags (for the Xone stages: out 1)
    system_max_dbfs: float  # ac2d's --max-level bound on this rig
    electrical_max_dbfs: float  # the most an electrical drop-in may allow (config)
    speaker_max_dbfs: float  # config; clamped to SPEAKER_HARD_MAX_DBFS
    emit_dbfs: float | None  # --emit
    emit_speaker_dbfs: float | None  # --emit-speaker
    allow_electrical_dbfs: float | None  # --allow-electrical-level

    def check(self, channels: list[int], level_dbfs: float, *, speaker_stage: bool) -> float:
        """Returns the level to emit on `channels` or raises PolicyError.

        Electrical stages: every channel configured and electrical-only, none forbidden;
        the level at most --emit, and above the system max only up to
        --allow-electrical-level (itself capped by config and the -30 backstop).
        Speaker stage: --emit-speaker given, at most -50 dBFS whatever any config says;
        the electrical allowance never reaches it."""
        if not channels:
            raise PolicyError("no output channel")
        for c in channels:
            if c not in self.outputs:
                raise PolicyError(f"out {c} is not described in the rig config: refused")
        speakers = [c for c in channels if self.outputs[c].role == "speaker"]
        if speaker_stage:
            if self.emit_speaker_dbfs is None:
                raise PolicyError("the speaker stage needs --emit-speaker <level>dbfs")
            cap = min(self.speaker_max_dbfs, SPEAKER_HARD_MAX_DBFS)
            if self.emit_speaker_dbfs > cap:
                raise PolicyError(f"--emit-speaker {self.emit_speaker_dbfs:g} dBFS is above the speaker ceiling {cap:g} dBFS")
            if level_dbfs > self.emit_speaker_dbfs:
                raise PolicyError(f"{level_dbfs:g} dBFS is above --emit-speaker {self.emit_speaker_dbfs:g}")
            if len(speakers) != 1:
                raise PolicyError("the speaker stage drives exactly one speaker output")
            others = [c for c in channels if c not in speakers]
            for c in others:
                if not self.outputs[c].electrical_only:
                    raise PolicyError(f"out {c} beside the speaker is not electrical-only")
            return level_dbfs
        for c in channels:
            if c in self.forbidden:
                raise PolicyError(f"out {c} ({self.outputs[c].note or self.outputs[c].role}) is refused for this stage")
            if speakers:
                raise PolicyError(f"out {c} drives a speaker: only the speaker stage may use it")
            if not self.outputs[c].electrical_only:
                raise PolicyError(f"out {c} is not marked electrical-only")
        if self.emit_dbfs is None:
            raise PolicyError("emitting stages need --emit <level>dbfs")
        if level_dbfs > self.emit_dbfs:
            raise PolicyError(f"{level_dbfs:g} dBFS is above --emit {self.emit_dbfs:g} dBFS")
        if level_dbfs > self.system_max_dbfs:
            allow = self.allow_electrical_dbfs
            cap = min(self.electrical_max_dbfs, ELECTRICAL_HARD_MAX_DBFS)
            if allow is None:
                raise PolicyError(
                    f"{level_dbfs:g} dBFS is above the rig's {self.system_max_dbfs:g} dBFS; "
                    f"electrical-only outputs may go up to {cap:g} with --allow-electrical-level")
            if allow > cap:
                raise PolicyError(f"--allow-electrical-level {allow:g} is above the electrical cap {cap:g} dBFS")
            if level_dbfs > allow:
                raise PolicyError(f"{level_dbfs:g} dBFS is above --allow-electrical-level {allow:g}")
        return level_dbfs

    def electrical_level(self) -> float:
        """The level the electrical stages run at: --emit."""
        if self.emit_dbfs is None:
            raise PolicyError("emitting stages need --emit <level>dbfs")
        return self.emit_dbfs

    def needs_raised_daemon(self) -> bool:
        return self.emit_dbfs is not None and self.emit_dbfs > self.system_max_dbfs


def check_stimulus_peak(peak_dbfs: float, level_dbfs: float, *, speaker: bool) -> None:
    """A file to be played must peak no higher than the level it is played at (sine
    convention); on the speaker also never above the stimulus backstop."""
    if speaker and peak_dbfs > STIMULUS_PEAK_HARD_MAX_DBFS + 1e-6:
        raise PolicyError(f"stimulus peaks at {peak_dbfs:.2f} dBFS, above {STIMULUS_PEAK_HARD_MAX_DBFS} dBFS")
    if peak_dbfs > level_dbfs + 0.01:
        raise PolicyError(f"stimulus peaks at {peak_dbfs:.2f} dBFS, above the allowed {level_dbfs:g} dBFS")
