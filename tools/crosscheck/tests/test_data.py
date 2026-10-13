"""`crosscheck data`: pack, fetch over a local file:// URL, verify, and the unpack guards."""
import gzip
import hashlib
import io
import json
import tarfile

import pytest

from crosscheck import data as D
from crosscheck.__main__ import main

RUN = "20261011T191033Z"


def make_run(root, name=RUN):
    r = root / name
    (r / "genelec" / "sine").mkdir(parents=True)
    (r / "report").mkdir()
    (r / "cal").mkdir()
    (r / "genelec" / "sine" / "noise.wav").write_bytes(bytes(range(256)) * 64)
    (r / "genelec" / "sine" / "results.json").write_text('{"tones": []}')
    (r / "cal" / "ac2_cal.json").write_text("{}")
    (r / "manifest.json").write_text(json.dumps({"rig": "pupu", "started": "2026-10-11T19:10:34Z",
                                                 "flags": {"stages": ["genelec"]}}))
    (r / "report" / "results.json").write_text('{"summary": {}}')
    (r / "report" / "report.md").write_text("# report")
    (r / "report" / "plot.png").write_bytes(b"\x89PNG")
    (r / "ac2.log").write_text("log")
    (r / "genelec" / "rew").mkdir()
    for n in ("rec.wav", "rec_meas.wav", "rec_ref.wav"):
        (r / "genelec" / "rew" / n).write_bytes(b"RIFF")
    return r


def make_suite(root):
    s = root / "suite"
    (s / "baselines" / "pupu").mkdir(parents=True)
    (s / "reports").mkdir()
    (s / "baselines" / "pupu" / "genelec-30dbfs.json").write_text(json.dumps({"provenance": {"run": RUN}}))
    (s / "reports" / "repeats.md").write_text(f"takes {RUN} and 20261010T142303Z")
    (s / "comparison.md").write_text("osm-6f0e5b57 and nothing else")
    (s / "osm.toml").write_text('[[recordings]]\nname = "x"\nwav = "20261008T213854Z/xone/ac2_tf/rec.wav"\n')
    return s


@pytest.fixture
def packed(tmp_path):
    run = make_run(tmp_path / "runs")
    out = tmp_path / "pub"
    man = D.pack([run], out, suite=make_suite(tmp_path), url_template=(out.as_uri() + "/{archive}"), log=lambda *_: None)
    return tmp_path, out, man


def test_pack_selects_the_analysis_set_and_maps_the_citations(packed):
    _, out, man = packed
    e = man["runs"][RUN]
    assert sorted(e["files"]) == ["cal/ac2_cal.json", "genelec/rew/rec.wav", "genelec/sine/noise.wav",
                                  "genelec/sine/results.json",
                                  "manifest.json", "report/results.json"]
    assert e["rig"] == "pupu" and e["stages"] == ["genelec"] and e["entries"] == 6
    assert e["sha256"] == hashlib.sha256((out / f"{RUN}.tar.gz").read_bytes()).hexdigest()
    assert man["cites"]["baselines/pupu/genelec-30dbfs.json"] == [RUN]
    assert man["cites"]["reports/repeats.md"] == ["20261010T142303Z", RUN]
    assert man["cites"]["comparison.md"] == ["osm-6f0e5b57"]
    assert man["cites"]["osm.toml"] == ["20261008T213854Z"]
    assert man["cited_not_packed"] == ["20261008T213854Z", "20261010T142303Z", "osm-6f0e5b57"]
    assert json.loads((out / "manifest.json").read_text()) == man
    assert e["excluded"] == []


def test_pack_exclude_leaves_a_file_out_and_says_so(tmp_path):
    run = make_run(tmp_path / "runs")
    man = D.pack([run], tmp_path / "pub", suite=make_suite(tmp_path), exclude=("*/sine/noise.wav",),
                 log=lambda *_: None)
    e = man["runs"][RUN]
    assert "genelec/sine/noise.wav" not in e["files"] and e["excluded"] == ["genelec/sine/noise.wav"]


def test_pack_is_byte_reproducible(tmp_path, packed):
    root, out, man = packed
    again = D.pack([root / "runs" / RUN], tmp_path / "again", suite=root / "suite", log=lambda *_: None)
    assert again["runs"][RUN]["sha256"] == man["runs"][RUN]["sha256"]


def test_fetch_then_verify_over_a_file_url(packed):
    root, out, _ = packed
    dest = root / "data"
    assert main(["data", "fetch", "--manifest", str(out / "manifest.json"), "--dir", str(dest)]) == 0
    assert (dest / RUN / "genelec" / "sine" / "noise.wav").read_bytes() == bytes(range(256)) * 64
    assert not (dest / RUN / "report" / "report.md").exists()
    assert not list(dest.glob(".*"))  # no partial download or staging left
    assert main(["data", "verify", "--manifest", (out / "manifest.json").as_uri(), "--dir", str(dest)]) == 0
    assert main(["data", "verify", "--manifest", str(out / "manifest.json"), "--archives", str(out)]) == 0
    (dest / RUN / "cal" / "ac2_cal.json").write_text('{"x": 1}')
    assert main(["data", "verify", "--manifest", str(out / "manifest.json"), "--dir", str(dest)]) == 1
    # a second fetch replaces the tampered tree
    assert main(["data", "fetch", "--manifest", str(out / "manifest.json"), "--dir", str(dest), RUN]) == 0
    assert D.check_tree(dest / RUN, json.loads((out / "manifest.json").read_text())["runs"][RUN]) == []


