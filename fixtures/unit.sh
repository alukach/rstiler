#!/usr/bin/env bash
# The dependency-free modules self-test without a wasm toolchain or a server.
#
# `cargo test` cannot run these: the crate is a cdylib against `worker`, which
# only builds for wasm32, and there is no test harness for that target. These
# modules deliberately depend on nothing, so rustc can build each as its own
# test binary.
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
for m in tiling colormap query; do
  if ! out=$(rustc --test "src/$m.rs" -o "/tmp/unit_$m" 2>&1); then
    printf "  FAIL  %-9s did not compile\n%s\n" "$m" "$out"; fail=1; continue
  fi
  if result=$("/tmp/unit_$m" 2>&1 | grep "test result"); then
    printf "  %-9s %s\n" "$m" "$result"
    [[ "$result" == *"0 failed"* ]] || fail=1
  else
    printf "  FAIL  %-9s no result\n" "$m"; fail=1
  fi
done
exit $fail
