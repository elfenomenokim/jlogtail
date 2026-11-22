"""0.3.0 ``end=`` tests: the read treats the file as if it ended at ``end``.

Real temp files only; run with the built extension (``maturin develop`` venv):
``python -m unittest discover -s python/tests -v``.
"""
import os
import shutil
import subprocess
import tempfile
import unittest

import jlogtail

ROWS = [b'{"n":1}', b'{"n":2}', b'{"n":3}']


class EndOptionTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="jlogtail-end-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)
        self.path = os.path.join(self.dir, "t.jsonl")
        with open(self.path, "wb") as fh:
            for row in ROWS:
                fh.write(row + b"\n")
        self.size = os.path.getsize(self.path)
        self.row_len = len(ROWS[0]) + 1  # row bytes plus newline

    # 1. end inside a row drops that row; the previous complete row is returned.
    def test_end_mid_row_drops_that_row(self):
        meta, rows = jlogtail.tail_window(self.path, lines=10, end=self.size - 1)
        self.assertEqual(rows, ['{"n":1}', '{"n":2}'])
        self.assertEqual(meta["end_offset"], self.size - self.row_len)
        self.assertEqual(meta["start_offset"], 0)
        self.assertFalse(meta["trailing_oversized"])

    # 2. end exactly after a newline: the row ending there is the last row.
    def test_end_after_newline_includes_that_row(self):
        r1_end = self.row_len
        meta, rows = jlogtail.tail_window(self.path, lines=10, end=r1_end)
        self.assertEqual(rows, ['{"n":1}'])
        self.assertEqual(meta["end_offset"], r1_end)
        self.assertEqual(meta["start_offset"], 0)
        meta, rows = jlogtail.tail_window(self.path, lines=10, end=self.size)
        self.assertEqual(rows, ['{"n":1}', '{"n":2}', '{"n":3}'])
        self.assertEqual(meta["end_offset"], self.size)

    # 3a. start_offset in lines and byte_window modes.
    def test_start_offset_in_lines_and_byte_window_modes(self):
        meta, _ = jlogtail.tail_window(self.path, lines=2, end=self.size)
        self.assertEqual(meta["start_offset"], self.row_len)
        self.assertEqual(meta["end_offset"], self.size)
        meta, rows = jlogtail.tail_window(self.path, byte_window=2 * self.row_len, end=self.size)
        self.assertEqual(rows, ['{"n":2}', '{"n":3}'])
        self.assertEqual(meta["start_offset"], self.row_len)
        self.assertEqual(meta["end_offset"], self.size)

    # 3b. reverse-scan termination on a three-row file.
    def test_reverse_scan_terminates_on_three_row_file(self):
        cursor = self.size
        seen = []
        while cursor:
            meta, rows = jlogtail.tail_window(self.path, lines=1, end=cursor)
            seen.extend(reversed(rows))
            nxt = meta["start_offset"]
            self.assertLess(nxt, cursor)
            cursor = nxt
        self.assertEqual(list(reversed(seen)), ['{"n":1}', '{"n":2}', '{"n":3}'])

    # 3c. reverse-scan termination when the single row exceeds max_bytes.
    def test_reverse_scan_terminates_on_oversized_row(self):
        with open(self.path, "wb") as fh:
            fh.write(b'{"big":"' + b"x" * 200 + b'"}\n')
        size = os.path.getsize(self.path)
        meta, rows = jlogtail.tail_window(self.path, lines=10, end=size, max_bytes=64)
        self.assertEqual(rows, [])
        self.assertEqual(meta["skipped_oversized"], 1)
        self.assertTrue(meta["ceiling_reached"])
        # the parked cursor stops the scan: start_offset does not move
        self.assertEqual(meta["start_offset"], meta["end_offset"])

    # 4. end > size raises ValueError; end=0 returns no rows.
    def test_end_beyond_size_raises_and_end_zero_is_empty(self):
        with self.assertRaises(ValueError):
            jlogtail.tail_window(self.path, lines=10, end=self.size + 1)
        with self.assertRaises(ValueError):
            jlogtail.tail(self.path, lines=10, end=self.size + 1)
        meta, rows = jlogtail.tail_window(self.path, lines=10, end=0)
        self.assertEqual(rows, [])
        self.assertEqual(meta["start_offset"], 0)
        self.assertEqual(meta["end_offset"], 0)

    # 5. default (end=None) keeps the whole-file 0.2.2 behavior.
    def test_default_end_keeps_whole_file_behavior(self):
        meta, rows = jlogtail.tail_window(self.path, lines=10)
        self.assertEqual(rows, ['{"n":1}', '{"n":2}', '{"n":3}'])
        self.assertEqual(meta["start_offset"], 0)
        self.assertEqual(meta["end_offset"], self.size)
        self.assertEqual(jlogtail.tail(self.path, lines=2), ['{"n":2}', '{"n":3}'])

    # CLI: --end bounds the read; end beyond the file exits 2.
    def test_cli_end_flag_and_exit_code(self):
        exe = shutil.which("jlogtail")
        if exe is None:
            self.skipTest("jlogtail CLI not on PATH")
        ok = subprocess.run(
            [exe, self.path, "--lines", "10", "--end", str(self.size - 1)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertEqual(ok.stdout, '{"n":1}\n{"n":2}\n')
        bad = subprocess.run(
            [exe, self.path, "--end", str(self.size + 1)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(bad.returncode, 2)


if __name__ == "__main__":
    unittest.main()
