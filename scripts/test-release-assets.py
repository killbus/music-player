#!/usr/bin/env python3
"""Exercise publication gates without contacting GitHub."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release_checksums as checksums

spec = importlib.util.spec_from_file_location("assets", Path(__file__).with_name("release-assets.py"))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)
spec = importlib.util.spec_from_file_location("headless", Path(__file__).with_name("publish-headless.py"))
headless = importlib.util.module_from_spec(spec)
spec.loader.exec_module(headless)


def write_pair(name, data=b"package bytes"):
    archive = Path(name)
    archive.write_bytes(data)
    checksum = archive.with_name(archive.name + ".sha256")
    checksum.write_text(f"{hashlib.sha256(data).hexdigest()}  {archive.name}\n", encoding="ascii", newline="\n")
    return [archive, checksum]


def remote_asset(path):
    return {"name": path.name, "state": "uploaded", "digest": "sha256:" + checksums.file_digest(path)}


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

    def invoke(self, command, names=(), side_effect=None, pattern="*.tar.gz*", corrupt_remote=False):
        published = []

        def transfer(args, **kwargs):
            if side_effect:
                side_effect(args, **kwargs)
            if args[:3] == ["gh", "release", "upload"]:
                asset = remote_asset(Path(args[-1]))
                if corrupt_remote:
                    asset["digest"] = "sha256:" + "0" * 64
                published.append(asset)

        def verify(tag, paths):
            self.assertEqual(tag, os.environ["RELEASE_TAG"])
            checksums.verify_remote_files(paths, published)

        with patch.object(assets.sys, "argv", ["assets", command, pattern]), \
             patch.object(assets, "release", return_value={"assets": [{"name": n} for n in names]}) as lookup, \
             patch.object(assets.subprocess, "run", side_effect=transfer) as upload, \
             patch.object(assets, "verify_uploaded_files", side_effect=verify) as verified:
            assets.main()
            if command == "publish" and published:
                verified.assert_called()
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
        write_pair("new.tar.gz")
        _, upload = self.invoke("publish")
        self.assertEqual(upload.call_count, 2)
        for call in upload.call_args_list:
            self.assertNotIn("--clobber", call.args[0])
            self.assertTrue(call.kwargs["check"])

    def test_preserves_existing_archive_and_its_checksum(self):
        for name in ("old.tar.gz", "old.tar.gz.sha256"):
            Path(name).touch()
        _, upload = self.invoke("publish", ("old.tar.gz", "old.tar.gz.sha256"))
        upload.assert_not_called()

    def test_recovers_checksum_from_preserved_remote_bytes(self):
        Path("old.tar.gz").write_bytes(b"new local build")
        Path("old.tar.gz.sha256").write_text("incorrect local checksum")
        remote = b"original release bytes"
        uploaded = []

        def transfer(command, **kwargs):
            self.assertTrue(kwargs["check"])
            if command[2] == "download":
                (Path(command[-1]) / "old.tar.gz").write_bytes(remote)
            else:
                self.assertNotIn("--clobber", command)
                uploaded.append(Path(command[-1]).read_text())

        self.invoke("publish", ("old.tar.gz",), transfer)
        self.assertEqual(uploaded, [f"{hashlib.sha256(remote).hexdigest()}  old.tar.gz\n"])

    def test_rejects_orphan_checksum(self):
        Path("old.tar.gz").touch()
        with self.assertRaises(SystemExit):
            self.invoke("publish", ("old.tar.gz.sha256",))

    def test_uploads_prerelease_rpm_pair(self):
        Path("Cargo.toml").write_text('[package]\nversion = "0.4.6-rc.1"\n')
        os.environ["RELEASE_TAG"] = "v0.4.6-rc.1"
        write_pair("music-player-0.4.6-rc.1-1.aarch64.rpm")
        _, upload = self.invoke("publish", pattern="*.rpm*")
        self.assertEqual(upload.call_count, 2)

    def test_rejects_nonportable_rpm_before_upload(self):
        write_pair("music-player-0.4.6~rc.1-1.aarch64.rpm")
        with patch.object(assets.sys, "argv", ["assets", "publish", "*.rpm*"]), \
             patch.object(assets, "release", return_value={"assets": []}), \
             patch.object(assets.subprocess, "run") as upload:
            with self.assertRaisesRegex(SystemExit, "Nonportable release asset"):
                assets.main()
            upload.assert_not_called()

    def test_rejects_stale_checksum_before_upload(self):
        _, checksum = write_pair("music-player-0.4.6-rc.1-1.aarch64.rpm")
        checksum.write_text(checksum.read_text().replace("0.4.6-rc.1", "0.4.6~rc.1"))
        with self.assertRaisesRegex(SystemExit, "Checksum target mismatch"):
            self.invoke("publish", pattern="*.rpm*")

    def test_checks_uploaded_digests(self):
        write_pair("new.tar.gz")
        with self.assertRaisesRegex(SystemExit, "Uploaded asset digest mismatch"):
            self.invoke("publish", corrupt_remote=True)


class ChecksumTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(os.chdir, Path.cwd())
        os.chdir(self.directory.name)

    def test_stable_and_prerelease_pairs(self):
        for version in ("0.4.5", "0.4.6-rc.1", "0.4.6-rc.1-dev.abcdef"):
            for package in ("music-player", "music-player-cli"):
                for arch in ("x86_64", "aarch64"):
                    paths = write_pair(f"{package}-{version}-1.{arch}.rpm")
                    checksums.validate_local_files(paths)
                    checksums.verify_remote_files(paths, [remote_asset(p) for p in paths])

    def test_rejects_github_tilde_rewrite(self):
        paths = write_pair("music-player-0.4.6~rc.1-1.aarch64.rpm")
        with self.assertRaisesRegex(SystemExit, "Nonportable"):
            checksums.validate_local_files(paths)
        published = [dict(remote_asset(p), name=p.name.replace("~", ".")) for p in paths]
        with self.assertRaisesRegex(SystemExit, "exact name"):
            checksums.verify_remote_files(paths, published)

    def test_rejects_missing_checksum_and_missing_archive(self):
        paths = write_pair("package.rpm")
        for selected in ([paths[0]], [paths[1]]):
            with self.assertRaisesRegex(SystemExit, "Missing"):
                checksums.validate_local_files(selected)

    def test_rejects_bad_targets_and_hashes(self):
        paths = write_pair("package.rpm")
        original = paths[1].read_text()
        for target in ("wrong.rpm", "../package.rpm", "package.rpm\nextra line"):
            paths[1].write_text(original.replace("package.rpm", target))
            with self.assertRaisesRegex(SystemExit, "target mismatch"):
                checksums.validate_local_files(paths)
        paths[1].write_text(original)
        paths[0].write_bytes(b"changed")
        with self.assertRaisesRegex(SystemExit, "digest mismatch"):
            checksums.validate_local_files(paths)

    def test_accepts_windows_and_binary_checksum_lines(self):
        paths = write_pair("package.tar.gz")
        text = paths[1].read_text().replace("  package", " *package").replace("\n", "\r\n")
        paths[1].write_bytes(text.encode("ascii"))
        checksums.validate_local_files(paths)

    def test_rejects_missing_digest_or_unfinished_upload(self):
        paths = write_pair("package.rpm")
        for replacement in ({"digest": None}, {"state": "starter"}):
            published = [dict(remote_asset(p), **replacement) for p in paths]
            with self.assertRaises(SystemExit):
                checksums.verify_remote_files(paths, published)

    def test_reads_remote_metadata_without_writing(self):
        paths = write_pair("package.rpm")
        response = json.dumps({"assets": [remote_asset(p) for p in paths]})
        with patch.object(checksums.subprocess, "check_output", return_value=response) as read:
            checksums.verify_uploaded_files("v0.4.6-rc.1", paths)
        read.assert_called_once_with(
            ["gh", "release", "view", "v0.4.6-rc.1", "--json", "assets"], text=True)


class HeadlessPublicationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(os.chdir, Path.cwd())
        os.chdir(self.directory.name)
        self.out = Path("dist/build/release")
        self.out.mkdir(parents=True)
        env = patch.dict(os.environ, {
            "RELEASE_TAG": "v0.4.6-rc.1", "RELEASE_VERSION": "0.4.6-rc.1",
            "IMAGE": "ghcr.io/example/music-player", "GITHUB_ACTOR": "actor",
            "GH_TOKEN": "test-token", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1",
        })
        env.start()
        self.addCleanup(env.stop)

    def test_rejects_nonportable_name_before_docker_or_github(self):
        write_pair(self.out / "music-player-cli-0.4.6~rc.1-1.aarch64.rpm")
        with patch.object(headless, "run") as read, patch.object(headless.subprocess, "run") as write:
            with self.assertRaisesRegex(SystemExit, "Nonportable"):
                headless.main()
            read.assert_not_called()
            write.assert_not_called()

    def test_checks_uploaded_files_and_keeps_no_overwrite_policy(self):
        for arch in ("amd64", "arm64"):
            write_pair(self.out / f"build-{arch}.json", b'{"SOURCE_REVISION": "revision"}')
        write_pair(self.out / "music-player-cli-0.4.6-rc.1-1.aarch64.rpm")

        def read(*args):
            if args[0] == "gh":
                return '{"assets": []}'
            if args[:3] == ("docker", "image", "inspect"):
                if args[-1] == "{{.Architecture}}":
                    return args[3].split(":")[-1]
                if "revision" in args[-1]:
                    return "revision"
                return "ghcr.io/example/music-player@sha256:" + "a" * 64
            return json.dumps({"manifests": [
                {"platform": {"os": "linux", "architecture": arch}} for arch in ("amd64", "arm64")
            ]})

        def verify(tag, paths):
            self.assertEqual(tag, "v0.4.6-rc.1")
            self.assertEqual(set(paths), set(self.out.iterdir()))
            raise SystemExit("remote verification failed")

        with patch.object(headless, "run", side_effect=read), \
             patch.object(headless, "require_missing") as missing, \
             patch.object(headless.subprocess, "run") as write, \
             patch.object(headless, "verify_uploaded_files", side_effect=verify) as verified:
            with self.assertRaisesRegex(SystemExit, "remote verification failed"):
                headless.main()
            missing.assert_called_once_with("ghcr.io/example/music-player:0.4.6-rc.1")
            verified.assert_called_once()
            self.assertTrue(any(call.args[0][:3] == ["gh", "release", "upload"] for call in write.call_args_list))
            self.assertTrue(all("--clobber" not in call.args[0] for call in write.call_args_list))
            write.assert_called_with(["docker", "logout", "ghcr.io"], check=False)


if __name__ == "__main__":
    unittest.main()
