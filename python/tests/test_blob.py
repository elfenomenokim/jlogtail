"""0.3.2 ``blob=True`` tests: one bytes object holding the joined complete rows.

Real temp files only; run with the built extension (``maturin develop`` venv):
``python -m unittest discover -s python/tests -v``.
"""
import os
import shutil
import tempfile
import unittest

import jlogtail

ROWS = [b'{"n":1}', b'{"n":2,"t":"\xed\x95\x9c\xea\xb8\x80"}', b'{"n":3}']  # includes multibyte UTF-8
BAD = b'{"n":2,"t":"m\xffid"}'


def joined(raw_rows):
    return b"".join(row + b"\n" for row in raw_rows)


class BlobOptionTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="jlogtail-blob-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)
        self.path = os.path.join(self.dir, "t.jsonl")
        with open(self.path, "wb") as fh:
            for row in ROWS:
                fh.write(row + b"\n")
        self.size = os.path.getsize(self.path)
        self.row_len = len(ROWS[0]) + 1  # row bytes plus newline

    # 1. blob == b"".join(raw row + \n) and meta identical across lines,
    #    byte_window, end, partial-first, oversized and keep_partial_first cases.
    def test_tail_window_blob_equals_joined_raw_and_meta_identical(self):
        big = b'{"big":"' + b"x" * 200 + b'"}\n'
        with open(self.path, "ab") as fh:
            fh.write(big)
        size = os.path.getsize(self.path)
        cases = [
            dict(lines=2),
            dict(lines=10, byte_window=2 * self.row_len),
            dict(lines=10, end=size),
            dict(lines=10, end=size - 1),          # end mid-row drops that row
            dict(lines=10, end=self.row_len),      # end at the first boundary
            dict(lines=10, end=0),                 # empty window
            dict(lines=10, byte_window=20, max_bytes=20),   # oversized skip
            dict(lines=10, byte_window=2 * self.row_len, keep_partial_first=True),
        ]
        for kw in cases:
            with self.subTest(**kw):
                meta_r, rows_r = jlogtail.tail_window(self.path, raw=True, **kw)
                meta_b, data = jlogtail.tail_window(self.path, blob=True, **kw)
                self.assertEqual(meta_b, meta_r)
                self.assertIsInstance(data, bytes)
                self.assertEqual(data, joined(rows_r))

    # 2. read_forward: blob == joined raw rows, meta identical.
    def test_read_forward_blob_equals_joined_raw_and_meta_identical(self):
        cases = [
            dict(offset=0),
            dict(offset=self.row_len),   # on a boundary
            dict(offset=3),              # mid-row: partial first row dropped
            dict(offset=self.size),      # idle poll at EOF
            dict(offset=0, max_bytes=2 * self.row_len),  # ceiling cut
        ]
        for kw in cases:
            with self.subTest(**kw):
                meta_r, rows_r = jlogtail.read_forward(self.path, raw=True, **kw)
                meta_b, data = jlogtail.read_forward(self.path, blob=True, **kw)
                self.assertEqual(meta_b, meta_r)
                self.assertIsInstance(data, bytes)
                self.assertEqual(data, joined(rows_r))

    # 3. an unterminated trailing row is held on the blob path too.
    def test_blob_holds_an_unterminated_trailing_row(self):
        with open(self.path, "ab") as fh:
            fh.write(b'{"n":4')
        meta_b, data = jlogtail.read_forward(self.path, offset=0, blob=True)
        meta_r, rows_r = jlogtail.read_forward(self.path, offset=0, raw=True)
        self.assertEqual(meta_b, meta_r)
        self.assertEqual(data, joined(rows_r))
        self.assertEqual(data, joined(ROWS))
        self.assertEqual(meta_b["new_offset"], self.size)

    # 4. invalid UTF-8 passes through the blob untouched and errors is ignored.
    def test_blob_preserves_invalid_utf8_and_ignores_errors(self):
        with open(self.path, "wb") as fh:
            fh.write(ROWS[0] + b"\n" + BAD + b"\n" + ROWS[2] + b"\n")
        meta, data = jlogtail.tail_window(self.path, lines=10, blob=True, errors="bogus")
        self.assertEqual(data, ROWS[0] + b"\n" + BAD + b"\n" + ROWS[2] + b"\n")
        meta_s, rows_s = jlogtail.tail_window(self.path, lines=10, raw=True)
        self.assertEqual(data, joined(rows_s))
        self.assertEqual(meta, meta_s)

    # 5. an empty selection is an empty bytes object, not an empty list.
    def test_empty_selection_is_empty_bytes(self):
        meta, data = jlogtail.tail_window(self.path, lines=10, end=0, blob=True)
        self.assertEqual(data, b"")
        self.assertEqual(meta["end_offset"], 0)
        meta, data = jlogtail.read_forward(self.path, offset=self.size, blob=True)
        self.assertEqual(data, b"")
        self.assertEqual(meta["new_offset"], self.size)

    # 7. a first row longer than the 64 KiB head read falls back to the
    #    buffered path and still answers identically to raw rows.
    def test_first_row_longer_than_the_head_falls_back(self):
        long_row = b'{"pad":"' + b"x" * 100_000 + b'"}'
        with open(self.path, "wb") as fh:
            fh.write(long_row + b"\n")
            for row in ROWS:
                fh.write(row + b"\n")
        size = os.path.getsize(self.path)
        for kw in (dict(lines=10, byte_window=size),
                   dict(lines=10, byte_window=size - len(long_row))):
            with self.subTest(**kw):
                meta_r, rows_r = jlogtail.tail_window(self.path, raw=True, **kw)
                meta_b, data = jlogtail.tail_window(self.path, blob=True, **kw)
                self.assertEqual(meta_b, meta_r)
                self.assertEqual(data, joined(rows_r))
        meta_r, rows_r = jlogtail.read_forward(self.path, offset=3, raw=True)
        meta_b, data = jlogtail.read_forward(self.path, offset=3, blob=True)
        self.assertEqual(meta_b, meta_r)
        self.assertEqual(data, joined(rows_r))

    # 8. default blob=False is unchanged (list of rows).
    def test_default_blob_false_unchanged(self):
        meta, rows = jlogtail.tail_window(self.path, lines=2)
        self.assertEqual(rows, [r.decode("utf-8") for r in ROWS[-2:]])
        self.assertIsInstance(rows, list)


if __name__ == "__main__":
    unittest.main()
