//! The headless command-line surface. Bare `penknife` launches the TUI; a
//! subcommand runs one operation and exits, so penknife composes with editors
//! and scripts.
//!
//! Two rules hold across every command:
//! - stdout is the machine payload (HTML, matches, a URL); stderr is for
//!   humans (progress, warnings, errors). That keeps `x=$(penknife ...)`
//!   clean and every command pipeable.
//! - exit codes are an API: 0 success, 1 a normal-negative (no match), 2 a
//!   usage error (clap), 3 auth, 4 no matching root, 5 an operational error.
//!
//! The commands here (`render`, `search`) touch no config, store, token, or
//! network, so they run on a bare CI runner. That is deliberate: it is the
//! surface the packaging `test:` step exercises without credentials.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use penknife_gist::{GistClient, GistError};

use crate::config::Config;
use crate::store::Store;
use crate::sync::{self, PushOutcome, SyncStatus};

pub const EXIT_OK: i32 = 0;
pub const EXIT_NEGATIVE: i32 = 1;
pub const EXIT_AUTH: i32 = 3;
pub const EXIT_NO_ROOT: i32 = 4;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_OPERATIONAL: i32 = 5;

#[derive(Parser)]
#[command(
    name = "penknife",
    version,
    about = "A terminal home for your markdown, with drift-tracked gist sharing",
    long_about = "Run with no command to launch the interactive TUI. The subcommands \
                  below run one operation and exit.\n\n\
                  render and search need no account or network. push, url, and status \
                  need a GitHub token with the 'gist' scope (run 'gh auth login')."
)]
pub struct Cli {
    /// Open the config file in $EDITOR and exit
    #[arg(short = 'c', long = "config")]
    pub config: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Render a markdown file to HTML on stdout
    Render {
        /// Markdown file, or - for stdin. Omit to read piped stdin.
        file: Option<String>,
        /// Wrap the output in a full HTML document (for opening in a browser)
        #[arg(short = 's', long)]
        standalone: bool,
    },
    /// Search the full text of your writing (substring, grep-style)
    ///
    /// Searches .md files under the given path (default: the current directory).
    Search {
        /// Text to find (case-sensitive substring)
        query: String,
        /// Directory to search (default: the current directory)
        path: Option<PathBuf>,
        /// Match case-insensitively
        #[arg(short = 'i', long = "ignore-case")]
        ignore_case: bool,
        /// Print only the file paths that contain a match
        #[arg(short = 'l', long = "files-with-matches")]
        files_with_matches: bool,
        /// Emit one JSON object per match instead of text
        #[arg(long)]
        json: bool,
        /// Suppress output; set the exit code only
        #[arg(short = 'q', long)]
        quiet: bool,
    },
    /// Publish or update the file's gist, then print its URL
    ///
    /// Needs a GitHub token with the 'gist' scope (run 'gh auth login').
    Push {
        /// A markdown file under one of your watched folders
        file: PathBuf,
        /// Overwrite even if the gist changed on GitHub since your last sync
        #[arg(long)]
        force: bool,
    },
    /// Print (and optionally copy) the file's shareable gist URL
    ///
    /// Needs a GitHub token with the 'gist' scope (run 'gh auth login').
    Url {
        /// A markdown file under one of your watched folders
        file: PathBuf,
        /// Copy the URL to the system clipboard
        #[arg(long)]
        clip: bool,
        /// Push the current content first, then print the URL
        #[arg(long)]
        push: bool,
        /// Open the URL in your browser
        #[arg(long)]
        open: bool,
    },
    /// Show per-file sync drift; exit 1 if anything has drifted
    ///
    /// Offline by default; --sync checks GitHub live and needs a token.
    Status {
        /// A file or directory to check (default: every published file)
        path: Option<PathBuf>,
        /// Check GitHub live (needs a token) instead of the local record
        #[arg(long)]
        sync: bool,
        /// Machine form: one `<code>\t<path>` line per file
        #[arg(long)]
        porcelain: bool,
        /// One JSON object per file
        #[arg(long)]
        json: bool,
        /// Suppress output; set the exit code only
        #[arg(short = 'q', long)]
        quiet: bool,
    },
    /// Pull the gist's content down into the local file
    ///
    /// Needs a GitHub token with the 'gist' scope (run 'gh auth login').
    Pull {
        /// A markdown file under one of your watched folders
        file: PathBuf,
        /// Overwrite local changes that haven't been pushed
        #[arg(long)]
        force: bool,
    },
    /// Show a unified diff of the local file against its gist
    ///
    /// Needs a GitHub token with the 'gist' scope (run 'gh auth login').
    Diff {
        /// A markdown file under one of your watched folders
        file: PathBuf,
    },
}

