#!/usr/bin/env python3
"""Exercise publication gates without contacting GitHub."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("assets", Path(__file__).with_name("release-assets.py"))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        cwd = Path.cwd()
        self.addCleanup(os.chdir, cwd)
        os.chdir(self.directory.name)
        Path("Cargo.toml").write_text('[package]\nversion = "0.4.5"\n')
        self.environment = patch.dict(os.environ, {
            "PUBLISH_RELEASE": "true", "RELEASE_TAG": "v0.4.5",
            "GITHUB_ENV": str(Path("environment").resolve()),
        })
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def invoke(self, command, names=()):
        with patch.object(assets.sys, "argv", ["assets", command, "*.tar.gz*"]), \
             patch.object(assets, "release", return_value={"assets": [{"name": n} for n in names]}) as lookup, \
             patch.object(assets.subprocess, "run") as upload:
            assets.main()
            return lookup, upload

    def test_validation_does_not_contact_release(self):
        os.environ["PUBLISH_RELEASE"] = "false"
        lookup, upload = self.invoke("prepare")
        lookup.assert_not_called()
        upload.assert_not_called()
        self.assertEqual(Path("environment").read_text(), "RELEASE_VERSION=v0.4.5\n")

    def test_publish_requires_explicit_tag(self):
        os.environ["RELEASE_TAG"] = ""
        with self.assertRaises(SystemExit):
            self.invoke("prepare")

    def test_mismatched_version_rejected(self):
        os.environ["RELEASE_TAG"] = "v0.4.4"
        with self.assertRaises(SystemExit):
            self.invoke("prepare")

    def test_publish_disabled(self):
        os.environ["PUBLISH_RELEASE"] = "false"
        with self.assertRaises(SystemExit):
            self.invoke("publish")

    def test_uploads_new_pair_without_clobber(self):
        for name in ("new.tar.gz", "new.tar.gz.sha256"):
            Path(name).touch()
        _, upload = self.invoke("publish")
        self.assertEqual(upload.call_count, 2)
        for call in upload.call_args_list:
            self.assertNotIn("--clobber", call.args[0])
            self.assertTrue(call.kwargs["check"])

    def test_preserves_existing_archive_and_its_checksum(self):
        for name in ("old.tar.gz", "old.tar.gz.sha256"):
            Path(name).touch()
        _, upload = self.invoke("publish", ("old.tar.gz",))
        upload.assert_not_called()

    def test_rejects_orphan_checksum(self):
        Path("old.tar.gz").touch()
        with self.assertRaises(SystemExit):
            self.invoke("publish", ("old.tar.gz.sha256",))


if __name__ == "__main__":
    unittest.main()
