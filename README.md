# gist-tools

A terminal UI for managing GitHub Gists with local sync, search, and diff capabilities.

## Features

- **Browse gists** — tree-based navigation of local gist files
- **Search & filter** — fast in-memory search across gists
- **Diff viewer** — compare local vs remote versions
- **Sync** — pull gists from GitHub, push local changes
- **Google Docs integration** — paste Google Docs URLs to hydrate content
- **Persistent state** — config and sync history stored locally

## Quick Start

### Prerequisites

- Rust 1.70+
- GitHub account + personal access token
- Git

### Installation

```bash
git clone https://github.com/jhheider/gist-tools.git
cd gist-tools
cargo build --release
./target/release/wm
```

### Configuration

On first run, the app will:
1. Prompt for a root directory (defaults to `~/Dropbox/Personal/RP/shared` if it exists)
2. Resolve your GitHub token via `gh auth` or environment (`GITHUB_TOKEN`)
3. Create a `.gist-tools/config.json` in the root directory

**Config file** (`~/.gist-tools/config.json`):
```json
{
  "root": "/path/to/gists",
  "github_token": "ghp_..."
}
```

## Usage

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j/k` or `↓/↑` | Navigate tree |
| `Enter` | Select/view file |
| `?` | Help |
| `/` | Search |
| `d` | Diff (local vs remote) |
| `s` | Sync down (fetch remote) |
| `c` | Confirm action (in dialogs) |
| `q` | Quit |

### Workflows

**Fetch all gists:**
```
1. Start app
2. Select "Sync" or press `s`
3. Confirm with `c`
4. App hydrates gists from GitHub
```

**Search for a gist:**
```
1. Press `/`
2. Type search term
3. Results filter in real-time
4. Press Enter to select
```

**View diff:**
```
1. Navigate to a file
2. Press `d` to show local vs remote diff
3. Review changes
```

## Architecture

### Workspace

- **gist-rs** — GitHub Gist API client + auth
  - `client.rs` — reqwest-based HTTP client
  - `auth.rs` — GitHub token resolution
  - `types.rs` — Gist data structures

- **wm** — Terminal UI application
  - `app.rs` — state machine + event handling
  - `ui/` — ratatui rendering components
  - `sync.rs` — sync logic
  - `hydrate.rs` — Google Docs → markdown conversion
  - `store.rs` — local cache/state persistence

## Development

### Build

```bash
cargo build
```

### Test

```bash
cargo test
```

### Format & Lint

```bash
cargo fmt
cargo clippy
```

## Known Issues

- **Slow hydration**: Large gist collections (500+) lack progress indicator. Restart needed to populate tree after hydration completes.
- **Config path**: Hardcoded Dropbox path breaks on systems without Dropbox. Use env var or manual config edit as workaround.
- **No multi-directory support**: Single root directory only. Future enhancement planned.

## Roadmap

- [ ] Progress indicator during hydration
- [ ] Directory picker on startup
- [ ] Multi-directory / multi-root support
- [ ] Color themes + emoji indicators
- [ ] Gist creation/deletion UI
- [ ] Markdown preview pane
- [ ] Config file wizard

## License

MIT

## Contributing

Bug reports and feature requests welcome. Open an issue on GitHub.

---

**v0.1.0** — Initial release. Functional core with rough edges.
