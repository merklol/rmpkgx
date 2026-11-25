# rmpkgx

**Remove apps. Leave no traces.**

An offline Rust terminal utility for inspecting and uninstalling macOS applications,
reviewing associated data and restoring eligible objects from Trash.

**v0.1.0 is an early release candidate for Apple Silicon.** No release has been
published. Intel is outside this release's support scope. The tool is not an
exhaustively audited uninstaller: review each cleanup plan and keep backups.

macOS has no universal record of every file an app creates. The tagline is a goal,
not a promise that every leftover can be discovered or removed.

## Features

- Application discovery with Info.plist metadata, versions, bundle IDs and paths.
- Conservative scanning of ten standard user Library locations.
- Confidence, ownership evidence, shared-data flags, logical sizes and inspection errors.
- Keyboard-driven Ratatui interface with immediate search and cancellable background scans.
- Reviewed native Trash moves, private operation history and eligible restoration.
- Read-only possible-leftover reports and LaunchAgent inspection.
- CLI text and JSON reports using the same engine as the TUI.
- One executable; no network service, root requirement or automatic privilege escalation.

```text
rmpkgx  v0.1.0
Remove apps. Leave no traces.

Installed applications
❯ <discovered application>  <version>  <absolute bundle path>

↑ ↓ navigate   Enter inspect   F2 history   F4 leftovers   F1 help
› Search applications…
```

The interface uses actual filesystem data. The row above is a layout placeholder.

## Install from source

Use a current stable Rust toolchain on an Apple Silicon Mac:

```sh
cargo build --locked --release --target aarch64-apple-darwin
./target/aarch64-apple-darwin/release/rmpkgx
```

For installation into Cargo's executable directory:

```sh
cargo install --locked --path .
rmpkgx --version
```

To update, obtain the desired reviewed source version and repeat the installation.
No repository URL is supplied until a repository has been published.

## Usage

```sh
rmpkgx                         # Open the TUI
rmpkgx list
rmpkgx list --json
rmpkgx scan Spotify
rmpkgx scan /Applications/Spotify.app --json
rmpkgx remove Spotify --dry-run
rmpkgx remove Spotify          # Interactive review and explicit /confirm
rmpkgx analyze --json          # Bundle sizes, not associated-data totals
rmpkgx orphans --json          # Read-only possible leftovers
rmpkgx history
rmpkgx history JOURNAL_ID --json
rmpkgx recover JOURNAL_ID --json # Fresh read-only data scan using saved app metadata
```

Names must match exactly; a bundle ID or absolute path also works. Multiple matching
installations require disambiguation by path. Read-only CLI commands work through
pipes without terminal rendering. Actual removal requires an interactive terminal;
there is no `--yes` bypass. The CLI requires `/confirm` and, for uncertain ownership,
each exact path. JSON includes inspection problems. Exit status is nonzero on errors;
invalid command arguments use Clap's usage error status.

## Keyboard controls

| Screen | Controls |
| --- | --- |
| Applications | Type to search; ↑/↓ navigate; Enter inspect; Esc clear search |
| Associated files | ↑/↓ navigate; Space select; → focus actions; Tab toggle errors; Esc back |
| Action menu | ↑/↓ choose Back or Review cleanup; Enter open choice; ←/Esc return to files |
| Ownership question | Y include / N keep; Enter apply to this path; Esc cancel review |
| Final cleanup or restore review | Y/N answer; Enter confirm; ↑/↓ scroll; Esc cancel |
| History | F2 from applications; ↑/↓ choose journal; Enter inspect; F2/Esc return |
| Journal details | Enter re-scan eligible failed/skipped data; F3 prepare restoration; Esc back |
| Restore list | Space select; R review restore; F5 recheck and clear selection; Esc history |
| Restore access help | S open System Settings; F5 recheck; Esc history |
| Possible leftovers | F4 from applications; Esc cancel scan; ↑/↓ scroll report; Enter/Esc return |
| Results | ↑/↓ scroll; Enter/Esc return |

