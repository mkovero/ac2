"""Checks of convert.py's backups: `python3 -I tools/proto-next/test_convert.py`."""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import convert  # noqa: E402


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
    unittest.main()