/// Run a one-shot subcommand and return its process exit code.
pub async fn run(command: Command) -> i32 {
    match command {
        Command::Render { file, standalone } => run_render(file, standalone),
        Command::Search {
            query,
            path,
            ignore_case,
            files_with_matches,
            json,
            quiet,
        } => run_search(query, path, ignore_case, files_with_matches, json, quiet),
        Command::Push { file, force } => run_push(file, force).await,
        Command::Url {
            file,
            clip,
            push,
            open,
        } => run_url(file, clip, push, open).await,
        Command::Status {
            path,
            sync,
            porcelain,
            json,
            quiet,
        } => run_status(path, sync, porcelain, json, quiet).await,
        Command::Pull { file, force } => run_pull(file, force).await,
        Command::Diff { file } => run_diff(file).await,
    }
}

fn run_render(file: Option<String>, standalone: bool) -> i32 {
    let (source, title) = match read_source(file.as_deref()) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let fragment = crate::markdown::render_html(&source);
    let out = if standalone {
        crate::markdown::standalone(&fragment, &source, &title)
    } else {
        fragment
    };
    print!("{out}");
    // Block-level HTML already ends with a newline; a bare fragment (e.g. from
    // inline-only input) may not, so a capture and the next prompt stay tidy.
    if !out.ends_with('\n') {
        println!();
    }
    EXIT_OK
}

/// Resolve render input to (markdown, title). Reads a file, or stdin when the
/// arg is `-` or omitted-and-piped. Refuses to hang on an interactive stdin.
fn read_source(file: Option<&str>) -> Result<(String, String), i32> {
    let read_stdin = || {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map(|_| (buf, "penknife".to_string()))
            .map_err(|e| {
                eprintln!("penknife: reading stdin: {e}");
                EXIT_OPERATIONAL
            })
    };
    match file {
        Some("-") => read_stdin(),
        Some(path) => std::fs::read_to_string(path)
            .map(|s| {
                let title = std::path::Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "penknife".into());
                (s, title)
            })
            .map_err(|e| {
                eprintln!("penknife: {path}: {e}");
                EXIT_OPERATIONAL
            }),
        None => {
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "penknife: no input. Give a file, or pipe markdown in:\n  \
                     penknife render notes.md\n  cat notes.md | penknife render -"
                );
                Err(EXIT_USAGE)
            } else {
                read_stdin()
            }
        }
    }
}

