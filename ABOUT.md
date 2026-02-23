# About gist-tools

## Vision

gist-tools exists to solve a specific problem: **GitHub Gists are great for distributed note-taking and code snippets, but they're scattered across the web.**

This tool brings gists into your local workflow — search them, sync them, diff them — all from the terminal without leaving your editor.

## Design Philosophy

### 1. Local-First

Your gists live on your disk. You own the data. Sync is opt-in, not automatic. The tool facilitates the connection to GitHub, but doesn't require it.

### 2. Terminal Native

No web UI, no Electron bloat. Pure TUI built with ratatui. Fast, scriptable, composable with other CLI tools.

### 3. Minimal Dependencies

Leverage Rust's ecosystem for speed and reliability:
- **reqwest** for HTTP (battle-tested)
- **ratatui** for rendering (lightweight, fast)
- **tokio** for async (industry standard)
- **serde** for serialization (stable, pervasive)

No external services (except GitHub). No fancy databases.

### 4. Extensible Architecture

- **gist-rs** is a standalone library → can be used in other tools
- **wm** (the TUI) is modular → easy to add new UI modes, search strategies, sync logic
- **Config-driven** → future plugins or integrations via config files

## Use Cases

**Personal knowledge base**: Store snippets, templates, quick references in gists. Sync to local disk, search offline, update when needed.

**Collaborative notes**: Share gists with a team. Each member syncs to their machine, edits locally, pushes back up.

**Ephemeral data**: Google Docs hydration means you can paste a doc, convert to markdown, commit to gist. Useful for converting workshop notes → reusable docs.

**Scripting**: gist-rs library can be used in build systems, automation, etc. to read/write gists programmatically.

## Why Not Just Use GitHub Web UI?

**Speed**: Terminal navigation is faster than web UI for power users.

**Offline**: Download once, search/browse offline. No network latency.

**Integration**: Pipe gist content to other tools. Diff with `git` directly. Commit to version control.

**Keyboard-driven**: No mouse. No context-switching to a browser tab.

## Why Not Just Use `gh` CLI?

`gh` is great for individual operations (`gh gist view`, `gh gist create`). This tool treats gists as a *collection* to browse, search, and manage holistically.

Think:
- `gh` = individual file operations
- gist-tools = filesystem-like interface to your gist collection

## Future Direction

- **Multi-source**: Support Gists, GitHub Discussions, GitHub Wikis as note sources
- **Sync backends**: Not just GitHub. GitLab, Gitea, custom Git repos
- **Plugins**: Hooks for custom processing (e.g., auto-convert Markdown → HTML)
- **Collaborative workflows**: Team sync, conflict resolution, merge strategies
- **Mobile sync**: Keep local collection in sync across devices

## Why Build This?

Because managing distributed notes is tedious, and the terminal is the most efficient interface for information work when you're comfortable there.

---

**v0.1.0** addresses the core: sync, search, diff. Future versions will expand scope.
