# About penknife

## The problem

You write in markdown, in a folder you own. But your writing has to leave that folder: a gist link in Slack, a doc for a colleague, a post on a blog. Every copy you paste somewhere is a copy you lose track of. Which version is current? Did someone edit the shared one? You can't tell without opening each destination and eyeballing it.

penknife treats each published copy like a git remote: it records what was pushed, notices when either side moves, and shows the drift per file. The folder is primary; every remote is a downstream copy.

## What it is not

- **Not a backup or device-sync tool.** obsidian-git and Obsidian Sync own that job. penknife assumes your folder is already safe.
- **Not an editor.** It opens `$EDITOR` and gets out of the way. The moment a tool renders an editing buffer it competes with Obsidian and vim simultaneously, and loses to both.
- **Not an Obsidian plugin.** Editor-agnosticism is the point; it works beside any editor, or none.

## Design principles

1. **Local-first.** Your files live on your disk. Sync is opt-in per file, never automatic mutation.
2. **Terminal native.** Pure TUI (ratatui). Fast, keyboard-driven, composable with other CLI tools.
3. **Honest state.** A file's icon reflects verified hashes, not optimism. A divergent remote shows as a conflict, never a fake "synced".
4. **Small surface.** Anything that can be automatic should not be a keybinding. Anything that can be a default should not be a config knob.

## Roadmap

Ordered by intent, not by promise:

1. ~~**Polling.**~~ Done: remote poll on a timer, local filesystem sweep, auto-hydrate at startup. The manual check/refresh/hydrate keys retired when their jobs became automatic.
2. **Backend trait.** The gist client becomes the first implementation of a small backend contract (authenticate, create, read, update, delete, list-changed-since). Each backend declares itself *sync* (lossless round-trip, pull is safe) or *publish* (lossy up-render, push-only).
3. **One file, many remotes.** The store maps a file to a list of published copies, so one essay can be simultaneously current as a gist and a Google Doc.
4. **Google Docs publish.** Push markdown up via the Drive API's conversion path; re-push replaces the doc. Publish-only: bidirectional sync with a lossy format is a conflict factory.
5. **Notion publish** via its markdown content API, then the long tail (dev.to, GitLab snippets) as demand shows up.

Explicitly declined: Medium (API discontinued), Telegraph, paste services, plugin systems, git porcelain beyond the basics, team merge workflows.

## License

MIT.
