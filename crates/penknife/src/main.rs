mod app;
mod cli;
mod config;
mod event;
mod gdoc;
mod git;
mod glyphs;
mod hydrate;
mod markdown;
mod picker;
mod remote;
mod replace;
mod scanner;
mod store;
mod sync;
mod sync_apply;
mod ui;

use std::io;
use std::panic;
use std::time::Duration;

use clap::Parser;
use crossterm::ExecutableCommand;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;

use app::App;
use event::{UiEvent, async_channel};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // Use ring as the rustls crypto provider. reqwest's `rustls-no-provider`
    // feature keeps the heavy aws-lc-rs C library out of the tree; install the
    // (lean) ring provider process-wide before any TLS request (idempotent -
    // the gist client installs it too, for tests that never run main).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = cli::Cli::parse();
    if args.config {
        return edit_config();
    }
    if let Some(command) = args.command {
        std::process::exit(cli::run(command).await);
    }

    // Panic hook: restore terminal before printing panic
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(DisableMouseCapture);
        let _ = io::stdout().execute(LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    // Setup terminal. Mouse capture is OFF by default so terminal-native
    // features (cmd-click on URLs, native text selection) keep working.
    // Power users can opt in with PENKNIFE_MOUSE=1.
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = async_channel();
    let mut app = App::new(tx)?;
    if glyphs::env_flag("MOUSE") {
        io::stdout().execute(EnableMouseCapture)?;
        app.mouse_capture = true;
    }

    // Main loop
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        match event::poll_event(Duration::from_millis(50)) {
            Some(UiEvent::Key(key)) if key.kind == KeyEventKind::Press => app.handle_key(key),
            Some(UiEvent::Mouse(m)) => app.handle_mouse(m),
            _ => {}
        }

        while let Ok(event) = rx.try_recv() {
            app.handle_async_event(event);
        }

        // Background cadence: the local filesystem sweep, remote poll, and
        // one-shot startup hydration all pace themselves off this call.
        app.tick();

        if let Some(path) = app.pending_editor.take() {
            suspend_and_edit(&mut terminal, &mut app, &path)?;
        }

        if let Some(cmd) = app.pending_alias.take() {
            suspend_and_run_alias(&mut terminal, &mut app, &cmd)?;
        }

        if app.should_quit {
            break;
        }
    }

    app.abort_tasks();

    if app.mouse_capture {
        let _ = io::stdout().execute(DisableMouseCapture);
    }
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// Open the config file in `$EDITOR` and exit (the `-c/--config` flag).
fn edit_config() -> color_eyre::Result<()> {
    let path = config::Config::config_path();
    // Ensure the file exists (and the parent dir) so the editor has something
    // to open. If we have saved state, leave it alone; otherwise drop a
    // minimal scaffold the user can edit.
    if !path.exists() {
        let cfg = config::Config::load()?;
        cfg.save()?;
    }
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&path).status()?;
    if !status.success() {
        eprintln!("penknife: {editor} exited with status {status}");
    }
    Ok(())
}

/// Suspend the TUI (leave alt screen, drop raw mode, release mouse capture),
/// spawn `$EDITOR` synchronously on the given path, then re-enter the TUI
/// and force a redraw. Refreshes the file list afterward so any external
/// edits show up in the tree and preview.
fn suspend_and_edit(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    path: &std::path::Path,
) -> color_eyre::Result<()> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let had_mouse = app.mouse_capture;
    if had_mouse {
        let _ = io::stdout().execute(DisableMouseCapture);
    }
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    let status = std::process::Command::new(&editor).arg(path).status();

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    if had_mouse {
        let _ = io::stdout().execute(EnableMouseCapture);
    }
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => {
            if let Err(e) = app.refresh_files() {
                app.status_message = format!("Refresh after edit failed: {e}");
            } else {
                app.status_message = format!("Returned from {editor}.");
            }
            app.update_status();
        }
        Ok(s) => {
            app.status_message = format!("{editor} exited with status {s}");
        }
        Err(e) => {
            app.status_message = format!("Failed to launch {editor}: {e}");
        }
    }

    Ok(())
}

/// Suspend the TUI, run a user-defined alias command via `sh -c`, then
/// restore. Working directory is the active root so commands like
/// `git push` operate on the right repo when the root is one. We refresh
/// the file list afterward in case the command modified anything.
fn suspend_and_run_alias(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    cmd: &str,
) -> color_eyre::Result<()> {
    let cwd = app.active_root_path();

    let had_mouse = app.mouse_capture;
    if had_mouse {
        let _ = io::stdout().execute(DisableMouseCapture);
    }
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(cmd);
    if let Some(d) = &cwd {
        command.current_dir(d);
    }
    let status = command.status();

    // Pause for a keypress so the user can actually read the output before
    // the TUI redraws over it. Without this, fast commands like `git status`
    // flash and disappear. Re-enable raw mode briefly so a single keystroke
    // (without Enter) dismisses the prompt; then re-enter alt screen.
    use std::io::Write as _;
    let _ = write!(
        io::stdout(),
        "\n\x1b[2m[Press any key to return to penknife]\x1b[0m"
    );
    let _ = io::stdout().flush();
    enable_raw_mode()?;
    // Drain whatever event arrives - Key, Mouse, Resize all dismiss.
    let _ = crossterm::event::read();
    // raw mode stays on for the TUI; we don't need to toggle it off again.
    io::stdout().execute(EnterAlternateScreen)?;
    if had_mouse {
        let _ = io::stdout().execute(EnableMouseCapture);
    }
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => {
            if let Err(e) = app.refresh_files() {
                app.status_message = format!("Refresh after alias failed: {e}");
            } else {
                app.status_message = format!("Ran: {cmd}");
            }
            app.update_status();
        }
        Ok(s) => {
            app.status_message = format!("`{cmd}` exited with status {s}");
        }
        Err(e) => {
            app.status_message = format!("Failed to run `{cmd}`: {e}");
        }
    }

    Ok(())
}
