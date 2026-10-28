"""A live owned publication can invalidate a work-directory enumeration."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w11_jpeg_matrix import _work_bytes
from w11_jpeg_verify import BOUNDS


class W11PublicationMonitorTests(unittest.TestCase):
    def test_atomic_owned_publication_is_conservatively_charged(self):
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder)
            pending = work / "workers/S01_before_onload/observe/pending.png"
            pending.parent.mkdir(parents=True)
            payload = b"published-by-the-owned-worker"
            pending.write_bytes(payload)
            # The actual filesystem changes after enumeration but before stat,
            # as it does when WIC moves pending.png to its final output path.
            enumerated = list(work.rglob("*"))
            pending.rename(pending.parent.parent / "output.png")
            with patch.object(Path, "rglob", return_value=iter(enumerated)):
                charged = _work_bytes(work)
            self.assertEqual(charged, BOUNDS["max_output_bytes"])
            self.assertEqual(_work_bytes(work), len(payload))

    def test_missing_immutable_or_foreign_file_is_not_ignored(self):
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder)
            for relative in ("cases/source.jpg", "workers/foreign/observe/pending.png"):
                with self.subTest(path=relative):
                    path = work / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b"must-remain-accounted")
                    enumerated = list(work.rglob("*"))
                    path.unlink()
                    with patch.object(Path, "rglob", return_value=iter(enumerated)):
                        with self.assertRaises(FileNotFoundError):
                            _work_bytes(work)


if __name__ == "__main__":
    unittest.main()
