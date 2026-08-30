#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat
export PAGER=cat

crate_name="finery"

usage() {
    cat <<'EOF'
Usage: ./scripts/release.sh [major|minor|patch]

Bump Finery (default: minor), validate, commit, tag, and publish to crates.io.
Latest stable Tuicore is selected from crates.io. This script never pushes.
EOF
}

confirm() {
    local answer
    [[ -t 0 ]] || return 1
    read -r -p "$1 [y/N] " answer || return 1
    [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]]
}

crates_io_version_state() {
    python3 - "$1" "$2" <<'PY'
import json, sys, urllib.error, urllib.parse, urllib.request
crate, expected = sys.argv[1:]
request = urllib.request.Request(f"https://crates.io/api/v1/crates/{urllib.parse.quote(crate, safe='')}/{urllib.parse.quote(expected, safe='')}", headers={"User-Agent": "finery-release/1.0"})
try:
    with urllib.request.urlopen(request, timeout=10) as response: payload = json.load(response)
except urllib.error.HTTPError as error:
    if error.code == 404: print("absent"); raise SystemExit(0)
    raise SystemExit(f"error: crates.io version query failed with HTTP {error.code}")
except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
    raise SystemExit(f"error: crates.io version query failed: {error}")
if not isinstance(payload, dict) or not isinstance(payload.get("version"), dict) or payload["version"].get("num") != expected: raise SystemExit("error: invalid crates.io version response")
print("present")
PY
}

case "${1:-minor}" in -h|--help) usage; exit 0;; major|minor|patch) bump="${1:-minor}";; *) usage >&2; exit 2;; esac
(( $# <= 1 )) || { usage >&2; exit 2; }

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir/.."
for command in git cargo python3; do command -v "$command" >/dev/null 2>&1 || { printf 'error: required command not found: %s\n' "$command" >&2; exit 1; }; done
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || { printf 'error: Git working tree must be clean, including untracked files\n' >&2; exit 1; }
branch="$(git symbolic-ref --quiet --short HEAD)" || { printf 'error: cannot release from detached HEAD\n' >&2; exit 1; }

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
cargo_token="${CARGO_REGISTRIES_CRATES_IO_TOKEN:-${CARGO_REGISTRY_TOKEN:-}}"
if [[ -z "$cargo_token" ]]; then
    cargo_token="$(python3 - "$cargo_home/credentials.toml" "$cargo_home/credentials" <<'PY'
import pathlib, sys, tomllib
for raw in sys.argv[1:]:
    path = pathlib.Path(raw)
    if not path.is_file(): continue
    try: data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError): continue
    for token in (data.get("registries", {}).get("crates-io", {}).get("token"), data.get("registry", {}).get("token")):
        if isinstance(token, str) and token.strip(): print(token.strip()); raise SystemExit(0)
raise SystemExit(1)
PY
)" || true
fi
[[ -n "$cargo_token" ]] || { printf 'error: cannot resolve crates.io token; use `cargo login` or CARGO_REGISTRY_TOKEN\n' >&2; exit 1; }
unset cargo_token

latest_tuicore="$(python3 <<'PY'
import json, re, urllib.error, urllib.request
request = urllib.request.Request("https://crates.io/api/v1/crates/tuicore", headers={"User-Agent": "finery-release/1.0"})
try:
    with urllib.request.urlopen(request, timeout=10) as response: versions = json.load(response).get("versions")
except (OSError, urllib.error.URLError, json.JSONDecodeError) as error: raise SystemExit(f"error: failed to query crates.io for Tuicore: {error}")
stable = [(tuple(map(int, match.groups())), version["num"]) for version in versions if isinstance(version, dict) and version.get("yanked") is False and isinstance(version.get("num"), str) and (match := re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\+[0-9A-Za-z.-]+)?", version["num"]))]
if not stable: raise SystemExit("error: crates.io has no stable non-yanked Tuicore version")
print(max(stable)[1])
PY
)"

read -r old_version new_version declared_tuicore < <(python3 - "$bump" "$latest_tuicore" <<'PY'
import pathlib, re, sys
bump, latest = sys.argv[1:]; text = pathlib.Path("Cargo.toml").read_text()
package = re.search(r'(?ms)^\[package\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', text); dependencies = re.search(r'(?ms)^\[dependencies\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', text)
version = re.search(r'(?m)^\s*version\s*=\s*["\']([^"\']+)["\']', package.group("body")); tuicore = re.search(r'(?m)^\s*tuicore\s*=\s*\{[^\n}]*\bversion\s*=\s*["\']([^"\']+)["\']', dependencies.group("body"))
if not version or not tuicore: raise SystemExit("error: package version and Tuicore dependency must declare string versions")
def parse(label, value):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value): raise SystemExit(f"error: {label} version must be X.Y.Z, found {value!r}")
    return tuple(map(int, value.split(".")))
major, minor, patch = parse("Finery", version.group(1)); declared = parse("declared Tuicore", tuicore.group(1)); current = parse("latest Tuicore", latest.split("+", 1)[0])
if declared > current: raise SystemExit(f"error: declared Tuicore {tuicore.group(1)} is newer than crates.io {latest}")
new = (major + 1, 0, 0) if bump == "major" else (major, minor + 1, 0) if bump == "minor" else (major, minor, patch + 1)
print(version.group(1), ".".join(map(str, new)), tuicore.group(1))
PY
)

