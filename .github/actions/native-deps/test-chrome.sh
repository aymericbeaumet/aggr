#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)" in
  Linux|Darwin) ;;
  *) printf 'Skipping Linux Chrome runtime fixtures on this host.\n'; exit 0 ;;
esac

# shellcheck source=.github/actions/native-deps/chrome.sh
source "$(dirname "${BASH_SOURCE[0]}")/chrome.sh"

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export GITHUB_ENV="$fixture/github-env"
export CHROME_FIXTURE="$fixture"
mkdir -p "$fixture/pinned browser/chrome-linux64"
chrome="$fixture/pinned browser/chrome-linux64/chrome"
cat > "$chrome" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --version ]]; then echo 'pinned Chrome'; exit; fi
[[ ! -e "$CHROME_FIXTURE/broken" ]] || exit 1
printf '%s\n' "$@" > "$CHROME_FIXTURE/arguments"
printf '<html><head><title>aggr-runtime-check</title></head><body></body></html>\n'
BASH
chmod +x "$chrome"
mise() { printf '%s\n' "$fixture/pinned browser"; }
timeout() { shift 2; "$@"; }
install_chrome_dependencies() {
  touch "$fixture/installed"
  rm "$fixture/broken"
}

main
test ! -e "$fixture/installed"
test "$(cat "$GITHUB_ENV")" = "AGGR_CHROME_BINARY=$chrome"
grep -qx -- '--headless=new' "$fixture/arguments"
grep -qx -- '--dump-dom' "$fixture/arguments"

# A successful --version cannot conceal missing runtime dependencies.
touch "$fixture/broken"
: > "$GITHUB_ENV"
main
test -e "$fixture/installed"
test "$(cat "$GITHUB_ENV")" = "AGGR_CHROME_BINARY=$chrome"

# Failed repair must not advertise a usable browser to later steps.
install_chrome_dependencies() { :; }
touch "$fixture/broken"
: > "$GITHUB_ENV"
if main; then
  echo 'Expected a broken headless browser to fail after installation' >&2
  exit 1
fi
test ! -s "$GITHUB_ENV"
printf 'Chrome runtime preflight contracts passed\n'
