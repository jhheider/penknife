# penknife

[![CI](https://github.com/jhheider/penknife/actions/workflows/ci.yml/badge.svg)](https://github.com/jhheider/penknife/actions/workflows/ci.yml)
[![Coverage Status](https://coveralls.io/repos/github/jhheider/penknife/badge.svg?branch=main)](https://coveralls.io/github/jhheider/penknife?branch=main)
[![crates.io](https://img.shields.io/crates/v/penknife.svg)](https://crates.io/crates/penknife)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](#license)

A fast terminal home for your markdown writing.

Point penknife at a folder of writing (an Obsidian vault, a notes directory, anywhere you keep `.md` files) and it gives you a keyboard-driven browser over all of it: search across everything, preview any file, and hand a piece to someone with one keystroke that pastes cleanly into a Doc, an email, or Slack, formatting intact.

That much needs no account and no setup. When you want it, penknife can also keep copies of your files on GitHub and tell you, at a glance, which ones are up to date and which have drifted, so a thing you shared last week never goes quietly stale. That part is optional; the rest works on its own.

## What you can do

**With zero setup, no account:**

- Browse a folder of markdown as a tidy tree, arbitrarily nested, and preview any file with syntax highlighting.
- **Share anything as rich text** (`p`): penknife renders the file and puts it on your clipboard so a paste into Google Docs, email, Slack, Notion, or any rich editor keeps your headings, bold, lists, tables, and links. No sign-up, no upload, nothing to track, just paste.
- Search the full text of every file (`f`) and jump straight to a match.
- Find and replace across a whole folder (`s`), reviewing each change before it applies.
- Open a file in your own editor (`e`), rename or move it (`m`), or pull text off the clipboard into a new file (`V`).

**When you connect GitHub (optional):**

- Publish any file to a gist and keep it in sync, with a per-file status that stays honest: up to date, local is newer, the online copy is newer, or the two have diverged.
- The tree keeps itself current on its own. Edit a file in another app, or change a gist from your phone, and penknife notices within seconds. No refresh button.
- Diff your local copy against the online one before you overwrite either side.
- penknife finds your existing gists and matches them to your files for you the first time you run it.

> **What's a gist?** A gist is a single file (or a few) hosted on GitHub, each with its own shareable link and a full history of edits, like a pastebin that remembers every version. penknife uses gists as the place your synced copies live. You need a free GitHub account to use this part; you do not need one for anything above.

## Install

**Download a ready-to-run copy** (nothing else needed): grab the archive for your platform from the [latest release](https://github.com/jhheider/penknife/releases/latest), unpack it, and put `penknife` on your `PATH`. On macOS a downloaded program is quarantined; clear it with `xattr -d com.apple.quarantine ./penknife`, or use Homebrew, which avoids that.

**Homebrew** (macOS / Linux):

```bash
brew install jhheider/tap/penknife
```

**With Cargo** (builds from source; needs a [Rust](https://rustup.rs) toolchain):

```bash
cargo install --git https://github.com/jhheider/penknife penknife
```

**From a clone** (for hacking on it):

```bash
git clone https://github.com/jhheider/penknife.git
cd penknife
cargo install --locked --path crates/penknife
```

## Getting started

Run `penknife`. The first time, it asks for a folder to watch: type a path (`~` works) and press **Enter**. It scans everything under there and shows you the tree.

From here you can already browse (`j`/`k` or arrows), open a preview (`Enter`), search everyone's favorite question "where did I write about X?" (`f`), and copy a file as rich text to paste anywhere (`p`). Press `?` at any time for the full key list, and `q` to quit.

**Want live sync too?** Give penknife a GitHub token with the `gist` permission. The friendly way is the [GitHub CLI](https://cli.github.com): run `gh auth login` once and penknife picks it up automatically. Or set `GITHUB_TOKEN` in your environment. Tokens are never written to disk by penknife; it reads them fresh each launch. With a token in place, `u` publishes the selected file and `c` copies its shareable link.

## Sharing and syncing

**Share (no account):** select a file, press `p`, and paste it wherever you're headed. What lands keeps its formatting, because penknife hands the destination real rich text, not raw markdown symbols. This is a deliberate snapshot: you're giving someone a copy, so there's nothing to keep in sync afterward.

**Sync (with GitHub):** when you want a living, shareable link that you can tell is current, publish the file as a gist with `u` and share its URL with `c`. From then on each file carries a small status icon:

| Slim (default) | Emoji | ASCII | Meaning |
|---|---|---|---|
| `✓` | ✅ | `[=]` | Up to date |
| `↑` | ⬆️ | `[^]` | Your copy is newer (push to update the gist) |
| `↓` | ⬇️ | `[v]` | The gist is newer (pull to update your copy) |
| `!` | ❗ | `[!]` | Both changed since you last synced |
| `·` | ⚪ | `[ ]` | Not published yet |

The status stays true on its own: penknife checks GitHub every few minutes and re-scans your folder every few seconds, so the icons reflect reality without you asking. Before overwriting either side, `D` shows you a diff, and a push refuses to clobber a gist that changed out from under you until you've looked.

## Keys

Press `?` in the app for this list any time. The essentials:

| Key | What it does |
|---|---|
| `j` `k` / arrows | Move around; `Enter` opens a folder or selects a file |
| `p` | Copy the file as rich text (paste into Docs, email, Slack) |
| `f` | Find in files: search all your text, jump to a match |
| `s` | Find and replace across the folder, with review |
| `e` | Open the file in your `$EDITOR` |
| `m` | Rename or move the file |
| `V` | Paste clipboard text as a new file (rich HTML becomes markdown) |
| `C` | Copy the file's raw markdown to the clipboard |
| `/` | Fuzzy jump to a file by name |
| `R` | Switch to another watched folder |
| `?` | Full help; `q` quits |

With a GitHub token, these light up too:

| Key | What it does |
|---|---|
| `u` | Publish or update the file as a gist |
| `d` | Pull the online copy down into your file |
| `c` | Copy the gist's shareable link (publishes first if needed) |
| `o` | Open the gist in your browser |
| `D` | Diff your copy against the online one |
| `n` / `N` | Jump to the next / previous file that's out of sync |
| `X` | Delete menu: remove the gist, trash the file, or both |
| `L` | Link a file to an existing gist by URL or ID |
| `I` | Import from a URL: a public Google Doc, or a gist |
| `M` | Resolve any ambiguous gist-to-file matches |

There's also a git menu (`g`) when your folder is a git repository, a sort menu (`O`), and a bulk-actions menu (`B`).

## Command line

Bare `penknife` launches the app; a subcommand runs one operation and exits, so penknife composes with your editor and scripts.

No account needed:

```bash
penknife render notes.md              # markdown to HTML on stdout
cat notes.md | penknife render -      # a Unix filter
penknife render -s notes.md > out.html   # -s wraps a full HTML document
penknife search "grapple"             # find in files, grep-style (exit 1 on no match)
```

With a GitHub token (the same one the app uses):

```bash
penknife push notes.md                # publish or update the gist, print its URL
penknife url notes.md --clip          # print the shareable URL and copy it
penknife status                       # per-file drift, offline and instant
penknife status --sync                # check GitHub live
penknife status --porcelain           # machine form for a shell prompt or hook
```

Each command prints its payload to stdout and everything else to stderr, and its exit code is meaningful (0 ok, 1 nothing-to-report or drifted, 2 usage, 3 auth, 4 file under no watched folder, 5 error). So `url=$(penknife push x.md)` captures a clean URL, and a `penknife status -q` pre-commit hook fails only when something drifted. Run `penknife --help` (or `penknife <command> --help`) for the rest.

## Configuration

`penknife --config` opens your config file in `$EDITOR`. It lives (with a small state file) in your platform data directory: `~/Library/Application Support/penknife` on macOS, `~/.local/share/penknife` on Linux.

**Watched folders** are added from inside the app (`R`), but you can also list them in config, each with optional ignore patterns:

```toml
[[roots]]
path = "~/Documents/writing"
ignore = ["drafts/**", "*.tmp"]
```

**Aliases** bind a single key to a shell command, run from the active folder:

```toml
[aliases]
S = "just stats"        # a quick word count
P = "git push"
```

Keys that clash with built-in ones are ignored with a warning.

**How often it checks** (seconds; `0` turns a check off):

```toml
[poll]
remote_secs = 300   # check GitHub every 5 minutes
local_secs = 5      # re-scan the folder every 5 seconds
```

**Appearance and input**, via environment variables:

- `PENKNIFE_MOUSE=1` turns on mouse support (click a row, scroll a pane). It's off by default so your terminal's own click-to-open-URL and text selection keep working.
- `PENKNIFE_EMOJI=1` uses wide emoji icons; `PENKNIFE_NO_EMOJI=1` forces plain ASCII. The default is slim single-width symbols that stay aligned everywhere.

## Under the hood

Everything below is for the curious and for contributors; you don't need any of it to use penknife.

### How syncing stays honest

penknife records a hash of both your file and its gist at each sync. A background check lists your gists every few minutes (cheap, and it backs off when you're offline) and a filesystem sweep re-scans your folder every few seconds, refreshing only what actually changed. Because it compares real hashes rather than timestamps, a gist edited elsewhere shows up as a genuine conflict, never a false "up to date."

### Hydration (matching gists to files)

On first run against a folder, penknife lists your gists and pairs them to local files by filename and content. Unique matches map automatically; where several gists share a filename, it asks you to pick (`M`). After the first pass it remembers a per-folder cursor and only fetches gists changed since, so later runs are fast even with hundreds of gists. A gist whose name doesn't match any file can be attached by hand with `L`.

### Project layout

A three-crate Cargo workspace:

- **`crates/penknife-backend`**: the contract a sync target implements (create, read, update, delete, and an optional changed-since feed). A small internal seam so the rest of the app doesn't hard-code GitHub. Gists are the only backend today; the trait keeps the door open.
- **`crates/penknife-gist`**: a standalone GitHub Gist client and the founding backend. Token from `$GITHUB_TOKEN` or `gh auth token`; idempotent retries with backoff and rate-limit handling; pagination via `Link` headers.
- **`crates/penknife`**: the terminal app itself. State is saved atomically (write-temp-then-rename) and the on-disk format is versioned, migrating older layouts in place on first launch.

### Development

```bash
cargo test            # run the tests
cargo clippy          # lint
cargo fmt             # format (run before committing)
```

House style: em dashes and en dashes are a CI error, in code and prose alike. Use a comma, colon, parenthetical, or a plain hyphen instead.

## License

MIT. See [LICENSE](LICENSE).
