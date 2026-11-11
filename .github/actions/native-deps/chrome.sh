#!/usr/bin/env bash
set -euo pipefail

probe_chrome_runtime() {
  local browser=$1 directory=$2
  timeout --kill-after=5s 20s "$browser" \
    --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage \
    --disable-background-networking --disable-component-update \
    --no-first-run --no-default-browser-check --disable-search-engine-choice-screen \
    '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost' \
    "--user-data-dir=$directory/profile" --dump-dom \
    'data:text/html,<title>aggr-runtime-check</title>' \
    > "$directory/page.html" 2> "$directory/chrome.log" || return 1
  grep -Fq '<title>aggr-runtime-check</title>' "$directory/page.html"
}

install_chrome_dependencies() {
  local browser_dir=$1
  local -a dependencies
  mapfile -t dependencies < "$browser_dir/deb.deps"
  sudo apt-get update -qq
  sudo apt-get satisfy -y --no-install-recommends "${dependencies[@]}"
}

main() (
  local browser_dir browser directory
  browser_dir="$(mise where http:chrome-for-testing)/chrome-linux64"
  browser="$browser_dir/chrome"
  if [[ ! -x "$browser" ]]; then
    echo "Pinned Chrome executable is missing: $browser" >&2
    return 1
  fi
  directory=$(mktemp -d)
  trap 'rm -rf "$directory"' EXIT
  if ! probe_chrome_runtime "$browser" "$directory"; then
    echo 'Pinned Chrome could not render a local page; installing its native dependencies.'
    install_chrome_dependencies "$browser_dir" || return 1
    if ! probe_chrome_runtime "$browser" "$directory"; then
      cat "$directory/chrome.log" >&2
      echo 'Pinned Chrome still cannot render a local page after installation.' >&2
      return 1
    fi
  fi
  "$browser" --version
  printf 'AGGR_CHROME_BINARY=%s\n' "$browser" >> "$GITHUB_ENV"
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main
fi
