# rmpkgx

**Remove apps. Leave no traces.**

An offline terminal uninstaller for **macOS on Apple Silicon**. Find applications,
review associated data, and move approved items to Trash.

- Search as you type and review cleanup before confirming.
- Inspect sizes, ownership evidence and possible leftovers.
- View cleanup history and restore eligible items from Trash.

## Install

With Homebrew and Xcode Command Line Tools installed:

```sh
brew tap merklol/rmpkgx https://github.com/merklol/rmpkgx.git
brew trust --formula merklol/rmpkgx/rmpkgx
brew install --build-from-source merklol/rmpkgx/rmpkgx
```

Rust is installed as a build dependency. The application runs offline.

## Use

```sh
rmpkgx                         # Interactive interface
rmpkgx list --json              # Installed applications
rmpkgx scan Spotify             # Inspect associated data
rmpkgx remove Spotify --dry-run # Preview without moving files
rmpkgx orphans                  # Possible leftovers (read-only)
```

| Key | Action |
| --- | --- |
| Type / ↑↓ / Enter | Search, navigate, inspect |
| Space | Select a data path |
| → / ↑↓ / Enter | Focus the action menu, choose, open review |
| Y/N, then Enter | Answer and confirm a review question |
| Esc | Clear input, go back or cancel a scan |
| F2 / F4 | History / possible leftovers |
| Ctrl+C | Quit; an active confirmed move must finish first |

Restore: **F2 → journal → F3**, select with **Space**, then **R** to review.
Use **F1** for controls on the current screen.

## Safety

Early release: review paths and inspection problems before confirming. No permanent
deletion or automatic sudo. Shared data and LaunchAgents are inspect-only; orphan
candidates cannot be deleted. Restoration requires an eligible recorded object.

macOS has no universal record of files created by an app. Some locations may be
inaccessible, and not every leftover can be found. [Safety and limitations](docs/SAFETY.md).

## License

[MIT](LICENSE) · [Changelog](CHANGELOG.md) · [Build and release guide](docs/RELEASING.md)