fn run_search(
    query: String,
    path: Option<PathBuf>,
    ignore_case: bool,
    files_with_matches: bool,
    json: bool,
    quiet: bool,
) -> i32 {
    let dir = path.unwrap_or_else(|| PathBuf::from("."));
    if !dir.exists() {
        eprintln!("penknife: {}: no such directory", dir.display());
        return EXIT_OPERATIONAL;
    }
    // rel_path in each match is relative to `dir`; join it back so printed
    // paths open from where the user invoked the search (grep semantics).
    let matches = crate::replace::scan_opts(&dir, &dir, &query, ignore_case);
    if matches.is_empty() {
        return EXIT_NEGATIVE;
    }
    if quiet {
        return EXIT_OK;
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if files_with_matches {
        let mut seen = std::collections::BTreeSet::new();
        for m in &matches {
            if seen.insert(&m.rel_path) {
                let _ = writeln!(out, "{}", dir.join(&m.rel_path).display());
            }
        }
    } else if json {
        for m in &matches {
            // Hand-built JSON keeps this dependency-free and the field order
            // stable; paths/text are escaped.
            let _ = writeln!(
                out,
                "{{\"path\":{},\"line\":{},\"col\":{},\"text\":{}}}",
                json_str(&dir.join(&m.rel_path).to_string_lossy()),
                m.line,
                m.col_byte,
                json_str(&m.line_text)
            );
        }
    } else {
        for m in &matches {
            let _ = writeln!(
                out,
                "{}:{}:{}",
                dir.join(&m.rel_path).display(),
                m.line,
                m.line_text
            );
        }
    }
    EXIT_OK
}

async fn run_push(file: PathBuf, force: bool) -> i32 {
    let config = Config::load().unwrap_or_default();
    let (root, rel) = match resolve_file(&config, &file) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let content = match std::fs::read_to_string(&file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("penknife: {}: {e}", file.display());
            return EXIT_OPERATIONAL;
        }
    };
    let mut store = Store::load().unwrap_or_default();
    let existing = store.get(&root, &rel).cloned();
    let token = match penknife_gist::auth::resolve_token() {
        Ok(t) => t,
        Err(_) => return no_token_error(),
    };
    let client = GistClient::new(token);
    match sync::push(&client, existing.as_ref(), &basename(&rel), &content, force).await {
        Ok(PushOutcome::Pushed(entry)) => {
            let url = entry.url.clone();
            store.insert(&root, rel.clone(), entry);
            eprintln!("penknife: pushed {rel}");
            println!("{url}");
            if let Err(e) = store.save() {
                eprintln!("penknife: pushed, but saving local state failed: {e}");
                return EXIT_OPERATIONAL;
            }
            EXIT_OK
        }
        Ok(PushOutcome::RemoteChanged { .. }) => {
            eprintln!(
                "penknife: not pushing. The gist changed on GitHub since your last sync,\n  \
                 so pushing now would overwrite those edits.\n  \
                 Inspect:   penknife status --sync {0}\n  \
                 Overwrite: penknife push --force {0}",
                file.display()
            );
            EXIT_NEGATIVE
        }
        Err(e) => report_gist_error(e),
    }
}

async fn run_url(file: PathBuf, clip: bool, push: bool, open: bool) -> i32 {
    let config = Config::load().unwrap_or_default();
    let (root, rel) = match resolve_file(&config, &file) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut store = Store::load().unwrap_or_default();
    let existing = store.get(&root, &rel).cloned();

    // Fast path: already published and not asked to push. Reuse the stored URL
    // with no network and no token; just note if the local copy has moved on.
    let url = if let (Some(entry), false) = (&existing, push) {
        if let Ok(content) = std::fs::read_to_string(&file)
            && sync::local_status(&content, Some(entry)) == SyncStatus::LocalNewer
        {
            eprintln!(
                "penknife: note: your local copy is newer than the shared gist; \
                 run 'penknife push {}' to update it.",
                file.display()
            );
        }
        entry.url.clone()
    } else {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("penknife: {}: {e}", file.display());
                return EXIT_OPERATIONAL;
            }
        };
        let token = match penknife_gist::auth::resolve_token() {
            Ok(t) => t,
            Err(_) => return no_token_error(),
        };
        let client = GistClient::new(token);
        match sync::push(&client, existing.as_ref(), &basename(&rel), &content, false).await {
            Ok(PushOutcome::Pushed(entry)) => {
                let url = entry.url.clone();
                store.insert(&root, rel.clone(), entry);
                eprintln!("penknife: published {rel}");
                if let Err(e) = store.save() {
                    eprintln!("penknife: published, but saving local state failed: {e}");
                    return EXIT_OPERATIONAL;
                }
                url
            }
            Ok(PushOutcome::RemoteChanged { .. }) => {
                eprintln!(
                    "penknife: the gist changed on GitHub since your last sync.\n  \
                     Inspect:   penknife status --sync {0}\n  \
                     Overwrite: penknife push --force {0}",
                    file.display()
                );
                return EXIT_NEGATIVE;
            }
            Err(e) => return report_gist_error(e),
        }
    };

    println!("{url}");
    // Side effects are opt-in and non-fatal: a scripted capture never wants a
    // surprise clipboard write, and a missing display must not fail the run.
    if clip {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(url.clone())) {
            Ok(()) => eprintln!("penknife: copied link to clipboard."),
            Err(e) => eprintln!("penknife: could not copy to clipboard: {e}"),
        }
    }
    if open && let Err(e) = open::that(&url) {
        eprintln!("penknife: could not open browser: {e}");
    }
    EXIT_OK
}

