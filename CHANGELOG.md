# Changelog

## 0.1.0 — local release candidate (not published)

First candidate for macOS on Apple Silicon (`aarch64-apple-darwin`).

- Discover applications, read bundle metadata and distinguish installations by path.
- Inspect associated Library data with confidence, ownership evidence and logical sizes.
- Search applications as you type; review files and cleanup through a keyboard-driven TUI.
- Preview cleanup, review uncertain ownership and explicitly confirm native Trash moves.
- Record cleanup outcomes and partial failures in private journals; re-scan retained data.
- Restore eligible journaled objects from Trash without overwriting existing files.
- Explain denied access and offer an explicit shortcut to System Settings.
- Inspect container metadata and LaunchAgent executable references conservatively.
- Show possible orphaned data as read-only Likely/Unknown candidates (F4 or `orphans`).
- Provide CLI JSON for read-only inspection and dry-run removal previews.

### Known limits

- Discovery cannot identify every file created by an application.
- Group/shared data and LaunchAgents cannot be removed; orphan candidates are read-only.
- Native filesystem APIs are deprecated; real Trash and privacy behavior need qualification
  across macOS versions. Automated mutation tests use temporary fixtures only.
- Restore is limited to eligible recorded objects in the current user's `~/.Trash`.
- No Intel release artifact, Developer ID signing, notarization or published Homebrew tap.
