mod app;
mod i18n;
mod text;
mod types;
mod ui;
mod utils;
mod video;
mod web;

rust_i18n::i18n!("locales");

use anyhow::Result;
use app::App;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use std::io;
use std::time::Duration;
use types::{AutoScroll, ScriptEntry};

const DEFAULT_URL: &str = "https://en.touhouwiki.net/wiki/Bad_Apple!!";

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long, default_value = "bad_apple.mp4")]
    video: String,
    #[arg(long, default_value = DEFAULT_URL)]
    start_url: String,
    #[arg(long)]
    demo: Option<String>,
    #[arg(long, env = "BAD_BROWSER_LOCALE")]
    lang: Option<String>,
}

fn parse_demo(path: &str) -> Result<Vec<ScriptEntry>> {
    let content = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "Line {actual}: Expected format 'timestamp URL', got: {line}",
                actual = line_num + 1,
            );
        }

        let timestamp = parse_timestamp(parts[0]).map_err(|e| {
            anyhow::anyhow!(
                "Line {actual}: Failed to parse timestamp '{t}': {e}",
                actual = line_num + 1,
                t = parts[0],
            )
        })?;
        let url = parts[1].trim().to_string();

        entries.push(ScriptEntry { timestamp, url });
    }

    entries.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap());
    Ok(entries)
}

fn parse_timestamp(s: &str) -> Result<f64> {
    // Try MM:SS.ms format first
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let minutes: f64 = parts[0].parse()?;
            let seconds: f64 = parts[1].parse()?;
            return Ok(minutes * 60.0 + seconds);
        }
    }
    // Fall back to plain seconds
    Ok(s.parse()?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    i18n::init_locale(cli.lang.as_deref());

    let demo = if let Some(demo_path) = &cli.demo {
        parse_demo(demo_path)?
    } else {
        Vec::new()
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = App::new(cli.video, cli.start_url, demo);

    loop {
        app.handle_events();
        app.check_demo_transitions();

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
