#!/usr/bin/env bash
# Regenerates the committed mcs-compiled test fixtures. Requires Mono's mcs.
set -euo pipefail
cd "$(dirname "$0")/.."
fixtures=crates/metadata/tests/fixtures
mcs -out:"$fixtures/hello.exe" "$fixtures/src/hello.cs"
mcs -target:library -out:"$fixtures/lib.dll" "$fixtures/src/lib.cs"
echo "fixtures regenerated in $fixtures"
