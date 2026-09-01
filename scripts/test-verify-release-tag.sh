#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp_dir=$(mktemp -d)
cleanup() { find "$tmp_dir" -depth -delete; }
trap cleanup EXIT HUP INT TERM

cat > "${tmp_dir}/gh" <<'EOF'
#!/bin/sh
set -eu
[ "$1" = "api" ]
printf '%s\n' "${FAKE_GH_RESOLVED_SHA:?}"
EOF
chmod 0755 "${tmp_dir}/gh"

expected=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
PATH="${tmp_dir}:${PATH}" FAKE_GH_RESOLVED_SHA="$expected" \
  "${script_dir}/verify-release-tag.sh" owner/repo v0.1.1 "$expected" >/dev/null

if PATH="${tmp_dir}:${PATH}" \
  FAKE_GH_RESOLVED_SHA=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  "${script_dir}/verify-release-tag.sh" owner/repo v0.1.1 "$expected" >/dev/null 2>&1; then
  echo "tag guard accepted a mismatched peeled commit" >&2
  exit 1
fi
