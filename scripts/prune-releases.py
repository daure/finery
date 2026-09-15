"""Keep the newest published stable releases; preserve drafts, prereleases, and tags."""

import argparse
from datetime import datetime
import json
import subprocess
import sys

REPOSITORIES = ("daure/finery", "daure/tuido")


def gh_api(endpoint, *options):
    result = subprocess.run(
        ["gh", "api", "--hostname", "github.com", endpoint, *options],
        check=True, capture_output=True, text=True, timeout=120,
    )
    return json.loads(result.stdout) if result.stdout.strip() else None


def stable_release(value):
    if not isinstance(value, dict) or type(value.get("draft")) is not bool or type(value.get("prerelease")) is not bool:
        raise ValueError("Release response must include draft and prerelease flags")
    if value["draft"] or value["prerelease"]:
        return None
    release_id, tag, published = value.get("id"), value.get("tag_name"), value.get("published_at")
    if type(release_id) is not int or release_id <= 0 or not isinstance(tag, str) or not tag or not isinstance(published, str):
        raise ValueError("Stable release response is missing identity or publication time")
    timestamp = datetime.fromisoformat(published.replace("Z", "+00:00"))
    if timestamp.tzinfo is None:
        raise ValueError("Publication time must include a timezone")
    return {"id": release_id, "tag": tag, "published": timestamp}


def select_deletions(values, keep=30, published_tag=None):
    if type(keep) is not int or keep < 1:
        raise ValueError("Keep count must be a positive integer")
    releases = [release for value in values if (release := stable_release(value)) is not None]
    if len({release["id"] for release in releases}) != len(releases):
        raise ValueError("Release list changed during pagination; retry with a fresh list")
    releases.sort(key=lambda release: (release["published"], release["id"]), reverse=True)
    if published_tag is not None and not any(release["tag"] == published_tag for release in releases[:keep]):
        raise ValueError("Triggering release must be published, stable, and in the retained set")
    return list(reversed(releases[keep:]))


def prune(repository, keep=30, published_tag=None, apply=False):
    if repository not in REPOSITORIES:
        raise ValueError("Retention is limited to Finery and Tuido")
    if apply and not published_tag:
        raise ValueError("Applying retention requires the successfully published tag")
    pages = gh_api(f"repos/{repository}/releases?per_page=100", "--paginate", "--slurp")
    if not isinstance(pages, list) or not all(isinstance(page, list) for page in pages):
        raise ValueError("Expected complete paginated release lists")
    candidates = select_deletions([release for page in pages for release in page], keep, published_tag)
    if candidates:
        latest = stable_release(gh_api(f"repos/{repository}/releases/latest"))
        if latest is None or any(release["id"] == latest["id"] for release in candidates):
            raise ValueError("GitHub's latest release must remain available; refusing cleanup")
    mode = "Apply" if apply else "Dry run"
    print(f"{mode}: {len(candidates)} older stable releases; retain up to {keep} in {repository}.")
    for release in candidates:
        print(f"{'Delete' if apply else 'Would delete'} {json.dumps(release['tag'])} (release {release['id']})")
        if apply:
            endpoint = f"repos/{repository}/releases/{release['id']}"
            if stable_release(gh_api(endpoint)) != release:
                raise ValueError("Release changed after selection; refusing deletion")
            # The releases endpoint removes assets, but never deletes the Git reference.
            gh_api(endpoint, "--method", "DELETE")
    return len(candidates)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, choices=REPOSITORIES)
    parser.add_argument("--keep", type=int, default=30)
    parser.add_argument("--published-tag")
    parser.add_argument("--apply", action="store_true", help="Delete selected releases; default is a dry run")
    args = parser.parse_args()
    try:
        prune(args.repo, args.keep, args.published_tag, args.apply)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(f"error: retention stopped: {error}")


if __name__ == "__main__":
    main()
