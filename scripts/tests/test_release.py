import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import release


class ReleaseTests(unittest.TestCase):
    def test_stable_version_bumps(self):
        for bump, expected in [("patch", "0.27.1"), ("minor", "0.28.0"), ("major", "1.0.0")]:
            self.assertEqual(release.next_version("0.27.0", bump), expected)
        with self.assertRaises(ValueError):
            release.next_version("0.27.0-rc.1", "patch")

    def test_dirty_tree_stops_before_network_or_publication(self):
        with patch.object(release.os, "chdir"), patch.object(release, "output", return_value=" M Cargo.toml"), patch.object(release, "registry_version") as registry:
            with self.assertRaisesRegex(ValueError, "working tree must be clean"):
                release.release("patch")
            registry.assert_not_called()

    def test_registry_cargo_ignores_local_config(self):
        with patch.object(release, "run") as run:
            release.registry_cargo("metadata", "--locked")
            args, kwargs = run.call_args
            self.assertEqual(args[:3], ("cargo", "metadata", "--manifest-path"))
            self.assertEqual(args[-1], "--locked")
            self.assertEqual(str(kwargs["cwd"]), kwargs["env"]["CARGO_HOME"])
            self.assertIn("finery-release-cargo-", kwargs["env"]["CARGO_HOME"])


class ReleaseGitTests(unittest.TestCase):
    def test_release_pushes_matching_commit_and_tag_without_building(self):
        environment = {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Release test",
            "GIT_AUTHOR_EMAIL": "release@example.invalid",
            "GIT_COMMITTER_NAME": "Release test",
            "GIT_COMMITTER_EMAIL": "release@example.invalid",
        }
        original_cwd = Path.cwd()
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, environment):
            root = Path(directory)
            repo = root / "repo"
            repo.mkdir()
            (repo / "scripts").mkdir()
            (repo / "Cargo.toml").write_text('[package]\nname = "finery"\nversion = "0.27.0"\n[dependencies]\ntuicore = "0.40.0"\n')
            (repo / "Cargo.lock").write_text("version = 4\n")

            def git(*args):
                return subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True).stdout.strip()

            git("init", "--initial-branch=main")
            git("add", "Cargo.toml", "Cargo.lock")
            git("commit", "-m", "Initial")
            git("init", "--bare", str(root / "remote.git"))
            git("remote", "add", "origin", str(root / "remote.git"))
            git("push", "origin", "main")
            real_run = release.run
            calls = []

            def run(*args, **kwargs):
                calls.append(args)
                if args[:2] == ("gh", "auth"):
                    return subprocess.CompletedProcess(args, 0)
                return real_run(*args, **kwargs)

            try:
                with patch.object(release, "__file__", str(repo / "scripts/release.py")), patch.object(release, "run", side_effect=run), patch.object(release, "registry_version", side_effect=[None, {"yanked": False}]), patch.object(release, "registry_cargo") as cargo:
                    release.release("patch")
                self.assertIn('version = "0.27.1"', (repo / "Cargo.toml").read_text())
                self.assertEqual(git("rev-parse", "HEAD"), git("rev-parse", "v0.27.1^{commit}"))
                self.assertIn(git("rev-parse", "HEAD"), git("ls-remote", "origin", "refs/heads/main"))
                self.assertIn("refs/tags/v0.27.1", git("ls-remote", "origin", "refs/tags/v0.27.1"))
                self.assertIn(("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.27.1"), calls)
                self.assertEqual([call.args[0] for call in cargo.call_args_list], ["update", "update"])
                self.assertEqual(git("status", "--porcelain"), "")
            finally:
                os.chdir(original_cwd)


class PublishTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("publish_crate", SCRIPTS / "publish-crate.py")
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)

    def test_retry_skips_published_version(self):
        with patch.object(self.module, "registry_version", return_value={"num": "0.27.0"}), patch.object(self.module.subprocess, "run") as run:
            self.module.main()
            run.assert_not_called()

    def test_publication_failure_propagates(self):
        with patch.object(self.module, "registry_version", return_value=None), patch.dict(self.module.os.environ, {"CARGO_REGISTRY_TOKEN": "test"}), patch.object(self.module.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                self.module.main()


if __name__ == "__main__":
    unittest.main()