async fn run_status(
    path: Option<PathBuf>,
    sync_live: bool,
    porcelain: bool,
    json: bool,
    quiet: bool,
) -> i32 {
    let config = Config::load().unwrap_or_default();
    let store = Store::load().unwrap_or_default();

    // A path (file or directory) filters to entries whose on-disk location is
    // under it; no path means every published file across all roots.
    let filter = match &path {
        Some(p) => match std::fs::canonicalize(p) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("penknife: {}: {e}", p.display());
                return EXIT_OPERATIONAL;
            }
        },
        None => None,
    };

    let mut targets: Vec<(PathBuf, String, crate::store::FileEntry)> = Vec::new();
    for root in &config.roots {
        let Some(map) = store.files_for_root(&root.path) else {
            continue;
        };
        let canon_root = std::fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.clone());
        for (rel, copies) in map {
            let Some(entry) = copies
                .iter()
                .find(|c| c.backend == crate::store::GIST_BACKEND)
            else {
                continue;
            };
            if let Some(prefix) = &filter {
                let abs = canon_root.join(rel);
                let abs = std::fs::canonicalize(&abs).unwrap_or(abs);
                if !abs.starts_with(prefix) {
                    continue;
                }
            }
            targets.push((root.path.clone(), rel.clone(), entry.clone()));
        }
    }
    targets.sort_by(|a, b| a.1.cmp(&b.1));

    let client = if sync_live {
        match penknife_gist::auth::resolve_token() {
            Ok(t) => Some(GistClient::new(t)),
            Err(_) => return no_token_error(),
        }
    } else {
        None
    };

    let mut any_drift = false;
    let mut had_errors = false;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (root, rel, entry) in &targets {
        let abs = root.join(rel);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("penknife: {rel}: {e}");
                had_errors = true;
                continue;
            }
        };
        let status = if let Some(client) = &client {
            match sync::full_status(client, &content, entry, &basename(rel)).await {
                Ok(fs) => fs.status,
                Err(e) => {
                    eprintln!("penknife: {rel}: {e}");
                    had_errors = true;
                    continue;
                }
            }
        } else {
            sync::local_status(&content, Some(entry))
        };
        if status != SyncStatus::Synced {
            any_drift = true;
        }
        if quiet {
            continue;
        }
        if porcelain {
            let _ = writeln!(out, "{}\t{}", status_code(status), rel);
        } else if json {
            let _ = writeln!(
                out,
                "{{\"path\":{},\"status\":{},\"url\":{}}}",
                json_str(rel),
                json_str(status_label(status)),
                json_str(&entry.url)
            );
        } else {
            let _ = writeln!(
                out,
                "{} {:<13} {}",
                status.icon(),
                status_label(status),
                rel
            );
        }
    }
    if had_errors {
        EXIT_OPERATIONAL
    } else if any_drift {
        EXIT_NEGATIVE
    } else {
        EXIT_OK
    }
}

async fn run_pull(file: PathBuf, force: bool) -> i32 {
    let config = Config::load().unwrap_or_default();
    let (root, rel) = match resolve_file(&config, &file) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut store = Store::load().unwrap_or_default();
    let Some(entry) = store.get(&root, &rel).cloned() else {
        eprintln!("penknife: {rel} is not published; nothing to pull.");
        return EXIT_OPERATIONAL;
    };
    let local = std::fs::read_to_string(&file).unwrap_or_default();
    // Drift guard: refuse to clobber unpushed local changes unless forced.
    if !force {
        let st = sync::local_status(&local, Some(&entry));
        if matches!(st, SyncStatus::LocalNewer | SyncStatus::Conflict) {
            eprintln!(
                "penknife: {rel} has local changes that aren't on the gist; \
                 pulling would overwrite them.\n  \
                 Push them first, or use: penknife pull --force {}",
                file.display()
            );
            return EXIT_NEGATIVE;
        }
    }
    let token = match penknife_gist::auth::resolve_token() {
        Ok(t) => t,
        Err(_) => return no_token_error(),
    };
    let client = GistClient::new(token);
    match sync::pull(&client, &entry, &basename(&rel)).await {
        Ok((content, updated)) => {
            if let Err(e) = std::fs::write(&file, &content) {
                eprintln!("penknife: writing {}: {e}", file.display());
                return EXIT_OPERATIONAL;
            }
            store.insert(&root, rel.clone(), updated);
            eprintln!("penknife: pulled {rel}");
            if let Err(e) = store.save() {
                eprintln!("penknife: pulled, but saving local state failed: {e}");
                return EXIT_OPERATIONAL;
            }
            EXIT_OK
        }
        Err(e) => report_gist_error(e),
    }
}

