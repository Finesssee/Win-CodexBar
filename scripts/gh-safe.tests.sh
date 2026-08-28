#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin"
log="$test_root/gh.log"

cat > "$test_root/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%q ' "$@" >> "${GH_SAFE_TEST_LOG:?}"
printf '\n' >> "${GH_SAFE_TEST_LOG:?}"
if [[ "${FAKE_GH_MODE:-ok}" == cross ]]; then
  printf '%s\n' 'steipete/CodexBar|https://github.com/steipete/CodexBar'
  exit 0
fi
if [[ "${1:-}" == repo && "${2:-}" == view ]]; then
  printf '%s\n' 'nesszer/Win-CodexBar|https://github.com/nesszer/Win-CodexBar'
elif [[ "${1:-}" == pr && "${2:-}" == view ]]; then
  printf '%s\n' 'https://github.com/nesszer/Win-CodexBar/pull/361'
elif [[ "${1:-}" == issue && "${2:-}" == view ]]; then
  printf '%s\n' 'https://github.com/nesszer/Win-CodexBar/issues/123'
elif [[ "${1:-}" == api ]]; then
  printf '%s\n' 'https://github.com/nesszer/Win-CodexBar/releases/tag/v1.2.3'
fi
EOF
chmod +x "$test_root/bin/gh"
export PATH="$test_root/bin:$PATH"
export GH_SAFE_TEST_LOG="$log"

expect_fail() {
  if "$@" >/dev/null 2>&1; then
    echo "Expected failure: $*" >&2
    exit 1
  fi
}

bash -n "$repo_root/scripts/gh-safe.sh"

bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind repo --what-if -- \
  pr create --title test --body test >/dev/null

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo steipete/CodexBar --verify-kind repo --what-if -- \
  pr create --title test --body test

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo other/repo --verify-kind repo --what-if -- \
  pr create --title test --body test

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind pr --target 361 --what-if -- \
  pr comment 362 --body test

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind pr --target 361 --what-if -- \
  pr comment 999 --comment 361

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind pr --target 361 --what-if -- \
  pr close

expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind pr --target 361 --what-if -- \
  pr comment 361 --repo steipete/CodexBar --body test

FAKE_GH_MODE=cross expect_fail bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind repo --what-if -- \
  pr create --title test --body test

: > "$log"
bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind pr --target 361 -- \
  pr comment 361 --body test >/dev/null

grep -Fq 'pr comment 361 --body test --repo nesszer/Win-CodexBar' "$log" || {
  echo 'Safe wrapper did not bind the canonical repo on mutation.' >&2
  cat "$log" >&2
  exit 1
}
: > "$log"
bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind issue --target 123 --what-if -- \
  issue close 123 >/dev/null

: > "$log"
bash "$repo_root/scripts/gh-safe.sh" \
  --repo nesszer/Win-CodexBar --verify-kind release --target v1.2.3 --what-if -- \
  release upload v1.2.3 dist/app.zip >/dev/null

echo 'GitHub write-safety shell tests passed.'
