#!/bin/sh
set -eu

usage() {
  echo "Usage: verify-release-tag.sh OWNER/REPO TAG EXPECTED_COMMIT_SHA" >&2
}

[ "$#" -eq 3 ] || { usage; exit 2; }
repository="$1"
tag="$2"
expected_sha="$3"

case "$repository" in
  */*) ;;
  *) echo "repository must look like OWNER/REPO" >&2; exit 2 ;;
esac
case "$expected_sha" in
  *[!0-9a-f]*|'') echo "expected commit must be a lowercase hexadecimal SHA" >&2; exit 2 ;;
esac
[ "${#expected_sha}" -eq 40 ] || { echo "expected commit must be a 40-character SHA" >&2; exit 2; }

resolved_sha=$(gh api "repos/${repository}/commits/${tag}" --jq .sha)
if [ "$resolved_sha" != "$expected_sha" ]; then
  echo "release tag ${tag} resolves to ${resolved_sha}, expected ${expected_sha}" >&2
  exit 1
fi

printf 'release_tag_commit=%s\n' "$resolved_sha"