F1 shows current controls. Ctrl+C exits and requests scan cancellation. During a
confirmed native move, exit waits for the operation result. All confirmation answers
default to **No**. Selection, opening review and accepting uncertain ownership do
not move files. Final confirmation is always separate.

Typing immediately filters the main list. Ordinary letters, including `q` and `r`,
remain search text there. The associated-files action menu is vertical and has no
button backgrounds. Older slash commands and the details-screen R shortcut remain
hidden compatibility aliases; the TUI advertises applicable keyboard controls only.

## Safety and permissions

Cleanup creates an immutable review bound to open descriptors and native object
references. Paths, application identity and association evidence are revalidated
before moving. System/protected paths, symlinks, traversal, overlapping targets,
shared data and observed running apps are blocked. Process-inspection failure also
blocks execution; a negative result is only a snapshot and the app can restart.
Multiple installed copies with the same bundle ID block selected-data cleanup.

High-confidence nonshared data may start selected. Medium/Low findings start
unselected and require individual ownership acceptance when manually selected.
Approval never upgrades the evidence or makes a shared path removable.

Confirmed items move to macOS Trash. There is no permanent deletion, automatic
sudo, shell-built path command, cross-volume copy/delete fallback or automatic
rollback. A failure stops further moves and reports every skipped item. A move
whose identity cannot be verified is reported separately; inspect Trash before
retrying. A zero count of retained findings never means all leftovers were found.
The final review lists inspection problems as well as the objects to move.

Run as your normal user. macOS permissions and privacy restrictions may deny access,
including `OSStatus -5000`. Full Disk Access is broad permission and is not a
guaranteed fix. The restore access-help screen can explicitly open System Settings;
it never grants permissions or resumes a restore automatically. Do not change ACLs,
disable protections or elevate privileges to force cleanup.

## History, recovery and restore

Confirmed cleanup writes private append-only JSONL journals under
`~/Library/Application Support/rmpkgx/history/` (directory 0700, files 0600).
Intent is written synchronously before each move and the outcome afterward.
Journal creation failure blocks all moves; later write failure stops further moves.
An unresolved intent means an **unknown outcome**, not permission to retry.
Scanning, dry runs and cancelling a review create no cleanup journal.

F2 → journal → Enter can prepare a fresh **data-only** recovery plan for eligible
recorded failed/skipped data after a verified bundle move. Every finding starts
unselected and each selected path needs fresh approval. Reinstalled applications
block execution. Moved, unverified and ambiguous paths are excluded.
CLI `recover` is a broader read-only scan, never authorization to move files.

F2 → journal → F3 opens restoration. Space selects eligible objects, R opens the
review, then Y and Enter confirm. Existing destinations, including dangling
symlinks, are never overwritten or merged. Original parent directories must exist.
Only verified moved objects with a recorded destination and matching stored identity
can be restored; older records without identity require Finder. Current restore
support is limited to the user's `~/.Trash`, not external-volume Trash.
The application list refreshes after restoration.

Journals contain private paths, have a 16 MiB read limit, are not rotated automatically
and are not cryptographically authenticated. File moves and journal writes are not
a single transaction; process termination or power loss can leave ambiguous outcomes.

## Association and possible leftovers

Container ownership uses bounded XML/binary reads of
`.com.apple.containermanagerd.metadata.plist`. Exact bundle ID plus private class 2
provides High confidence; conflicting metadata rejects filename association.
Missing/malformed metadata never silently promotes a name match. This is an observed
ContainerManager schema, not a stable public API or proof of exclusive ownership.
Group and unknown-class containers stay shared/read-only. Signed-entitlement mapping
for [App Groups](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.application-groups)
is not implemented.

