#!/usr/bin/env python3
"""Generate source asset + standalone tap, without network, git or publishing.

Requires Python 3.11+. Archives normalize timestamps and ownership so the same
source tree gives the same checksum. Cargo dependencies are not bundled.
"""
import argparse
import gzip
import hashlib
import io
import pathlib
import re
import tarfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE_PATHS = (
    "Cargo.toml", "Cargo.lock", "LICENSE", "README.md", "CHANGELOG.md",
    "src", "tests", "scripts", "docs", "packaging", ".github", ".gitignore",
)


def source_files(root):
    """Explicit allowlist: no target, dist, git history, credentials or local state."""
    files = []
    for relative in SOURCE_PATHS:
        path = root / relative
        candidates = [path] if path.is_file() else sorted(path.rglob("*"))
        if path.is_symlink():
            raise ValueError(f"Source symlink rejected: {path}")
        for candidate in candidates:
            if candidate.is_symlink():
                raise ValueError(f"Source symlink rejected: {candidate}")
            if candidate.is_file() and "__pycache__" not in candidate.parts:
                files.append(candidate)
    for required in ("Cargo.toml", "Cargo.lock", "LICENSE", "src/main.rs"):
        if root / required not in files:
            raise ValueError(f"Missing required source file: {required}")
    return sorted(set(files))


def create_archive(root, archive, version):
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w") as package:
                for path in source_files(root):
                    data = path.read_bytes()
                    info = tarfile.TarInfo(f"rmpkgx-{version}/{path.relative_to(root).as_posix()}")
                    info.size = len(data)
                    info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                    info.uid = info.gid = info.mtime = 0
                    package.addfile(info, io.BytesIO(data))
    return hashlib.sha256(archive.read_bytes()).hexdigest()


def prepare(owner, output, root=ROOT):
    root = root.resolve()
    if not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?", owner):
        raise ValueError("Use a GitHub owner login or organization, without a URL")
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("This release generator requires an x.y.z version")
    output = output.resolve()
    # Never generate into directories that will be included in the source asset.
    for relative in SOURCE_PATHS:
        source = root / relative
        if output == source or source in output.parents:
            raise ValueError("Output must be outside source inputs (default: dist/homebrew)")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"rmpkgx-v{version}-source.tar.gz"
    checksum = create_archive(root, archive, version)
    archive.with_suffix(archive.suffix + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    tap = output / "homebrew-tap"
    formula = tap / "Formula" / "rmpkgx.rb"
    formula.parent.mkdir(parents=True, exist_ok=True)
    template = (root / "packaging/rmpkgx.rb.example").read_text()
    formula.write_text(template.replace("OWNER", owner).replace("VERSION", version).replace("REPLACE_WITH_VERIFIED_SHA256", checksum))
    (tap / "LICENSE").write_bytes((root / "LICENSE").read_bytes())
    (tap / "README.md").write_text(f"""# {owner}/tap

Homebrew formula for [rmpkgx](https://github.com/{owner}/rmpkgx), macOS Apple Silicon.

Publish this directory as `{owner}/homebrew-tap` after uploading the exact source
asset referenced by `Formula/rmpkgx.rb` to release `v{version}` of `{owner}/rmpkgx`.
This directory contains no git repository and nothing has been uploaded.

## Install

```sh
brew tap {owner}/tap
# On Homebrew versions requiring explicit tap trust, trust only this formula:
brew trust --formula {owner}/tap/rmpkgx
brew install --build-from-source {owner}/tap/rmpkgx
brew test {owner}/tap/rmpkgx
```

Current Homebrew and Xcode Command Line Tools are required. Rust is a build dependency.
Dependencies are fetched before the offline Cargo install; the installed app runs offline.
No Developer ID certificate or notarization service is required for this source formula.

```sh
brew update
brew upgrade {owner}/tap/rmpkgx
brew uninstall {owner}/tap/rmpkgx
```

Updates require changing the URL, version and SHA-256 in the formula to a new immutable
source asset. There are no prebuilt Homebrew bottles. This is an independent tap,
not Homebrew Core. `brew uninstall` removes rmpkgx, not the user's operation history.
""")
    workflow = tap / ".github/workflows/test.yml"
    workflow.parent.mkdir(parents=True, exist_ok=True)
    workflow.write_text(f"""name: Formula test
on: [push, pull_request, workflow_dispatch]
permissions:
  contents: read
jobs:
  test:
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - name: Install and test the source formula
        env:
          HOMEBREW_NO_AUTO_UPDATE: "1"
        run: |
          brew tap {owner}/tap "$GITHUB_WORKSPACE"
          brew trust --formula {owner}/tap/rmpkgx
          brew install --build-from-source {owner}/tap/rmpkgx
          brew test {owner}/tap/rmpkgx
          rmpkgx --version
""")
    return archive, formula, checksum


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owner", required=True, help="GitHub login or organization")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "dist/homebrew")
    args = parser.parse_args()
    try:
        archive, formula, checksum = prepare(args.owner, args.output)
    except (ValueError, OSError) as error:
        parser.error(str(error))
    print(f"Source asset: {archive}\nSHA-256: {checksum}\nTap formula: {formula}\nNothing published. Upload this exact asset; do not substitute GitHub's automatic source archive.")


if __name__ == "__main__":
    main()
