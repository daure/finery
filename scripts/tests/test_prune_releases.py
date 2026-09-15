from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("prune_releases", Path(__file__).resolve().parents[1] / "prune-releases.py")
retention = importlib.util.module_from_spec(spec)
spec.loader.exec_module(retention)


def release(number, **overrides):
    return {
        "id": number,
        "tag_name": f"v1.0.{number}",
        "draft": False,
        "prerelease": False,
        "published_at": (datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(days=number)).isoformat(),
        **overrides,
    }


class RetentionTests(unittest.TestCase):
    def setUp(self):
        silence = patch("builtins.print")
        silence.start()
        self.addCleanup(silence.stop)

    def test_keeps_newest_thirty_by_publication_time_and_ignores_unpublished_releases(self):
        values = [release(number) for number in range(31, 0, -1)]
        values.extend([release(100, draft=True, published_at=None), release(101, prerelease=True)])
        self.assertEqual([item["id"] for item in retention.select_deletions(values)], [1])
        self.assertEqual(retention.select_deletions(values[:30]), [])
        self.assertEqual(retention.select_deletions([]), [])
        values[0]["published_at"] = release(1)["published_at"]
        values[-3]["published_at"] = release(40)["published_at"]
        self.assertEqual([item["id"] for item in retention.select_deletions(values)], [31])

    def test_timestamps_use_timezone_and_release_id_breaks_ties(self):
        values = [release(1, published_at="2026-01-01T01:00:00+02:00"), release(2, published_at="2026-01-01T00:00:00Z")]
        self.assertEqual(retention.select_deletions(values, keep=1)[0]["id"], 1)
        values[0]["published_at"] = values[1]["published_at"]
        self.assertEqual(retention.select_deletions(values, keep=1)[0]["id"], 1)

    def test_invalid_or_incomplete_data_fails_closed(self):
        for values, keep, tag in [
            ([release(1)], 0, None),
            ([release(1)], 30, "missing"),
            ([release(1), release(2)], 1, "v1.0.1"),
            ([release(1), release(1)], 30, None),
            ([release(1, draft=None)], 30, None),
            ([release(1, published_at=None)], 30, None),
        ]:
            with self.subTest(values=values, keep=keep, tag=tag), self.assertRaises(ValueError):
                retention.select_deletions(values, keep, tag)

    def test_dry_run_fetches_all_pages_without_mutations(self):
        pages = [[release(n) for n in range(131, 31, -1)], [release(n) for n in range(31, 0, -1)]]
        with patch.object(retention.subprocess, "run", side_effect=[subprocess.CompletedProcess([], 0, json.dumps(pages)), subprocess.CompletedProcess([], 0, json.dumps(release(131)))]) as run:
            self.assertEqual(retention.prune("daure/finery"), 101)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args_list[0].args[0], ["gh", "api", "--hostname", "github.com", "repos/daure/finery/releases?per_page=100", "--paginate", "--slurp"])
        self.assertTrue(all("DELETE" not in call.args[0] for call in run.call_args_list))

    def test_apply_rechecks_candidates_and_deletes_only_release_ids(self):
        pages = [[release(n) for n in range(1, 32)]]
        with patch.object(retention, "gh_api", side_effect=[pages, release(31), release(1), None]) as api:
            self.assertEqual(retention.prune("daure/tuido", published_tag="v1.0.31", apply=True), 1)
        self.assertEqual(api.call_args_list[-1].args, ("repos/daure/tuido/releases/1", "--method", "DELETE"))
        with patch.object(retention, "gh_api", side_effect=[pages, release(31), release(1, prerelease=True)]) as api:
            with self.assertRaisesRegex(ValueError, "changed"):
                retention.prune("daure/tuido", published_tag="v1.0.31", apply=True)
            self.assertEqual(api.call_count, 3)

    def test_pinned_latest_release_and_delete_failures_stop_cleanup(self):
        pages = [[release(n) for n in range(1, 33)]]
        with patch.object(retention, "gh_api", side_effect=[pages, release(1)]) as api:
            with self.assertRaisesRegex(ValueError, "latest release"):
                retention.prune("daure/finery", published_tag="v1.0.32", apply=True)
            self.assertEqual(api.call_count, 2)
        with patch.object(retention, "gh_api", side_effect=[pages, release(32), release(1), subprocess.CalledProcessError(1, "gh")]) as api:
            with self.assertRaises(subprocess.CalledProcessError):
                retention.prune("daure/finery", published_tag="v1.0.32", apply=True)
            self.assertEqual(api.call_count, 4)

    def test_scope_missing_confirmation_and_api_failure_prevent_deletion(self):
        with patch.object(retention, "gh_api") as api:
            with self.assertRaises(ValueError):
                retention.prune("daure/tuicore", published_tag="v1.0.31", apply=True)
            with self.assertRaises(ValueError):
                retention.prune("daure/finery", apply=True)
            api.assert_not_called()
        with patch.object(retention, "gh_api", side_effect=subprocess.CalledProcessError(1, "gh")) as api:
            with self.assertRaises(subprocess.CalledProcessError):
                retention.prune("daure/finery", published_tag="v1.0.31", apply=True)
            api.assert_called_once()


if __name__ == "__main__":
    unittest.main()
