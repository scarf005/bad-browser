use crate::app::App;
use crate::i18n::t;
use crate::text::clamp_cursor;
use crate::types::{AppMode, AutoScroll, RenderMode};
use crate::utils::decode_url;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let area = chunks[0];

    if app.mode == AppMode::Video {
        render_video_mask(f, app, area);
    } else {
        let p = Paragraph::new(app.page_text.as_ref().as_str())
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_y, 0));
        f.render_widget(p, area);
    }

    render_status_bar(f, app, chunks[1]);
    render_hints(f, app, chunks[2]);
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let (bg, txt) = match app.mode {
        AppMode::Normal => {
            if app.hint_mode_active {
                (Color::Magenta, format!(" {} ", t!("status.hint")))
            } else {
                (Color::Blue, format!(" {} ", t!("status.normal")))
            }
        }
        AppMode::Insert => (Color::Yellow, format!(" {} ", t!("status.insert"))),
        AppMode::Video => {
            if app.engine.is_paused {
                (Color::Gray, format!(" {} ", t!("status.pause")))
            } else {
                (Color::Red, format!(" {} ", t!("status.video")))
            }
        }
    };

    let mut left_spans = vec![
        Span::styled(txt, Style::default().bg(bg).fg(Color::Black).bold()),
        Span::raw(" "),
    ];

    if app.hint_mode_active {
        left_spans.push(Span::styled(
            t!("status.goto_prefix", hint = app.hint_buffer),
            Style::default().fg(Color::Yellow).bold(),
        ));
    } else if app.mode == AppMode::Insert {
        let nice_input = decode_url(&app.url_input);
        let safe_cursor = clamp_cursor(&nice_input, app.cursor_pos);
        let (l, r) = nice_input.split_at(safe_cursor);

        left_spans.push(Span::raw(l.to_string()));
        left_spans.push(Span::styled("█", Style::default().fg(Color::White)));
        left_spans.push(Span::raw(r.to_string()));
    } else {
        left_spans.push(Span::raw(decode_url(&app.url_input)));
    }

    if app.is_loading {
        left_spans.push(Span::raw(" "));
        left_spans.push(Span::styled(
            "⏳",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::RAPID_BLINK),
        ));
    }

    let scroll_icon = match app.auto_scroll {
        AutoScroll::Off => "",
        AutoScroll::Linear => " [AUTO]",
        AutoScroll::RandomWalk => " [RAND]",
        AutoScroll::Demo => " [DEMO]",
    };
    left_spans.push(Span::styled(scroll_icon, Style::default().fg(Color::Green)));

    if app.auto_scroll != AutoScroll::Off {
        let scroll_speed_multiplier = app.scroll_speed_multiplier;
        left_spans.push(Span::styled(
            format!(" x{scroll_speed_multiplier:.2}"),
            Style::default().fg(Color::LightGreen),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).bg(Color::DarkGray),
        status_chunks[0],
    );

    let mut right_spans = Vec::new();

    if app.mode == AppMode::Video {
        let current = if app.engine.is_paused {
            app.engine.seek_time
        } else {
            app.engine.seek_time + app.engine.start_instant.elapsed().as_secs_f64()
        };

        let total = app.engine.duration;

        let progress_width = 13;
        let progress = if total > 0.0 {
            (current / total * progress_width as f64).round() as usize
        } else {
            0
        };
        let progress = progress.min(progress_width);

        let filled = "━".repeat(progress);
        let empty = " ".repeat(progress_width.saturating_sub(progress));
        let bar = format!("[{filled}{empty}]");

        right_spans.push(Span::styled(bar, Style::default().fg(Color::Green)));

        let time_str = format!(
            "[{:02}:{:02}/{:02}:{:02}] ",
            (current as u64) / 60,
            (current as u64) % 60,
            (total as u64) / 60,
            (total as u64) % 60
        );
        right_spans.push(Span::styled(time_str, Style::default().fg(Color::Cyan)));
    }

    if app.mode == AppMode::Video && !app.demo.is_empty() {
        let (autoplay_text, autoplay_color) = if app.autoplay {
            (format!("[{}] ", t!("labels.autoplay_on")), Color::Green)
        } else {
            (format!("[{}] ", t!("labels.autoplay_off")), Color::Red)
        };
        right_spans.push(Span::styled(
            autoplay_text,
            Style::default().fg(autoplay_color).bold(),
        ));
    }

    let render_txt = match app.render_mode {
        RenderMode::Cast => "[CST]",
        RenderMode::Fit => "[FIT]",
    };

    right_spans.push(Span::styled(
        t!("labels.mode_toggle"),
        Style::default().fg(Color::Yellow),
    ));

    let render_style = if app.mode == AppMode::Video {
        Style::default().fg(Color::Magenta).bold()
    } else {
        Style::default().fg(Color::Gray)
    };
    right_spans.push(Span::styled(render_txt, render_style));

    f.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(Alignment::Right)
            .bg(Color::DarkGray),
        status_chunks[1],
    );
}

