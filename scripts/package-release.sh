#!/bin/sh
# Build a local candidate only. Never tags, publishes, uploads or signs releases.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo 'Release packaging requires a native Apple Silicon Mac.' >&2
    exit 1
fi
target=aarch64-apple-darwin
cargo build --locked --release --target "$target"
binary="target/$target/release/rmpkgx"
version=$("$binary" --version)
version=${version#rmpkgx }
case "$version" in
    ''|*[!0-9.]*) echo 'Unexpected executable version.' >&2; exit 1 ;;
esac
if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ "${GITHUB_REF_NAME:-}" != "v$version" ]; then
    echo 'Git tag and executable version differ; refusing to package.' >&2
    exit 1
fi
if [ "$(lipo -archs "$binary")" != arm64 ]; then
    echo 'Expected a single-architecture arm64 executable.' >&2
    exit 1
fi
name="rmpkgx-v$version-$target"
stage=$(mktemp -d "${TMPDIR:-/tmp}/rmpkgx-release.XXXXXX")
trap 'rm -rf "$stage"' EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$stage/$name" dist
cp "$binary" "$stage/$name/rmpkgx"
cp README.md LICENSE CHANGELOG.md "$stage/$name/"
mkdir "$stage/$name/docs"
cp docs/RELEASING.md "$stage/$name/docs/"
COPYFILE_DISABLE=1 tar -czf "dist/$name.tar.gz" -C "$stage" "$name"
(cd dist && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256" && shasum -a 256 -c "$name.tar.gz.sha256")
printf 'Local candidate: dist/%s.tar.gz\n' "$name"