LaunchAgent inspection uses `Program` or, when absent, `ProgramArguments[0]`.
Only a verified literal executable inside the bundle supplies High evidence.
Wrappers, shell expansion, PATH and BundleProgram service context are not inferred.
All LaunchAgents remain **inspect-only**; no launchctl, service unload or process kill
is performed. See [Apple's launchd manual](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5).

`orphans` / F4 lists **Likely** private-container candidates and **Unknown**
filename/shared/incomplete-catalog candidates. Each includes evidence and size
completeness. Installed identifiers and parent/helper relationships are excluded,
as are Apple identifiers. Arbitrary names such as `Godot` are not treated as orphans.
**Verified** is reserved for future provenance checks and is never emitted today.
All candidates are read-only: an undiscovered owner may live on another volume or
be a helper, extension or command-line tool. Inaccessible paths are reported separately.

## Limitations and testing

Discovery uses standard application roots, at most five directory levels, and skips
nested bundles. Data scanning checks immediate children of standard user Library
roots and measures matched trees. It does not inspect arbitrary disk locations.
Sizes are logical bytes, may count hard links separately, and do not guarantee
reclaimable space. There is no scan cache.

Native moves use deprecated CoreServices FSRef APIs. They pin object identity but
do not freeze recursive contents, audit signatures or make a multi-item cleanup
atomic. No path-based mutation fallback is enabled. See
[Apple's FSRef documentation](https://developer.apple.com/documentation/coreservices/fsref).

Automated mutation tests use temporary fixtures only. They exercise replacement
races, collisions, restore renaming, journal failures and partial-result reporting;
they do not exercise the actual system Trash entry point or all macOS privacy
restrictions. Local release checks ran on macOS 26.6.2 arm64. Older macOS versions
have not been qualified; GitHub workflows have been prepared but not run remotely.
The candidate has no Developer ID signature or notarization.

## Development and local release packaging

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
./scripts/package-release.sh
```

Packaging writes a versioned Apple Silicon archive and adjacent SHA-256 file under
`dist/`, including the executable, README, MIT license, changelog and release guide. It does not
publish anything. See [release preparation](docs/RELEASING.md) and [changelog](CHANGELOG.md).

## Homebrew tap preparation

Homebrew distribution uses a **source formula**, not a cask or downloaded executable.
A paid Apple Developer membership, Developer ID and notarization are not required
for this route. Cargo dependencies are fetched before an offline locked build;
the installed utility runs offline. Current Homebrew, Xcode Command Line Tools and
an Apple Silicon Mac are required. Rust is a declared build dependency.

```sh
python3 scripts/test-prepare-homebrew.py
python3 scripts/prepare-homebrew.py --owner YOUR_GITHUB_LOGIN
```

Python 3.11+ is needed only for release preparation. This produces a deterministic
source archive and SHA-256 plus a standalone `homebrew-tap/` directory under
`dist/homebrew/`. The generated formula references the exact uploaded release asset
and includes its checksum. It never uses the prebuilt executable checksum or
GitHub's automatically generated source archive.

Follow [the publication guide](docs/HOMEBREW.md) to upload the source and tap yourself.
Nothing is uploaded automatically. Once published, installation is:

```sh
brew tap YOUR_GITHUB_LOGIN/tap
# Current Homebrew may require explicit trust of this one formula:
brew trust --formula YOUR_GITHUB_LOGIN/tap/rmpkgx
brew install --build-from-source YOUR_GITHUB_LOGIN/tap/rmpkgx
brew test YOUR_GITHUB_LOGIN/tap/rmpkgx
```

Use `brew update && brew upgrade YOUR_GITHUB_LOGIN/tap/rmpkgx` for updates.
These are future installation commands until both repositories and the source
release asset exist. No Homebrew Core acceptance or prebuilt bottles are claimed.
See [Homebrew's tap guide](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
and [Rust formula guidance](https://docs.brew.sh/Language-Specific-Formulae#rust).