def test_fetch_refuses_a_hash_mismatch_and_leaves_nothing(packed):
    root, out, _ = packed
    arc = out / f"{RUN}.tar.gz"
    b = bytearray(arc.read_bytes())
    b[-20] ^= 1
    arc.write_bytes(bytes(b))
    dest = root / "data"
    assert main(["data", "fetch", "--manifest", str(out / "manifest.json"), "--dir", str(dest)]) == 1
    assert not (dest / RUN).exists() and not list(dest.glob(".*"))
    assert main(["data", "verify", "--manifest", str(out / "manifest.json"), "--archives", str(out)]) == 1


def test_fetch_refuses_the_placeholder_url_and_unknown_runs(packed, tmp_path):
    root, out, man = packed
    man["url_template"] = D.PLACEHOLDER_URL
    (out / "manifest.json").write_text(json.dumps(man))
    with pytest.raises(D.Refused, match="placeholder"):
        D.fetch(str(out / "manifest.json"), root / "data", log=lambda *_: None)
    D.fetch(str(out / "manifest.json"), root / "data", url_template=out.as_uri() + "/{archive}", log=lambda *_: None)
    assert (root / "data" / RUN / "manifest.json").exists()
    with pytest.raises(D.Refused, match="not in the manifest"):
        D.fetch(str(out / "manifest.json"), root / "data", ["20200101T000000Z"], url_template="file:///x/{archive}")


def _tar(entries):
    """A tar.gz of (TarInfo, bytes | None) pairs."""
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", mtime=0) as gz, tarfile.open(fileobj=gz, mode="w") as t:
        for ti, body in entries:
            if body is not None:
                ti.size = len(body)
            t.addfile(ti, io.BytesIO(body) if body is not None else None)
    return buf.getvalue()


def _entry(files):
    return {"entries": len(files), "unpacked_bytes": sum(len(b) for b in files.values()),
            "files": {k: {"size": len(b), "sha256": hashlib.sha256(b).hexdigest()} for k, b in files.items()}}


@pytest.mark.parametrize("name", [f"{RUN}/../evil", "/etc/evil", "evil", f"other/{RUN}/a", f"{RUN}/./a"])
def test_unpack_refuses_paths_outside_the_run(tmp_path, name):
    arc = tmp_path / "a.tar.gz"
    arc.write_bytes(_tar([(tarfile.TarInfo(name), b"x")]))
    with pytest.raises(D.Refused):
        D.unpack(arc, RUN, _entry({"a": b"x", "evil": b"x"}), tmp_path / "d")
    assert not (tmp_path / "evil").exists() and not (tmp_path / "d" / RUN).exists()


def test_unpack_refuses_links_extra_entries_and_oversize(tmp_path):
    arc = tmp_path / "a.tar.gz"
    link = tarfile.TarInfo(f"{RUN}/a")
    link.type, link.linkname = tarfile.SYMTYPE, "/etc/passwd"
    arc.write_bytes(_tar([(link, None)]))
    with pytest.raises(D.Refused, match="regular file"):
        D.unpack(arc, RUN, _entry({"a": b"x"}), tmp_path / "d")
    arc.write_bytes(_tar([(tarfile.TarInfo(f"{RUN}/a"), b"x"), (tarfile.TarInfo(f"{RUN}/b"), b"y")]))
    with pytest.raises(D.Refused, match="not in the manifest"):
        D.unpack(arc, RUN, _entry({"a": b"x"}), tmp_path / "d")
    arc.write_bytes(_tar([(tarfile.TarInfo(f"{RUN}/a"), b"xx")]))
    with pytest.raises(D.Refused, match="size"):
        D.unpack(arc, RUN, _entry({"a": b"x"}), tmp_path / "d")
    e = _entry({"a": b"xx"})
    e["unpacked_bytes"] = 1
    with pytest.raises(D.Refused, match="budget"):
        D.unpack(arc, RUN, e, tmp_path / "d")
    e = _entry({"a": b"xy"})
    with pytest.raises(D.Refused, match="SHA-256"):
        D.unpack(arc, RUN, e, tmp_path / "d")
    assert not (tmp_path / "d" / RUN).exists() and not list((tmp_path / "d").iterdir())
    D.unpack(arc, RUN, _entry({"a": b"xx"}), tmp_path / "d")
    assert (tmp_path / "d" / RUN / "a").read_bytes() == b"xx"


def test_manifest_with_a_bad_run_name_is_refused(packed):
    _, out, man = packed
    man["runs"]["../x"] = dict(man["runs"][RUN], archive="../x.tar.gz")
    (out / "manifest.json").write_text(json.dumps(man))
    with pytest.raises(D.Refused, match="bad name"):
        D.load_manifest(str(out / "manifest.json"))
