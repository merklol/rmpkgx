# Preparing a release candidate

This workflow builds local artifacts only. Do not tag, push, create a repository,
create a GitHub release or upload artifacts without explicit authorization.

## Scope

- Package/executable: `rmpkgx`, version `0.1.0`, MIT.
- Release target: `aarch64-apple-darwin` (native Apple Silicon Mac).
- Intel artifacts are intentionally omitted; do not claim Intel qualification.
- GitHub jobs use `macos-15`, the arm64 runner in the current
  [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
- The Homebrew source-formula route requires no Developer ID or notarization.
  Direct-download binaries are a separate distribution route; do not imply they
  have passed Gatekeeper qualification or bypass quarantine automatically.

## Build and verify locally

From the repository root:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./scripts/package-release.sh

cd dist
shasum -a 256 -c rmpkgx-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar -tzf rmpkgx-v0.1.0-aarch64-apple-darwin.tar.gz
```

The script builds with Cargo.lock, verifies the executable's version and arm64
architecture, stages only the executable and four documentation files and packages them under a versioned directory.
The checksum contains a relative filename and works after the pair is copied.
The archive is not claimed to be byte-for-byte reproducible: timestamps and local
build/toolchain inputs can change it. Every rebuild needs a fresh checksum.

Extract into a temporary directory, run `rmpkgx --version` and `rmpkgx --help`, and
check `file`/`lipo -archs` for arm64. Verify the MIT license, changelog, README and release guide are
present. Only install the executable after reviewing the candidate.

## Runtime qualification still required before public release

Use disposable application/data fixtures and record the macOS version:

- Exercise actual system Trash movement, partial permission failure and journaled
  restoration; existing automated moves are confined to temporary directories.
- Check allowed and denied Trash/privacy access with the intended host terminal.
- Confirm restore never overwrites and catalog refresh makes restored apps visible.
- Check final confirmation defaults to No and cancellation never moves files.
- Visually review file/action-menu focus, long paths and reports in a small terminal.
- Qualify each older macOS version before claiming support for it.

Local unit tests and PTY smoke tests are useful evidence, not a replacement for
real system-Trash and visual acceptance checks. Do not call uninspected data clean.

## Prepared GitHub workflow (not executed locally)

CI checks formatting, Clippy, tests and the Apple Silicon release build. The release
candidate job repeats checks, uses the same packaging script, tests the extracted
executable and uploads workflow artifacts only. A future `v0.1.0` tag must match
the executable version; a mismatch fails packaging. `workflow_dispatch` also creates
candidates without publishing a release. Permissions are read-only for repository
contents; there is no release-publishing action.

Before any authorized publication, run `python3 scripts/prepare-homebrew.py --owner YOUR_LOGIN`.
Use the generated source asset and tap formula together; its checksum is filled
automatically. Review all release notes and known limitations. Publishing remains manual. See [Homebrew publication](HOMEBREW.md): the source formula does not require Developer ID signing or notarization.