tag="v$new_version"
git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null && { printf 'error: tag %s already exists\n' "$tag" >&2; exit 1; }
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
    printf 'error: tag %s already exists on origin\n' "$tag" >&2
    exit 1
else
    remote_tag_status=$?
fi
if (( remote_tag_status != 2 )); then
    printf 'error: failed to verify tag %s on origin\n' "$tag" >&2
    exit 1
fi
[[ "$(crates_io_version_state "$crate_name" "$new_version")" == absent ]] || { printf 'error: %s %s already exists on crates.io\n' "$crate_name" "$new_version" >&2; exit 1; }

mutated=false; committed=false; tagged=false
failure_guidance() {
    status=$?
    if (( status != 0 )) && [[ "$mutated" == true ]]; then
        printf '\nRelease stopped; inspect with: git --no-pager status && git --no-pager diff\n' >&2
        [[ "$committed" == true ]] && printf 'Release commit remains. Inspect with: git --no-pager show --stat HEAD\n' >&2
        [[ "$tagged" == true ]] && printf 'Release tag remains. Verify clean tree, tag identity, and crates.io absence before resuming.\n' >&2
        printf 'Do not rerun bump or rewrite history automatically.\n' >&2
    fi
    exit "$status"
}
trap failure_guidance EXIT
mutated=true
python3 - "$old_version" "$new_version" "$declared_tuicore" "$latest_tuicore" "$crate_name" <<'PY'
import pathlib, re, sys
old, new, old_tui, new_tui, crate = sys.argv[1:]; manifest_path = pathlib.Path("Cargo.toml"); manifest = manifest_path.read_text()
if old_tui != new_tui:
    manifest, count = re.subn(r'(?m)^(\s*tuicore\s*=\s*\{[^\n}]*\bversion\s*=\s*["\'])' + re.escape(old_tui) + r'(["\'])', r'\g<1>' + new_tui + r'\g<2>', manifest, count=1)
    if count != 1: raise SystemExit("error: Tuicore dependency changed unexpectedly")
package = re.search(r'(?ms)^\[package\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', manifest); body, count = re.subn(r'(?m)^(\s*version\s*=\s*["\'])' + re.escape(old) + r'(["\'])', r'\g<1>' + new + r'\g<2>', package.group("body"), count=1)
if count != 1: raise SystemExit("error: package version changed unexpectedly")
manifest_path.write_text(manifest[:package.start("body")] + body + manifest[package.end("body"):])
lock_path = pathlib.Path("Cargo.lock"); lock = lock_path.read_text(); blocks = list(re.finditer(r'(?ms)^\[\[package\]\]\s*$\n.*?(?=^\[\[package\]\]|\Z)', lock)); matches = [(block, re.search(r'(?m)^version\s*=\s*["\']([^"\']+)["\']', block.group())) for block in blocks if re.search(r'(?m)^name\s*=\s*["\']([^"\']+)["\']', block.group()).group(1) == crate and re.search(r'(?m)^version\s*=\s*["\']([^"\']+)["\']', block.group()).group(1) == old]
if len(matches) != 1: raise SystemExit(f"error: expected one {crate} {old} lock entry, found {len(matches)}")
block, version = matches[0]; start = block.start() + version.start(1); end = block.start() + version.end(1); lock_path.write_text(lock[:start] + new + lock[end:])
PY
cargo update -p tuicore --precise "$latest_tuicore"
cargo test --locked
cargo package --locked --allow-dirty --registry crates-io
cargo publish --locked --allow-dirty --dry-run --registry crates-io
printf '\nFinery: %s -> %s\nTuicore: %s -> %s (latest crates.io)\n' "$old_version" "$new_version" "$declared_tuicore" "$latest_tuicore"
git --no-pager diff -- Cargo.toml Cargo.lock
confirm "Commit, tag, and publish Finery $new_version with Tuicore $latest_tuicore?" || { printf 'Release canceled; dependency/version changes remain in working tree.\n' >&2; exit 1; }
git add Cargo.toml Cargo.lock; git commit -m "release: $tag"; committed=true; git tag -a "$tag" -m "release: $tag"; tagged=true
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || { printf 'error: Git working tree must be clean before publishing\n' >&2; exit 1; }
[[ "$(git rev-parse HEAD)" == "$(git rev-parse "$tag^{commit}")" ]] || { printf 'error: HEAD must match release tag %s before publishing\n' "$tag" >&2; exit 1; }
[[ "$(crates_io_version_state "$crate_name" "$new_version")" == absent ]] || { printf 'error: %s %s appeared on crates.io before publish\n' "$crate_name" "$new_version" >&2; exit 1; }
if ! cargo publish --locked --registry crates-io; then
    [[ "$(crates_io_version_state "$crate_name" "$new_version")" == present ]] || { printf '\nPublish failed; release commit and tag %s remain.\nResume only after: tree is clean; HEAD equals %s; %s %s is still absent on crates.io.\nThen run: cargo publish --locked --registry crates-io\n' "$tag" "$tag" "$crate_name" "$new_version" >&2; trap - EXIT; exit 1; }
    printf '\n%s %s is present on crates.io; publication succeeded despite cargo error.\n' "$crate_name" "$new_version"
fi
trap - EXIT
printf '\nPublished %s. Push release commit and tag when ready:\n' "$tag"
printf 'git push origin %s\ngit push origin %s\n' "$branch" "$tag"
