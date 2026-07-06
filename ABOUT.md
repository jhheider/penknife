# About penknife

## The problem

You write in markdown, in a folder you own. But your writing has to leave that folder: a gist link in Slack, a doc for a colleague, a post on a blog. A gist you share is a copy you can lose track of. Which version is current? Did someone edit the shared one? You can't tell without opening each destination and eyeballing it.

penknife treats each published gist like a git remote: it records what was pushed, notices when either side moves, and shows the drift per file. The folder is primary; the gist is a downstream copy. And when the destination isn't a gist at all (a manager's Doc, an email, a Slack message), penknife hands you the writing as rich text to paste, a deliberate snapshot with nothing to track.

## What it is not

- **Not a backup or device-sync tool.** obsidian-git and Obsidian Sync own that job. penknife assumes your folder is already safe.
- **Not an editor.** It opens `$EDITOR` and gets out of the way. The moment a tool renders an editing buffer it competes with Obsidian and vim simultaneously, and loses to both.
- **Not an Obsidian plugin.** Editor-agnosticism is the point; it works beside any editor, or none.

## Design principles

1. **Local-first.** Your files live on your disk. Sync is opt-in per file, never automatic mutation.
2. **Terminal native.** Pure TUI (ratatui). Fast, keyboard-driven, composable with other CLI tools.
3. **Honest state.** A file's icon reflects verified hashes, not optimism. A divergent remote shows as a conflict, never a fake "synced".
4. **Small surface.** Anything that can be automatic should not be a keybinding. Anything that can be a default should not be a config knob.

## Two jobs, kept separate

- **Sync** (gists): a live, drift-tracked remote you can link to. Push, pull, diff, and honest per-file status. Gists are the one blessed sync backend.
- **Share** (rich-text copy): a frozen snapshot handed to someone who doesn't live in gists. Rendered to HTML on the clipboard, paste into a Doc, an email, Slack, anything. No auth, no account, nothing to track, because you meant to hand them a copy.

Collapsing those two jobs into one "publish everywhere" feature was a mistake we made and undid. A lossy, auth-heavy publish target (Google Docs, Notion) fights both of penknife's promises at once: it's the highest-friction surface in the app, and because the round trip loses formatting it can never honestly say "your shared copy is current." The share job is better served by a rich-text paste, which the destination renders natively and which needs no cloud project, OAuth, or token.

## Roadmap

Ordered by intent, not by promise:

1. ~~**Polling.**~~ Done: remote poll on a timer, local filesystem sweep, auto-hydrate at startup. The manual check/refresh/hydrate keys retired when their jobs became automatic.
2. ~~**Backend trait.**~~ Done: `penknife-backend` is a small internal seam so the sync engine, polling, and store don't hard-code GitHub. Gists are the only backend; the trait keeps the door open without advertising a hallway.
3. ~~**Copy as rich text.**~~ Done: `p` renders the file to HTML and puts it on the clipboard for a paste into any rich editor.
4. ~~**First public release.**~~ Done (v0.1.0): tag, prebuilt binaries, crates.io, a Homebrew tap.
5. ~~**Headless CLI.**~~ Done (v0.2.0): `render`, `search`, `push`, `url`, `status` so penknife composes with editors and scripts, not just the full-screen app.

Explicitly declined: **Google Docs / Notion publish and any other OAuth or token-setup backend** (friction that fights the product; the share job is a rich-text paste), Medium (API discontinued), Telegraph, paste services, plugin systems, git porcelain beyond the basics, team merge workflows, bidirectional sync for any lossy format.

## License

MIT.
