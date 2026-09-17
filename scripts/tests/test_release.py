import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import release


class ReleaseTests(unittest.TestCase):
    def test_run_disables_pagers(self):
        with patch.object(release.subprocess, "run") as subprocess_run:
            release.run("git", "status", env={"GIT_EDITOR": "true"})
        environment = subprocess_run.call_args.kwargs["env"]
        self.assertEqual(environment["PAGER"], "cat")
        self.assertEqual(environment["GIT_PAGER"], "cat")
        self.assertEqual(environment["CARGO_PAGER"], "cat")
        self.assertEqual(environment["GIT_EDITOR"], "true")

    def test_stable_version_bumps(self):
        for bump, expected in [("patch", "0.27.1"), ("minor", "0.28.0"), ("major", "1.0.0")]:
            self.assertEqual(release.next_version("0.27.0", bump), expected)
        with self.assertRaises(ValueError):
            release.next_version("0.27.0-rc.1", "patch")

    def test_dirty_tree_stops_before_network_or_publication(self):
        with patch.object(release.os, "chdir"), patch.object(release, "output", return_value=" M Cargo.toml"), patch.object(release, "run") as run:
            with self.assertRaisesRegex(ValueError, "working tree must be clean"):
                release.release("patch")
            run.assert_not_called()


class ReleaseGitTests(unittest.TestCase):
    def test_release_pushes_matching_commit_and_tag_without_building(self):
        self.check_release(refresh_lockfile=True)

    def test_release_refreshes_stale_path_dependency_before_locked_checks(self):
        self.check_release(refresh_lockfile=False)

    def check_release(self, refresh_lockfile):
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
            os.environ["CARGO_HOME"] = str(root / "cargo-home")
            (root / "cargo-home").mkdir()
            repo = root / "repo"
            repo.mkdir()
            (repo / "scripts").mkdir()
            (repo / "src").mkdir()
            (repo / "src/main.rs").write_text("fn main() {}\n")
            (repo / "Cargo.toml").write_text('[package]\nname = "finery"\nversion = "0.27.0"\n[dependencies]\ntuicore = { path = "../tuicore" }\n')
            local = root / "tuicore"
            (local / "src").mkdir(parents=True)
            (local / "src/lib.rs").write_text("")
            local_manifest = '[package]\nname = "tuicore"\nversion = "1.0.0"\n'
            (local / "Cargo.toml").write_text(local_manifest)
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=repo, check=True)

            def git(*args):
                return subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True).stdout.strip()

            git("init", "--initial-branch=main")
            git("add", "Cargo.toml", "Cargo.lock", "src")
            git("commit", "-m", "Initial")
            git("init", "--bare", str(root / "remote.git"))
            git("remote", "add", "origin", str(root / "remote.git"))
            git("push", "origin", "main")
            (local / "Cargo.toml").write_text(local_manifest.replace("1.0.0", "2.0.0"))
            if refresh_lockfile:
                subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=repo, check=True)
                self.assertIn("Cargo.lock", git("status", "--porcelain"))
            else:
                self.assertEqual(git("status", "--porcelain"), "")
            real_run = release.run
            calls = []

            def run(*args, **kwargs):
                calls.append((args, kwargs))
                if args[:2] == ("gh", "auth"):
                    return subprocess.CompletedProcess(args, 0)
                if args[:2] in (("cargo", "clippy"), ("cargo", "test")):
                    return real_run("cargo", "metadata", "--locked", "--offline", "--format-version", "1", stdout=subprocess.PIPE)
                if args[0] in ("cargo", "python3") and args[:2] != ("cargo", "update"):
                    return subprocess.CompletedProcess(args, 0)
                return real_run(*args, **kwargs)

            try:
                with patch.object(release, "__file__", str(repo / "scripts/release.py")), patch.object(release, "run", side_effect=run):
                    release.release("patch")
                self.assertIn('version = "0.27.1"', (repo / "Cargo.toml").read_text())
                self.assertEqual(git("rev-parse", "HEAD"), git("rev-parse", "v0.27.1^{commit}"))
                self.assertIn(git("rev-parse", "HEAD"), git("ls-remote", "origin", "refs/heads/main"))
                self.assertIn("refs/tags/v0.27.1", git("ls-remote", "origin", "refs/tags/v0.27.1"))
                self.assertIn(
                    (("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.27.1"), {}),
                    calls,
                )
                tag_call = next(
                    call
                    for call in calls
                    if call[0] == ("git", "tag", "-a", "v0.27.1", "-m", "release: v0.27.1")
                )
                self.assertEqual(tag_call[1]["env"], {"GIT_EDITOR": "true"})
                self.assertEqual(
                    [call for call, _ in calls if call[0] == "cargo"],
                    [
                        ("cargo", "fmt", "--all", "--check"),
                        (
                            "cargo",
                            "fmt",
                            "--manifest-path",
                            "tools/release-command/Cargo.toml",
                            "--check",
                        ),
                        ("cargo", "update", "--workspace"),
                        ("cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"),
                        ("cargo", "test", "--locked"),
                        ("cargo", "update", "--workspace"),
                    ],
                )
                self.assertIn(
                    (("python3", "-m", "unittest", "discover", "-s", "scripts/tests"), {}),
                    calls,
                )
                self.assertEqual(tomllib.loads((repo / "Cargo.toml").read_text())["dependencies"]["tuicore"], {"path": "../tuicore"})
                packages = tomllib.loads((repo / "Cargo.lock").read_text())["package"]
                self.assertIn({"name": "tuicore", "version": "2.0.0"}, packages)
                self.assertEqual(next(p["version"] for p in packages if p["name"] == "finery"), "0.27.1")
                self.assertEqual(git("status", "--porcelain"), "")
            finally:
                os.chdir(original_cwd)


if __name__ == "__main__":
    unittest.main()
