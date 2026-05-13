mod app;
mod config;
mod error;
mod event;
mod gdoc;
mod glyphs;
mod hydrate;
mod scanner;
mod store;
mod sync;
mod ui;

use std::io;
use std::panic;
use std::time::Duration;

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
    // Power users can opt in with WM_MOUSE=1.
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = async_channel();
    let mut app = App::new(tx)?;
    if std::env::var_os("WM_MOUSE").is_some() {
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

        if let Some(path) = app.pending_editor.take() {
            suspend_and_edit(&mut terminal, &mut app, &path)?;
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
