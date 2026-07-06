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
use std::path::PathBuf;

use clap::{Parser, Subcommand};

pub const EXIT_OK: i32 = 0;
pub const EXIT_NEGATIVE: i32 = 1;
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
    Search {
        /// Text to find (case-sensitive substring)
        query: String,
        /// Directory to search (default: the current directory)
        path: Option<PathBuf>,
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
}

/// Run a one-shot subcommand and return its process exit code.
pub fn run(command: Command) -> i32 {
    match command {
        Command::Render { file, standalone } => run_render(file, standalone),
        Command::Search {
            query,
            path,
            files_with_matches,
            json,
            quiet,
        } => run_search(query, path, files_with_matches, json, quiet),
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
                Err(2)
            } else {
                read_stdin()
            }
        }
    }
}

fn run_search(
    query: String,
    path: Option<PathBuf>,
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
    let matches = crate::replace::scan(&dir, &dir, &query);
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
        let code = run_search("x".into(), Some("/no/such/dir".into()), false, false, true);
        assert_eq!(code, EXIT_OPERATIONAL);
    }
}
