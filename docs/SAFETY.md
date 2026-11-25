# Safety and limitations

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

