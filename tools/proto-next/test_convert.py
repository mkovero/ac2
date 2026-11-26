"""Checks of convert.py: `python3 -I tools/proto-next/test_convert.py`.

`samples/old` holds a file of every format the converter converts, as the 105 daemon wrote
them; `samples/new` what the converter makes of them, which the 107 daemon loads whole
(`crates/ac2d/tests/it/converted.rs`).
"""

import json
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import convert  # noqa: E402

OLD = os.path.join(HERE, "samples", "old")
NEW = os.path.join(HERE, "samples", "new")
FILES = ["rig.json", "calibrations.json", "plans/Hall.json", "authorized_clients"]


def args(d):
    return [
        "--rig", os.path.join(d, "rig.json"),
        "--cal", os.path.join(d, "calibrations.json"),
        "--plans", os.path.join(d, "plans"),
        "--auth", os.path.join(d, "authorized_clients"),
    ]


def read(p):
    with open(p, "rb") as f:
        return f.read()


def same(a, b):
    """Equal as the daemon reads them: JSON by value, the key list by text."""
    if a.endswith(".json"):
        return json.loads(read(a)) == json.loads(read(b))
    return read(a) == read(b)


def listing(d):
    return sorted(
        os.path.relpath(os.path.join(r, n), d) for r, _, ns in os.walk(d) for n in ns
    )


class Samples(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = os.path.join(self.tmp.name, "host")
        shutil.copytree(OLD, self.d)
        os.chmod(os.path.join(self.d, "authorized_clients"), 0o600)

    def tearDown(self):
        self.tmp.cleanup()

    def test_every_format_converts_to_the_new_shape(self):
        self.assertEqual(convert.main(args(self.d)), 0)
        for f in FILES:
            self.assertTrue(same(os.path.join(self.d, f), os.path.join(NEW, f)), f)
            self.assertEqual(read(os.path.join(self.d, f + ".pre106")), read(os.path.join(OLD, f)), f)

    def test_a_second_run_changes_nothing(self):
        self.assertEqual(convert.main(args(self.d)), 0)
        before = {f: read(os.path.join(self.d, f)) for f in listing(self.d)}
        self.assertEqual(convert.main(args(self.d)), 0)
        self.assertEqual(listing(self.d), sorted(before))
        for f, b in before.items():
            self.assertEqual(read(os.path.join(self.d, f)), b, f)

    def test_the_new_shapes_are_left_alone(self):
        for f, conv in [
            ("rig.json", convert.convert_rig),
            ("calibrations.json", convert.convert_cal),
            ("plans/Hall.json", convert.convert_plan),
        ]:
            with open(os.path.join(NEW, f), encoding="utf-8") as fh:
                self.assertIsNone(conv(json.load(fh)), f)
        with open(os.path.join(NEW, "authorized_clients"), encoding="utf-8") as fh:
            self.assertIsNone(convert.convert_auth(fh.read()))

    def test_modes_are_kept_and_the_key_list_is_the_owners_alone(self):
        rig = os.path.join(self.d, "rig.json")
        os.chmod(rig, 0o640)
        os.chmod(os.path.join(self.d, "authorized_clients"), 0o644)
        self.assertEqual(convert.main(args(self.d)), 0)
        mode = lambda f: os.stat(os.path.join(self.d, f)).st_mode & 0o777  # noqa: E731
        self.assertEqual(mode("rig.json"), 0o640)
        self.assertEqual(mode("rig.json.pre106"), 0o640)
        self.assertEqual(mode("authorized_clients"), 0o600)
        self.assertEqual(mode("authorized_clients.pre106"), 0o600)

    @unittest.skipUnless(hasattr(os, "geteuid"), "no file owners")
    def test_a_file_of_another_user_is_refused_untouched(self):
        other = os.geteuid() + 1
        with mock.patch.object(convert.os, "geteuid", return_value=other):
            self.assertEqual(convert.main(args(self.d)), 1)
        self.assertEqual(listing(self.d), listing(OLD))
        for f in FILES:
            self.assertEqual(read(os.path.join(self.d, f)), read(os.path.join(OLD, f)), f)

    def test_another_format_is_refused_untouched(self):
        p = os.path.join(self.d, "calibrations.json")
        with mock.patch("sys.stderr"):
            self.assertEqual(convert.main(["--rig", p]), 1)
        self.assertEqual(read(p), read(os.path.join(OLD, "calibrations.json")))
        self.assertFalse(os.path.exists(p + ".pre106"))


class Rig12(unittest.TestCase):
    def test_a_106_rig_file_gains_the_sacn_section_alone(self):
        with open(os.path.join(NEW, "rig.json"), encoding="utf-8") as fh:
            new = json.load(fh)
        v12 = {k: v for k, v in new.items() if k != "sacn_target"}
        v12["version"] = 12
        self.assertEqual(convert.convert_rig(v12), new)

    def test_another_version_is_refused(self):
        with self.assertRaises(ValueError):
            convert.convert_rig({"format": convert.RIG_FORMAT, "version": 10})


class Backups(unittest.TestCase):
    def test_an_earlier_backup_is_never_written_over(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "authorized_clients")
            with open(p, "w") as f:
                f.write("laptop KEY1\n")
            with open(p + ".pre106", "w") as f:
                f.write("the first run's original\n")
            convert.rewrite_auth(p, None)
            with open(p + ".pre106") as f:
                self.assertEqual(f.read(), "the first run's original\n")
            with open(p + ".pre106.1") as f:
                self.assertEqual(f.read(), "laptop KEY1\n")
            self.assertEqual(os.stat(p + ".pre106.1").st_mode & 0o777, 0o600)
            with open(p) as f:
                self.assertEqual(f.read(), "laptop KEY1 operator\n")


if __name__ == "__main__":
    unittest.main(buffer=True)