async fn run_diff(file: PathBuf) -> i32 {
    let config = Config::load().unwrap_or_default();
    let (root, rel) = match resolve_file(&config, &file) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let store = Store::load().unwrap_or_default();
    let Some(entry) = store.get(&root, &rel).cloned() else {
        eprintln!("penknife: {rel} is not published; nothing to diff.");
        return EXIT_OPERATIONAL;
    };
    let local = std::fs::read_to_string(&file).unwrap_or_default();
    let token = match penknife_gist::auth::resolve_token() {
        Ok(t) => t,
        Err(_) => return no_token_error(),
    };
    let client = GistClient::new(token);
    let full = match sync::full_status(&client, &local, &entry, &basename(&rel)).await {
        Ok(f) => f,
        Err(e) => return report_gist_error(e),
    };
    // diff(1) semantics: no output and exit 0 when identical, the diff and
    // exit 1 when they differ.
    if full.remote_content == local {
        return EXIT_OK;
    }
    let diff = similar::TextDiff::from_lines(&full.remote_content, &local);
    let (gist_label, local_label) = (format!("{rel} (gist)"), format!("{rel} (local)"));
    let mut unified = diff.unified_diff();
    unified.context_radius(3).header(&gist_label, &local_label);
    print!("{unified}");
    EXIT_NEGATIVE
}

/// Map a file path to (configured root, rel_path within it). The store keys on
/// the root, so this is how a raw CLI path finds its gist entry. Errors (with
/// the watched-folder list) when the file is under no configured root.
fn resolve_file(config: &Config, target: &Path) -> Result<(PathBuf, String), i32> {
    let canon = match std::fs::canonicalize(target) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("penknife: {}: {e}", target.display());
            return Err(EXIT_OPERATIONAL);
        }
    };
    for root in &config.roots {
        let canon_root = std::fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.clone());
        if let Ok(rel) = canon.strip_prefix(&canon_root) {
            return Ok((root.path.clone(), crate::scanner::rel_to_string(rel)));
        }
    }
    eprintln!(
        "penknife: {} is not under any folder penknife watches.",
        target.display()
    );
    if config.roots.is_empty() {
        eprintln!("  No folders are configured yet. Launch 'penknife' once to add one.");
    } else {
        eprintln!("  Watched folders:");
        for root in &config.roots {
            eprintln!("    {}", root.path.display());
        }
        eprintln!("  Add one by launching 'penknife' (press R), or edit 'penknife --config'.");
    }
    Err(EXIT_NO_ROOT)
}

fn no_token_error() -> i32 {
    eprintln!(
        "penknife: this needs a GitHub token, and none was found.\n  \
         Fix it with one of:\n    \
         gh auth login             (recommended; penknife reads gh's token)\n    \
         export GITHUB_TOKEN=...   (a token with the 'gist' scope)"
    );
    EXIT_AUTH
}

fn report_gist_error(e: anyhow::Error) -> i32 {
    match e.downcast_ref::<GistError>() {
        Some(GistError::Api { status: 403, .. }) => {
            eprintln!(
                "penknife: your GitHub token can read but not publish; it's missing the \
                 'gist' scope.\n  \
                 If you use gh:  gh auth refresh -s gist\n  \
                 Otherwise, use a token with the 'gist' box checked."
            );
            EXIT_AUTH
        }
        Some(GistError::NoToken) => no_token_error(),
        Some(g) => {
            eprintln!("penknife: {g}");
            EXIT_OPERATIONAL
        }
        None => {
            eprintln!("penknife: {e:#}");
            EXIT_OPERATIONAL
        }
    }
}

