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

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = async_channel();
    let mut app = App::new(tx)?;

    // Main loop
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // Poll for UI events (key or mouse) with 50ms timeout.
        match event::poll_event(Duration::from_millis(50)) {
            Some(UiEvent::Key(key)) if key.kind == KeyEventKind::Press => app.handle_key(key),
            Some(UiEvent::Mouse(m)) => app.handle_mouse(m),
            _ => {}
        }

        // Drain async events
        while let Ok(event) = rx.try_recv() {
            app.handle_async_event(event);
        }

        if app.should_quit {
            break;
        }
    }

    // Cancel any outstanding background tasks before tearing down the runtime.
    app.abort_tasks();

    // Restore terminal
    io::stdout().execute(DisableMouseCapture)?;
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}