fn render_hints(f: &mut Frame, app: &App, area: Rect) {
    let hints_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    let hints = match app.mode {
        AppMode::Insert => t!("hints.insert"),
        AppMode::Video => t!("hints.video"),
        _ => {
            if app.hint_mode_active {
                t!("hints.link_typing")
            } else if !app.demo.is_empty() {
                t!("hints.demo")
            } else {
                t!("hints.normal")
            }
        }
    };
    f.render_widget(
        Paragraph::new(hints).bg(Color::Black).fg(Color::Gray),
        hints_chunks[0],
    );

    if app.mode == AppMode::Video && !app.demo.is_empty() {
        f.render_widget(
            Paragraph::new(t!("labels.autoplay_hint"))
                .alignment(Alignment::Right)
                .bg(Color::Black)
                .fg(Color::Gray),
            hints_chunks[1],
        );
    }
}

fn render_video_mask(f: &mut Frame, app: &App, area: Rect) {
    let (buf, src_w, src_h) = {
        let b = app.engine.buffer.lock().unwrap();
        let w = *app.engine.source_width.lock().unwrap();
        let h = *app.engine.source_height.lock().unwrap();
        if b.len() == 0 {
            f.render_widget(Paragraph::new(t!("ui.buffering")), area);
            return;
        }
        (b.clone(), w, h)
    };

    let term_w = area.width as usize;
    let term_h = area.height as usize;

    let scale_w = term_w as f64 / src_w as f64;
    let scale_h = term_h as f64 / src_h as f64;
    let scale = scale_w.min(scale_h);

    let draw_w = (src_w as f64 * scale) as usize;
    let draw_h = (src_h as f64 * scale) as usize;

    let off_x = (term_w.saturating_sub(draw_w)) / 2;
    let off_y = (term_h.saturating_sub(draw_h)) / 2;

    let mut lines = Vec::with_capacity(term_h);
    let scroll_offset = (app.scroll_y as usize) * term_w;
    let mut text_idx = scroll_offset % app.dense_text.len().max(1);

    for y in 0..term_h {
        let mut spans = Vec::with_capacity(term_w);
        let mut x = 0;

        while x < term_w {
            let inside_video = x >= off_x && x < off_x + draw_w && y >= off_y && y < off_y + draw_h;

            if !inside_video {
                let ch = app.dense_text[text_idx];
                let w = UnicodeWidthChar::width(ch).unwrap_or(1);
                if x + w <= term_w {
                    spans.push(Span::styled(
                        ch.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                x += w;
                text_idx = (text_idx + 1) % app.dense_text.len().max(1);
                continue;
            }

            let src_x = ((x - off_x) * src_w) / draw_w;
            let src_y = ((y - off_y) * src_h) / draw_h;

            let sx = src_x.min(src_w - 1);
            let sy = src_y.min(src_h - 1);
            let pixel_idx = (sy * src_w + sx).min(buf.len() - 1);
            let brightness = buf[pixel_idx];

            let ch = app.dense_text[text_idx];
            let w = UnicodeWidthChar::width(ch).unwrap_or(1);

            if x + w <= term_w {
                let (fg, bg, modifier) = match brightness {
                    0..=30 => (Color::Black, Color::Black, Modifier::empty()),
                    31..=100 => (Color::DarkGray, Color::Black, Modifier::DIM),
                    101..=200 => (Color::White, Color::Black, Modifier::empty()),
                    201..=255 => (Color::Black, Color::White, Modifier::BOLD),
                };

                match app.render_mode {
                    RenderMode::Cast => {
                        if bg == Color::Black && fg == Color::Black {
                            spans.push(Span::raw(" ".repeat(w)));
                        } else {
                            spans.push(Span::styled(
                                ch.to_string(),
                                Style::default().fg(fg).bg(bg).add_modifier(modifier),
                            ));
                        }
                        text_idx = (text_idx + 1) % app.dense_text.len().max(1);
                    }
                    RenderMode::Fit => {
                        if brightness > 50 {
                            spans.push(Span::styled(
                                ch.to_string(),
                                Style::default().fg(fg).bg(bg).add_modifier(modifier),
                            ));
                            text_idx = (text_idx + 1) % app.dense_text.len().max(1);
                        } else {
                            spans.push(Span::raw(" ".repeat(w)));
                        }
                    }
                }
            }
            x += w;
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), area);
}
