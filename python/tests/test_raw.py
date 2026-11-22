"""0.3.1 ``raw=True`` tests: rows come back as bytes, undecoded.

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


class RawOptionTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="jlogtail-raw-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)
        self.path = os.path.join(self.dir, "t.jsonl")
        with open(self.path, "wb") as fh:
            for row in ROWS:
                fh.write(row + b"\n")
        self.size = os.path.getsize(self.path)
        self.row_len = len(ROWS[0]) + 1  # row bytes plus newline

    # 1. raw rows equal row.encode() of the str rows across lines/byte_window/end cases.
    def test_tail_raw_rows_equal_str_encode_across_cases(self):
        cases = [
            dict(lines=2),
            dict(lines=10, byte_window=2 * self.row_len),
            dict(lines=10, end=self.size),
            dict(lines=10, end=self.size - 1),
            dict(lines=10, end=self.row_len),
        ]
        for kw in cases:
            with self.subTest(**kw):
                str_rows = jlogtail.tail(self.path, errors="strict", **kw)
                raw_rows = jlogtail.tail(self.path, raw=True, **kw)
                self.assertEqual([r.encode("utf-8") for r in str_rows], raw_rows)
                self.assertTrue(all(isinstance(r, bytes) for r in raw_rows))

    # 2. tail_window: meta identical between raw and non-raw, rows are bytes.
    def test_tail_window_raw_meta_identical(self):
        meta_s, rows_s = jlogtail.tail_window(self.path, lines=2, errors="strict")
        meta_r, rows_r = jlogtail.tail_window(self.path, lines=2, raw=True)
        self.assertEqual(meta_s, meta_r)
        self.assertEqual([r.encode("utf-8") for r in rows_s], rows_r)

    # 3. read_forward: meta identical between raw and non-raw, rows are bytes.
    def test_read_forward_raw_meta_identical(self):
        meta_s, rows_s = jlogtail.read_forward(self.path, offset=0)
        meta_r, rows_r = jlogtail.read_forward(self.path, offset=0, raw=True)
        self.assertEqual(meta_s, meta_r)
        self.assertEqual([r.encode("utf-8") for r in rows_s], rows_r)

    # 4. an invalid-UTF-8 row comes back with its original bytes.
    def test_invalid_utf8_row_comes_back_with_original_bytes(self):
        with open(self.path, "wb") as fh:
            fh.write(ROWS[0] + b"\n" + BAD + b"\n" + ROWS[2] + b"\n")
        raw_rows = jlogtail.tail(self.path, lines=10, raw=True)
        self.assertEqual(raw_rows, [ROWS[0], BAD, ROWS[2]])
        # the same row cannot decode strictly, proving the bytes never round-tripped
        with self.assertRaises(ValueError):
            jlogtail.tail(self.path, lines=10, errors="strict")
        meta, rows = jlogtail.read_forward(self.path, offset=0, raw=True)
        self.assertEqual(rows, [ROWS[0], BAD, ROWS[2]])
        self.assertEqual(meta["new_offset"], os.path.getsize(self.path))

    # 5. raw=True ignores errors (even a bogus value); default keeps validating.
    def test_raw_ignores_errors(self):
        rows = jlogtail.tail(self.path, lines=2, raw=True, errors="bogus")
        self.assertEqual(rows, ROWS[-2:])
        with self.assertRaises(ValueError):
            jlogtail.tail(self.path, lines=2, errors="bogus")

    # 6. default raw=False is unchanged (str rows).
    def test_default_raw_false_unchanged(self):
        rows = jlogtail.tail(self.path, lines=2)
        self.assertEqual(rows, [r.decode("utf-8") for r in ROWS[-2:]])
        self.assertTrue(all(isinstance(r, str) for r in rows))


if __name__ == "__main__":
    unittest.main()
