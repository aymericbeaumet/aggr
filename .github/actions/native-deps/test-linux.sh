#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)" in
  Linux|Darwin) ;;
  *) printf 'Skipping Linux SDK fixtures on this host; workflow lint still runs.\n'; exit 0 ;;
esac

# shellcheck source=.github/actions/native-deps/linux.sh
source "$(dirname "${BASH_SOURCE[0]}")/linux.sh"

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/with spaces"
export GITHUB_ENV="$fixture/github-env"
fixture_library="$fixture/with spaces/libclang.so.1"

case "$(uname -s)" in
  Darwin) shared_flag=-dynamiclib ;;
  *) shared_flag=-shared ;;
esac
cc "$shared_flag" -fPIC -x c -o "$fixture/valid.so" - <<'C'
void *clang_createIndex(int a, int b) { (void)a; (void)b; return (void *)1; }
void clang_disposeIndex(void *index) { (void)index; }
void clang_getClangVersion(void) {}
C
cc "$shared_flag" -fPIC -x c -o "$fixture/invalid.so" - <<'C'
void unrelated_library(void) {}
C

libclang_candidates() { printf '%s\n' "$fixture_library"; }
install_native_dependencies() {
  touch "$fixture/installed"
  cp "$fixture/valid.so" "$fixture_library"
}

# A working SDK and a loadable libclang need no package manager or network.
cp "$fixture/valid.so" "$fixture_library"
main
test ! -e "$fixture/installed"
test "$(cat "$GITHUB_ENV")" = "LIBCLANG_PATH=$fixture_library"

# A shared library with the wrong API must not satisfy the preflight.
cp "$fixture/invalid.so" "$fixture_library"
: > "$GITHUB_ENV"
main
test -f "$fixture/installed"
test "$(cat "$GITHUB_ENV")" = "LIBCLANG_PATH=$fixture_library"

# Installing packages is not sufficient: validate the resulting SDK again.
install_native_dependencies() { :; }
printf 'not a shared library\n' > "$fixture_library"
: > "$GITHUB_ENV"
if main; then
  echo 'Expected an unusable SDK to fail after installation' >&2
  exit 1
fi
test ! -s "$GITHUB_ENV"

# A loadable library cannot hide a compiler or standard-library failure.
cp "$fixture/valid.so" "$fixture_library"
c++() { return 1; }
if main; then
  echo 'Expected broken C++ compilation to fail the SDK check' >&2
  exit 1
fi
test ! -s "$GITHUB_ENV"
printf 'Native SDK preflight contracts passed\n'
