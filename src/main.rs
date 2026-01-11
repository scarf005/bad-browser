mod app;
mod types;
mod ui;
mod utils;
mod video;
mod web;

use anyhow::Result;
use app::App;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;
use std::time::Duration;
use types::AutoScroll;

const DEFAULT_URL: &str = "https://en.touhouwiki.net/wiki/Bad_Apple!!";

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(default_value = "bad_apple.mp4")]
    video_path: String,
    #[arg(default_value = DEFAULT_URL)]
    start_url: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = App::new(cli.video_path, cli.start_url);

    loop {
        app.handle_events();

        terminal.draw(|f| ui::draw(f, &app))?;

        if app.auto_scroll != AutoScroll::Off {
            let base_speed_ms = 100.0;
            let effective_delay =
                Duration::from_secs_f32((base_speed_ms / app.scroll_speed_multiplier) / 1000.0);

            if app.last_scroll_tick.elapsed() >= effective_delay {
                let h = terminal.size()?.height;
                app.scroll_down(h);
                app.last_scroll_tick = std::time::Instant::now();
            }
        }

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let size = terminal.size()?;
                    let (h, w) = (size.height, size.width);
                    if app.on_key(key.code, key.modifiers, h, w) {
                        break;
                    }
                }
            }
        }
    }

    app.stop_video();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
