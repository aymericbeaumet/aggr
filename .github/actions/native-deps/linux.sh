#!/usr/bin/env bash
set -euo pipefail

libclang_candidates() {
  if [[ -n "${LIBCLANG_PATH:-}" ]]; then
    if [[ -d "$LIBCLANG_PATH" ]]; then
      printf '%s\n' "$LIBCLANG_PATH"/libclang*.so*
    else
      printf '%s\n' "$LIBCLANG_PATH"
    fi
  else
    ldconfig -p | awk '$1 ~ /^libclang(-[0-9]+)?\.so/ { print $NF }' | sort -Vr
  fi
}

find_usable_libclang() {
  local probe=$1 library
  while IFS= read -r library; do
    if [[ -f "$library" ]] && "$probe" "$library"; then
      printf '%s\n' "$library"
      return 0
    fi
  done < <(libclang_candidates)
  return 1
}

probe_native_sdk() {
  local directory=$1
  make --version >/dev/null || return 1
  cc -x c -o "$directory/c-check" - <<'C' || return 1
int main(void) { return 0; }
C
  "$directory/c-check" || return 1
  c++ -x c++ -o "$directory/cpp-check" - <<'CXX' || return 1
#include <vector>
int main() { std::vector<int> values(1, 0); return values.front(); }
CXX
  "$directory/cpp-check" || return 1
  cc -x c -o "$directory/libclang-check" - -ldl <<'C' || return 1
#include <dlfcn.h>
int main(int argc, char **argv) {
  if (argc != 2) return 1;
  void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (!library) return 1;
  void *(*create)(int, int) = (void *(*)(int, int))dlsym(library, "clang_createIndex");
  void (*dispose)(void *) = (void (*)(void *))dlsym(library, "clang_disposeIndex");
  if (!create || !dispose || !dlsym(library, "clang_getClangVersion")) return 1;
  void *index = create(0, 0);
  if (!index) return 1;
  dispose(index);
  dlclose(library);
  return 0;
}
C
  find_usable_libclang "$directory/libclang-check"
}

install_native_dependencies() {
  sudo apt-get update -qq
  sudo apt-get install -y --no-install-recommends libclang-dev build-essential
}

main() (
  local directory libclang
  directory=$(mktemp -d)
  trap 'rm -rf "$directory"' EXIT
  if ! libclang=$(probe_native_sdk "$directory" 2>/dev/null); then
    echo 'Native SDK preflight failed; installing compiler support and libclang.'
    install_native_dependencies || return 1
    if ! libclang=$(probe_native_sdk "$directory"); then
      echo 'C/C++ compilation or libclang loading still fails after installation.' >&2
      return 1
    fi
  fi
  printf 'LIBCLANG_PATH=%s\n' "$libclang" >> "$GITHUB_ENV"
  printf 'Using verified native SDK and %s\n' "$libclang"
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main
fi
