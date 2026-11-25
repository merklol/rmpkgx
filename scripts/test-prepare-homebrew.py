#!/usr/bin/env python3
"""Fixture-only tests for packaging; no Homebrew prefix or network writes."""
import hashlib
import importlib.util
import pathlib
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("prepare", pathlib.Path(__file__).with_name("prepare-homebrew.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class PackagingTests(unittest.TestCase):
    def fixture(self, root):
        for name in ["Cargo.lock", "LICENSE", "README.md", "CHANGELOG.md", "src/main.rs", "packaging/rmpkgx.rb.example"]:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
        (root / "Cargo.toml").write_text('[package]\nname="rmpkgx"\nversion="0.1.0"\n')
        (root / "packaging/rmpkgx.rb.example").write_bytes((prepare.ROOT / "packaging/rmpkgx.rb.example").read_bytes())

    def test_archive_and_formula_share_checksum_and_rebuild_is_identical(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp) / "project"
            root.mkdir()
            self.fixture(root)
            (root / ".env").write_text("never package")
            (root / "target").mkdir()
            (root / "target/private").write_text("never package")
            archive, formula, sha = prepare.prepare("fixture-owner", root / "dist", root)
            self.assertEqual(formula, root.resolve() / "Formula/rmpkgx.rb")
            original = archive.read_bytes()
            self.assertEqual(hashlib.sha256(original).hexdigest(), sha)
            self.assertIn(sha, formula.read_text())
            self.assertIn("fixture-owner/rmpkgx/releases/download/v0.1.0/", formula.read_text())
            self.assertNotIn("OWNER", formula.read_text())
            with tarfile.open(archive) as package:
                names = package.getnames()
                self.assertIn("rmpkgx-0.1.0/Cargo.lock", names)
                self.assertFalse(any("/Formula/" in name for name in names))
                self.assertFalse(any(".env" in name or "/target/" in name or "/dist/" in name for name in names))
            prepare.prepare("fixture-owner", root / "dist", root)
            self.assertEqual(original, archive.read_bytes())

    def test_rejects_bad_owner_source_output_and_symlinks(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            self.fixture(root)
            for owner in ['../bad', 'a";oops', 'https://github.com/test', '-bad']:
                with self.assertRaises(ValueError):
                    prepare.prepare(owner, root / "dist", root)
            with self.assertRaises(ValueError):
                prepare.prepare("fixture", root / "src/output", root)
            (root / "src/link").symlink_to(root / "LICENSE")
            with self.assertRaises(ValueError):
                prepare.prepare("fixture", root / "dist", root)


if __name__ == "__main__":
    unittest.main()
