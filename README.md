# gist-tools

A terminal UI for managing local markdown files synced to GitHub Gists.

## Features

- **Tree-based file browser** for one or more local writing directories, arbitrarily nested
- **Push / pull / delete** local markdown files to/from gists; per-file sync status with at-a-glance counts in the status bar
- **Diff view** of local vs remote
- **Open gist in browser** (`o`) and **copy gist URL** (`c`) for quick sharing
- **Jump to next/previous dirty file** (`n` / `N`) for triage
- **Hydration** — match existing gists to local files by filename + content hash, with an interactive resolver for ambiguous cases
- **Multi-root** — switch between several configured directories from inside the app
- **Search / filter** the file tree as you type
- **Markdown preview** with syntax highlighting (headings, code blocks, inline code, bold, italic, lists, blockquotes)
- **Google Doc import** — fetch a public Google Doc and save it as markdown under any tree
- **Atomic on-disk state** with retry/backoff and rate-limit awareness for the GitHub API

## Quick Start

### Prerequisites

- Rust (edition 2024 toolchain — `rustup update stable` if you're behind)
- A GitHub token with `gist` scope, exposed via `$GITHUB_TOKEN` or `gh auth token`

### Install

```bash
git clone git@github.com:jhheider/gist-tools.git
cd gist-tools
cargo build --release
./target/release/wm
```

### First run

If no roots are configured, the app opens a setup dialog. Type a path to a directory of markdown files (`~` is expanded) and press **Enter**. From there, all writings under that directory are scanned recursively.

State is persisted under your platform data dir (`~/Library/Application Support/writings-manager` on macOS, `~/.local/share/writings-manager` on Linux):

- `config.json` — list of configured roots
- `store.json` — per-(root, file) gist mappings

Tokens are **not** persisted by this tool — they're resolved fresh on each launch.

## Usage

### Keybindings

| Key | Mode | Action |
|---|---|---|
| `Tab` | Normal | Toggle focus: tree pane ↔ preview/diff pane |
| `j/k` `↓/↑` | Normal | Navigate the focused pane |
| `Enter` `l` `→` | Normal (tree) | Expand directory / select file |
| `h` `←` `Bksp` | Normal (tree) | Collapse directory |
| `PgUp/PgDn` | Normal, Diff | Scroll preview/diff pane |
| `n` / `N` | Normal | Jump to next / previous non-synced file |
| `u` | Normal | Push selected file (create or update gist) |
| `d` | Normal | Pull remote into selected file (with confirmation) |
| `D` | Normal | Diff local vs remote |
| `c` | Normal | Copy gist URL to clipboard (auto-pushes if not yet gisted) |
| `o` | Normal | Open gist URL in the system browser |
| `e` | Normal | Edit selected file in `$EDITOR` (TUI suspends, then refreshes) |
| `X` | Normal | Delete remote gist (with confirmation; keeps local file) |
| `H` | Normal | Hydrate — match existing gists to files |
| `I` | Normal | Import a Google Doc as markdown |
| `R` | Normal | Switch root directory |
| `r` | Normal | Refresh the tree |
| `/` | Normal | Search / filter |
| `?` | Normal | Help |
| `q` | Normal | Quit |
| `j/k` `↑/↓` `PgUp/PgDn` | Diff | Scroll the diff |
| `j/k` `Enter` `a` `d` `Esc` | Root switcher | Navigate / switch / add / delete root / close |
| `j/k` `Enter` `s` `Esc` | Ambiguous resolver | Navigate candidates / pick / skip / abort |
| `y` `n` | Confirm dialog | Confirm / dismiss |
| `Esc` | Most modes | Cancel and return to Normal |

### Mouse

By default, mouse capture is **off** so terminal-native features (cmd-click on URLs, native text selection, the terminal's own scrollback) keep working. Set `WM_MOUSE=1` in the environment to enable wheel-scroll routing into the panes — useful if you don't need cmd-click and prefer wheel scrolling over PgUp/PgDn.

### Status icons

Each file in the tree carries a sync-state icon. By default these are emoji; set `WM_NO_EMOJI=1` (or run under a `TERM` of `dumb`/`linux`/`vt100`/`vt220`) to fall back to ASCII.

| Emoji | ASCII | Meaning |
|---|---|---|
| ✅ | `[=]` | Synced |
| ⬆️ | `[^]` | Local newer (push to update) |
| ⬇️ | `[v]` | Remote newer (pull to update) |
| ❗ | `[!]` | Conflict — both diverged |
| ⚪ | `[ ]` | Not yet mapped to a gist |

### Hydration

Press `H` to match your existing gists to local files. Three phases:

1. List all your gists (paginated, with retry/backoff and rate-limit handling).
2. Auto-map files where filename + content hash give a unique match.
3. For files where multiple gists share the same filename and no content match exists, the **ambiguous resolver** prompts you to pick one (or skip).

Hydration runs in the background; concurrent push/pull is preserved (results merge rather than reload-from-disk).

## Architecture

Two-crate Cargo workspace:

- **`crates/gist-rs`** — Standalone GitHub Gist client. Auth via `$GITHUB_TOKEN` or `gh auth token`. Idempotent GET retries with exponential backoff and `Retry-After` / `X-RateLimit-Reset` handling. Pagination via `Link` headers.
- **`crates/wm`** — The TUI. Modes for normal navigation, search, diff, confirm, hydration progress, ambiguous-match resolution, root switcher, setup, and Google Doc import. State persistence via atomic temp-file + rename.

## Development

```bash
cargo build           # build
cargo test            # run unit tests
cargo clippy          # lint
cargo fmt             # format (always run before committing)
```

The store format is versioned; `Store::load` will migrate v1 → v2 in place on first launch and persist the new format.

## License

MIT.
