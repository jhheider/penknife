# penknife

Git-style remotes for your documents.

Point penknife at a folder of markdown (an Obsidian vault, a notes directory, anywhere you write) and publish files to GitHub Gists. It tracks every published copy and shows you, per file, whether it is current, ahead, behind, or diverged. More backends are planned; see [ABOUT.md](ABOUT.md) for the roadmap.

Where it fits: your editor is where you write. A git remote or sync service is how you back up. penknife is how you share, and how you know your shares haven't gone stale.

## Features

- Tree-based browser for one or more local writing directories, arbitrarily nested. Supports `.md` and `.json` (with pretty-printed, syntax-highlighted JSON preview)
- Push, pull, and delete against gists, with per-file sync status and at-a-glance counts in the status bar. Pushes refuse to overwrite a remote that changed since the last sync (with a force option after review)
- Background polling: a cheap remote check every few minutes detects gists edited on the web or another machine, and a local sweep picks up files created, edited, or deleted outside the TUI. Icons and counts stay honest without any manual refresh; failures back off exponentially so offline sessions don't nag
- Diff view of local vs remote
- Open gist in browser (`o`) and copy gist URL (`c`) for quick sharing
- Jump to next/previous dirty file (`n` / `N`) for triage
- Hydration: existing gists are matched to local files by filename and content hash automatically at startup, with an interactive resolver (`M`) for ambiguous cases
- Multi-root: switch between several configured directories from inside the app
- Fuzzy file picker (`/`): fzf-style modal with smartcase matching and inline highlights, powered by [nucleo](https://crates.io/crates/nucleo-matcher)
- Find in files (`f`): content search across the current scope with a jump list; Enter opens the matched file
- Find and replace (`s`): recursive substring search within the current scope, per-match review checklist with line context, drift detection on apply
- Markdown preview with syntax highlighting
- Import from URL (`I`): fetch a public Google Doc as markdown, or a single-file gist (which arrives already linked and synced)
- Publish to Google Docs (`p`): markdown up-renders to a real Doc via the Drive API; re-publish replaces it. Push-only by design (the round trip is lossy), with a device-flow sign-in on first use
- Rich paste (`V`): clipboard HTML converts to markdown on the way in
- Atomic on-disk state, with retry/backoff and rate-limit awareness for the GitHub API

## Quick start

### Prerequisites

- Rust (edition 2024 toolchain; `rustup update stable` if you're behind)
- A GitHub token with `gist` scope, exposed via `$GITHUB_TOKEN` or `gh auth token`

### Install

```bash
git clone git@github.com:jhheider/penknife.git
cd penknife
cargo install --locked --path crates/penknife
penknife
```

### CLI flags

```
penknife            # launch the TUI
penknife --config   # open config.toml in $EDITOR
penknife --version  # print version
penknife --help     # print flag help
```

### First run

If no roots are configured, the app opens a setup dialog. Type a path to a directory of markdown files (`~` is expanded) and press **Enter**. All writings under that directory are scanned recursively.

State is persisted under your platform data dir (`~/Library/Application Support/penknife` on macOS, `~/.local/share/penknife` on Linux). An existing `writings-manager` data dir from earlier versions is migrated automatically on first launch.

- `config.toml`: list of configured roots plus user-defined aliases
- `store.json`: per-(root, file) gist mappings

Edit the config directly with `penknife --config` (opens `config.toml` in `$EDITOR`).
Add user aliases under `[aliases]`:

```toml
[aliases]
S = "just stats"        # quick word-count summary
A = "just ai-commit"
P = "git push"
```

Single-character keys only; keys that conflict with built-in bindings are dropped at load time with a warning. Commands run via `sh -c` with PWD set to the active root.

Tokens are **not** persisted by this tool; they're resolved fresh on each launch. (Exception: the optional Google Docs backend caches its OAuth tokens in the data dir with owner-only permissions, because the device flow is too heavy to repeat per session.)

### Publishing to Google Docs

The `p` menu publishes the selected file as a real Google Doc (and updates, opens, or deletes it thereafter). It needs an OAuth client with the Drive API enabled; until an official client ships with releases, supply your own:

```toml
[gdoc]
client_id = "....apps.googleusercontent.com"
client_secret = "..."
```

Create one in Google Cloud Console: new project, enable the Drive API, create an OAuth client of the "TV and Limited Input devices" type. Only the non-sensitive `drive.file` scope is used, so no verification review is needed. On first publish, penknife shows a short code and opens google.com/device; approve there and the publish continues. Publishing is push-only: a Doc edited on the Google side is reported, never silently pulled over your local file.

## Usage

### Keybindings

| Key | Mode | Action |
|---|---|---|
| `Tab` | Normal | Toggle focus: tree pane / preview-diff pane |
| `j/k` `↓/↑` | Normal | Navigate the focused pane |
| `Enter` `l` `→` | Normal (tree) | Expand directory / select file |
| `h` `←` `Bksp` | Normal (tree) | Collapse directory |
| `PgUp/PgDn` | Normal, Diff | Scroll preview/diff pane |
| `n` / `N` | Normal | Jump to next / previous non-synced file |
| `u` | Normal | Push selected file (create or update gist) |
| `d` | Normal | Pull remote into selected file (with confirmation) |
| `D` | Normal | Diff local vs remote |
| `c` | Normal | Copy gist URL to clipboard (auto-pushes if not yet gisted) |
| `C` | Normal | Copy selected file's contents to clipboard |
| `V` | Normal | Paste clipboard (HTML converted to markdown) as a new file |
| `o` | Normal | Open gist URL in the system browser |
| `e` | Normal | Edit selected file in `$EDITOR` (TUI suspends, then refreshes) |
| `m` | Normal | Rename / move the selected file (updates store and remote gist filename) |
| `X` | Normal | Delete menu: remote gist, local file (trash), or both (with confirmation) |
| `g` | Normal | Git menu: status / log / pull / push (when root is in a repo) |
| `p` | Normal | Publish menu: Google Docs (publish / update / open / copy URL / unpublish) |
| `f` | Normal | Find in files: content search with a jump list |
| `M` | Normal | Resolve ambiguous hydration matches (see below) |
| `L` | Normal | Link the selected file to an existing gist by URL or ID |
| `I` | Normal | Import from URL: a public Google Doc, or a gist (imports linked and synced) |
| `R` | Normal | Switch root directory |
| `q` | Normal | Quit |
| `j/k` `↑/↓` `PgUp/PgDn` | Diff | Scroll the diff |
| `j/k` `Enter` `a` `d` `Esc` | Root switcher | Navigate / switch / add / delete root / close |
| `j/k` `Enter` `s` `Esc` | Ambiguous resolver | Navigate candidates / pick / skip / abort |
| `y` `n` | Confirm dialog | Confirm / dismiss |
| `Esc` | Most modes | Cancel and return to Normal |

### Mouse

By default, mouse capture is **off** so terminal-native features (cmd-click on URLs, native text selection, the terminal's own scrollback) keep working. Set `WM_MOUSE=1` in the environment to enable mouse interaction inside the TUI: left-click selects a tree row (or focuses the right pane) and the wheel scrolls whichever pane the cursor is over.

### Status icons

Each file in the tree carries a sync-state icon followed by a git-state icon (the latter is blank for clean / not-in-repo). By default both columns use slim single-width unicode glyphs so rows stay aligned in every terminal/font combo. Set `WM_EMOJI=1` to opt into the wide emoji set, or `WM_NO_EMOJI=1` (or run under a `TERM` of `dumb`/`linux`/`vt100`/`vt220`) to fall back to pure ASCII.

| Slim (default) | Emoji | ASCII | Meaning |
|---|---|---|---|
| `✓` | ✅ | `[=]` | Synced |
| `↑` | ⬆️ | `[^]` | Local newer (push to update) |
| `↓` | ⬇️ | `[v]` | Remote newer (pull to update) |
| `!` | ❗ | `[!]` | Conflict: both diverged |
| `·` | ⚪ | `[ ]` | Not yet mapped to a gist |

### Polling

The tree keeps itself current; there are no refresh keys.

- **Remote** (default: every 5 minutes): one cheap API listing per check detects gists edited elsewhere and lights up the behind/conflict icons without pulling anything. Consecutive failures double the effective interval (capped at 64x), so an offline session stays quiet.
- **Local** (default: every 5 seconds): a filesystem sweep re-stats the tree and refreshes it only when membership or mtimes actually changed. Zero hashing in the steady state.

Tune or disable either in `config.toml` (`0` disables):

```toml
[poll]
remote_secs = 300
local_secs = 5
```

### Hydration

At startup, penknife matches your existing gists to local files automatically. Three phases:

1. List your gists (paginated, with retry/backoff and rate-limit handling).
2. Auto-map files with a unique filename match, fetching each gist's content so the recorded remote hash is real (a divergent remote hydrates as a conflict, not a fake "synced").
3. For files where multiple gists share the same filename and no content match exists, the status bar reports the count; press `M` to open the resolver and pick one (or skip).

Hydration runs in the background; concurrent push/pull is preserved (results merge rather than reload-from-disk).

**Incremental.** After a root's first full walk, the store records a per-root `last_hydrated` timestamp. Subsequent runs pass it as GitHub's `since=` filter, fetching only gists changed since the last walk instead of re-listing your whole account. The cursor advances on every successful run, and is kept per-root.

### Manual linking

Hydration only auto-pairs a local file with a gist when their filenames match. For a gist created elsewhere whose filename differs, or that you want to attach by hand, select the file and press `L`, then paste the gist URL (`https://gist.github.com/<user>/<id>`) or the bare ID. The gist is fetched, reconciled against the local content, and recorded; the tree then shows whether the two are in sync or have diverged (use `D` to diff, `u`/`d` to reconcile). A multi-file gist must share a filename with the local file (otherwise the pairing is ambiguous and is refused).

## Architecture

Three-crate Cargo workspace:

- **`crates/penknife-backend`**: the backend contract (create/read/update/delete plus an optional changed-since feed). Each backend declares itself *sync* (lossless round-trip, pull is safe) or *publish* (lossy up-render, push-only). One trait, many services.
- **`crates/penknife-gist`**: standalone GitHub Gist client and the founding `Backend` implementation. Auth via `$GITHUB_TOKEN` or `gh auth token`. Idempotent GET retries with exponential backoff and `Retry-After` / `X-RateLimit-Reset` handling. Pagination via `Link` headers.
- **`crates/penknife`**: the TUI. Modes for normal navigation, search, diff, confirm, ambiguous-match resolution, root switcher, setup, and URL import. State persistence via atomic temp-file + rename. The store (v3) maps each file to a *list* of published copies, one per backend, so a single essay can be simultaneously current as a gist and (soon) a Google Doc.

## Development

```bash
cargo build           # build
cargo test            # run unit tests
cargo clippy          # lint
cargo fmt             # format (always run before committing)
```

The store format is versioned; `Store::load` migrates older formats in place on first launch.

House style: em dashes are a CI error, in code and prose alike. Use a comma, colon, or parenthetical instead.

## License

MIT.