/// The gist filename for a rel_path (its basename); gists key files by name.
fn basename(rel: &str) -> String {
    Path::new(rel)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.to_string())
}

fn status_code(s: SyncStatus) -> char {
    match s {
        SyncStatus::Synced => '=',
        SyncStatus::LocalNewer => '^',
        SyncStatus::RemoteNewer => 'v',
        SyncStatus::Conflict => '!',
        SyncStatus::NotGisted => '?',
    }
}

fn status_label(s: SyncStatus) -> &'static str {
    match s {
        SyncStatus::Synced => "synced",
        SyncStatus::LocalNewer => "local-newer",
        SyncStatus::RemoteNewer => "remote-newer",
        SyncStatus::Conflict => "conflict",
        SyncStatus::NotGisted => "not-published",
    }
}

/// Minimal JSON string encoder (quotes + the escapes JSON requires).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_str_escapes_quotes_and_control() {
        assert_eq!(json_str(r#"a"b"#), r#""a\"b""#);
        assert_eq!(json_str("a\tb\n"), r#""a\tb\n""#);
    }

    #[test]
    fn search_missing_dir_is_operational() {
        let code = run_search(
            "x".into(),
            Some("/no/such/dir".into()),
            false,
            false,
            false,
            true,
        );
        assert_eq!(code, EXIT_OPERATIONAL);
    }

    #[test]
    fn basename_takes_the_last_component() {
        assert_eq!(basename("a/b/c.md"), "c.md");
        assert_eq!(basename("flat.md"), "flat.md");
    }

    #[test]
    fn status_code_and_label_are_stable() {
        assert_eq!(status_code(SyncStatus::Synced), '=');
        assert_eq!(status_code(SyncStatus::LocalNewer), '^');
        assert_eq!(status_code(SyncStatus::Conflict), '!');
        assert_eq!(status_label(SyncStatus::RemoteNewer), "remote-newer");
        assert_eq!(status_label(SyncStatus::NotGisted), "not-published");
    }

    #[test]
    fn report_gist_error_classifies_by_variant() {
        // A missing token and a 403 (a token without the 'gist' scope) are
        // auth failures; every other gist error, and any non-gist error, is
        // operational.
        assert_eq!(report_gist_error(GistError::NoToken.into()), EXIT_AUTH);
        let forbidden = GistError::Api {
            status: 403,
            message: "Forbidden".into(),
        };
        assert_eq!(report_gist_error(forbidden.into()), EXIT_AUTH);
        let server_err = GistError::Api {
            status: 500,
            message: "boom".into(),
        };
        assert_eq!(report_gist_error(server_err.into()), EXIT_OPERATIONAL);
        assert_eq!(
            report_gist_error(anyhow::anyhow!("a local IO failure")),
            EXIT_OPERATIONAL
        );
    }

    #[test]
    fn report_gist_error_sees_gist_error_through_context() {
        use anyhow::Context;
        // A gist error wrapped in added context must still be classified by
        // its variant: the downcast walks the chain, so the 'gist'-scope hint
        // survives even when a caller layers `.context()` on top.
        let wrapped = Err::<(), _>(GistError::NoToken)
            .context("while publishing")
            .unwrap_err();
        assert_eq!(report_gist_error(wrapped), EXIT_AUTH);
    }

    #[test]
    fn resolve_file_finds_root_and_rel_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let file = root.join("sub/note.md");
        std::fs::write(&file, "x").unwrap();
        let config = Config {
            roots: vec![crate::config::Root::new(root)],
            ..Default::default()
        };
        let (_root, rel) = resolve_file(&config, &file).unwrap();
        assert_eq!(rel, "sub/note.md");
    }

    #[test]
    fn resolve_file_rejects_a_file_outside_every_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside.md");
        std::fs::write(&outside, "x").unwrap();
        let config = Config {
            roots: vec![crate::config::Root::new(root)],
            ..Default::default()
        };
        assert_eq!(resolve_file(&config, &outside), Err(EXIT_NO_ROOT));
    }
}
