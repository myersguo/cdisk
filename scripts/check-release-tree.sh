#!/bin/sh
set -eu

pattern='AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|npm_[A-Za-z0-9]{20,}|(^|[^[:alnum:]_])sk-[A-Za-z0-9_-]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|code\.byted\.org|bytedance\.net'
matches=$(mktemp "${TMPDIR:-/tmp}/cdisk-release-scan.XXXXXX")
trap 'rm -f "$matches"' EXIT

git ls-files --cached --others --exclude-standard | while IFS= read -r file; do
  test -f "$file" || continue
  case "$file" in
    pnpm-lock.yaml|src-tauri/Cargo.lock|scripts/check-release-tree.sh|*.png|*.icns)
      continue
      ;;
  esac
  grep -EnH "$pattern" "$file" >>"$matches" || test "$?" -eq 1
  if test -n "${HOME:-}"; then
    grep -FnH "$HOME" "$file" >>"$matches" || test "$?" -eq 1
  fi
done

if test -s "$matches"; then
  cat "$matches"
  exit 1
fi
