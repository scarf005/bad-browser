use crate::types::*;
use crate::utils::log_msg;
use crate::video::VideoEngine;
use crate::web::WebEngine;
use crossterm::event::{KeyCode, KeyModifiers};
use rand::prelude::IndexedRandom;
use reqwest::Url;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

pub struct App {
    pub mode: AppMode,
    pub previous_mode: AppMode,
    pub render_mode: RenderMode,

    web: WebEngine,
    rx: Receiver<BgEvent>,
    pub is_loading: bool,

    pub prefetch_data: Option<BgEvent>,

    pub current_url: String,
    pub url_input: String,
    pub cursor_pos: usize,
    pub page_text: String,
    pub dense_text: Vec<char>,

    pub link_map: HashMap<String, String>,
    pub hint_buffer: String,
    pub hint_mode_active: bool,
    pub valid_links: Vec<String>,

    pub history: Vec<String>,
    pub history_index: usize,
    pub scroll_y: u16,

    pub auto_scroll: AutoScroll,
    pub scroll_speed_multiplier: f32,
    pub last_scroll_tick: Instant,

    pub engine: VideoEngine,
}

impl App {
    pub fn new(video_path: String, start_url: String) -> Self {
        let _ = std::fs::write("badlynx.log", "");
        log_msg("info", "App initialized");

        let (tx, rx) = mpsc::channel();
        let web = WebEngine::new(tx.clone());
        let engine = VideoEngine::new(video_path, tx);

        log_msg("info", &format!("Video Duration: {:.2}s", engine.duration));

        let mut app = Self {
            mode: AppMode::Normal,
            previous_mode: AppMode::Normal,
            render_mode: RenderMode::Cast,
            web,
            rx,
            is_loading: false,
            prefetch_data: None,
            current_url: start_url.clone(),
            url_input: start_url.clone(),
            cursor_pos: start_url.len(),
            page_text: String::new(),
            dense_text: Vec::new(),
            link_map: HashMap::new(),
            hint_buffer: String::new(),
            hint_mode_active: false,
            valid_links: Vec::new(),
            history: vec![],
            history_index: 0,
            scroll_y: 0,
            auto_scroll: AutoScroll::Off,
            scroll_speed_multiplier: 1.0,
            last_scroll_tick: Instant::now(),
            engine,
        };

        app.trigger_fetch(start_url, false, false);
        app
    }

    pub fn trigger_fetch(&mut self, url: String, is_prefetch: bool, is_history: bool) {
        if !is_prefetch {
            self.is_loading = true;
            self.url_input = url.clone();
            self.cursor_pos = self.url_input.len();
        }
        self.web.fetch(&self.current_url, url, is_prefetch, is_history);
    }

    pub fn handle_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                BgEvent::PageLoaded {
                    url,
                    text,
                    dense_text,
                    link_map,
                    links,
                    is_history_nav,
                } => {
                    log_msg("info", "Page Loaded");
                    self.is_loading = false;
                    self.current_url = url.clone();
                    self.url_input = url.clone();
                    self.cursor_pos = self.url_input.len();
                    self.page_text = text;
                    self.dense_text = dense_text;
                    self.link_map = link_map;
                    self.valid_links = links;
                    self.scroll_y = 0;

                    if !is_history_nav {
                        if self.history.last() != Some(&url) {
                            self.history.truncate(self.history_index + 1);
                            self.history.push(url);
                            self.history_index = self.history.len() - 1;
                        }
                    }

                    self.prefetch_data = None;
                    if self.auto_scroll == AutoScroll::RandomWalk {
                        self.trigger_random_prefetch();
                    }
                }
                BgEvent::PrefetchReady { .. } => {
                    self.prefetch_data = Some(event);
                }
                BgEvent::VideoEnded(id) => {
                    if self.mode == AppMode::Video && id == self.engine.session_id {
                        log_msg("info", "Video Ended Naturally");
                        self.stop_video();
                        self.auto_scroll = AutoScroll::Off;
                        self.mode = AppMode::Normal;
                    }
                }
                BgEvent::Error(e) => {
                    log_msg("error", &format!("{}", e));
                    self.is_loading = false;
                    self.page_text = format!("Error: {}", e);
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyCode, modifiers: KeyModifiers, term_h: u16, term_w: u16) -> bool {
        match self.mode {
            AppMode::Insert => self.handle_insert(key, modifiers),
            _ => {
                if self.hint_mode_active {
                    match key {
                        KeyCode::Esc => {
                            self.hint_mode_active = false;
                            self.hint_buffer.clear();
                        }
                        KeyCode::Backspace => {
                            self.hint_buffer.pop();
                        }
                        KeyCode::Char(c) => {
                            self.hint_buffer.push(c);
                            if let Some(url) = self.link_map.get(&self.hint_buffer) {
                                let u = url.clone();
                                self.hint_mode_active = false;
                                self.hint_buffer.clear();
                                self.trigger_fetch(u, false, false);
                            } else if self.hint_buffer.len() >= 2 {
                                self.hint_buffer.clear();
                                self.hint_mode_active = false;
                            }
                        }
                        _ => {}
                    }
                    return false;
                }

                match key {
                    KeyCode::Char('q') => {
                        if self.mode == AppMode::Video {
                            self.stop_video();
                        } else {
                            return true;
                        }
                    }
                    KeyCode::Char('i') => {
                        self.previous_mode = self.mode;
                        self.mode = AppMode::Insert;
                    }
                    KeyCode::Char('p') => {
                        let is_running = self.engine.current_stopper.is_some();
                        if self.mode == AppMode::Video && is_running {
                            self.stop_video();
                        } else if !is_running {
                            self.engine.start(term_w as usize, term_h as usize, 0.0);
                            self.mode = AppMode::Video;
                        }
                    }
                    KeyCode::Char(' ') if self.mode == AppMode::Video => self.engine.toggle_pause(),

                    KeyCode::Char('m') => {
                        self.render_mode = match self.render_mode {
                            RenderMode::Cast => RenderMode::Fit,
                            RenderMode::Fit => RenderMode::Cast,
                        };
                        log_msg(
                            "info",
                            &format!("Render mode changed to {:?}", self.render_mode),
                        );
                    }

                    KeyCode::Char('f') => self.hint_mode_active = true,
                    KeyCode::Char('s') => {
                        self.auto_scroll = match self.auto_scroll {
                            AutoScroll::Off => AutoScroll::Linear,
                            AutoScroll::Linear => AutoScroll::RandomWalk,
                            AutoScroll::RandomWalk => AutoScroll::Off,
                        };
                    }

                    KeyCode::Left if self.mode == AppMode::Video => {
                        self.engine.seek(-5.0, term_w as usize, term_h as usize);
                    }
                    KeyCode::Right if self.mode == AppMode::Video => {
                        self.engine.seek(5.0, term_w as usize, term_h as usize);
                    }

                    KeyCode::Up => {
                        if self.auto_scroll != AutoScroll::Off {
                            self.scroll_speed_multiplier = (self.scroll_speed_multiplier + 0.25).min(3.0);
                        } else {
                            self.scroll_y = self.scroll_y.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if self.auto_scroll != AutoScroll::Off {
                            self.scroll_speed_multiplier = (self.scroll_speed_multiplier - 0.25).max(0.5);
                        } else {
                            self.scroll_down(term_h);
                        }
                    }

                    KeyCode::Char('j') => self.scroll_down(term_h),
                    KeyCode::Char('k') => self.scroll_y = self.scroll_y.saturating_sub(1),
                    KeyCode::PageDown => self.scroll_down_pg(term_h),
                    KeyCode::PageUp => self.scroll_y = self.scroll_y.saturating_sub(10),

                    KeyCode::Char('h') => {
                        if self.history_index > 0 {
                            self.history_index -= 1;
                            let u = self.history[self.history_index].clone();
                            self.trigger_fetch(u, false, true);
                        }
                    }
                    KeyCode::Char('l') => {
                        if self.history_index + 1 < self.history.len() {
                            self.history_index += 1;
                            let u = self.history[self.history_index].clone();
                            self.trigger_fetch(u, false, true);
                        }
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn handle_insert(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match key {
            KeyCode::Enter => {
                self.mode = self.previous_mode;
                let u = self.url_input.clone();
                self.trigger_fetch(u, false, false);
            }
            KeyCode::Esc => self.mode = self.previous_mode,
            KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => self.delete_word(),
            KeyCode::Backspace => {
                if modifiers.contains(KeyModifiers::ALT) {
                    self.delete_word();
                } else if self.cursor_pos > 0 {
                    self.url_input.remove(self.cursor_pos - 1);
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.url_input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Left => self.cursor_pos = self.cursor_pos.saturating_sub(1),
            KeyCode::Right => self.cursor_pos = (self.cursor_pos + 1).min(self.url_input.len()),
            _ => {}
        }
    }

    fn delete_word(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let prefix = &self.url_input[..self.cursor_pos];
        let new_pos = prefix.trim_end().rfind(' ').map(|i| i + 1).unwrap_or(0);
        self.url_input.replace_range(new_pos..self.cursor_pos, "");
        self.cursor_pos = new_pos;
    }

    pub fn scroll_down(&mut self, term_h: u16) {
        self.scroll_y = self.scroll_y.saturating_add(1);
        self.check_random_walk_trigger(term_h);
    }

    fn scroll_down_pg(&mut self, term_h: u16) {
        self.scroll_y = self.scroll_y.saturating_add(10);
        self.check_random_walk_trigger(term_h);
    }

    fn check_random_walk_trigger(&mut self, term_h: u16) {
        if self.auto_scroll == AutoScroll::RandomWalk {
            let lines = self.page_text.lines().count();
            if (self.scroll_y as usize) + (term_h as usize) >= lines.saturating_sub(2) {
                if !self.apply_prefetch() {
                    self.trigger_random_prefetch();
                }
            }
        }
    }

    pub fn stop_video(&mut self) {
        self.mode = AppMode::Normal;
        self.engine.stop();
    }

    fn trigger_random_prefetch(&mut self) {
        let current_host = Url::parse(&self.current_url)
            .ok()
            .and_then(|u| u.host_str().map(|s| s.to_string()));

        let filtered_links: Vec<&String> = self
            .valid_links
            .iter()
            .filter(|link| {
                if let Some(ref host) = current_host {
                    if let Ok(u) = Url::parse(link) {
                        u.host_str() == Some(host)
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .collect();

        if let Some(link) = filtered_links.choose(&mut rand::rng()) {
            self.trigger_fetch((*link).clone(), true, false);
        } else if let Some(link) = self.valid_links.choose(&mut rand::rng()) {
            self.trigger_fetch(link.clone(), true, false);
        }
    }

    fn apply_prefetch(&mut self) -> bool {
        if let Some(BgEvent::PrefetchReady {
            url,
            text,
            dense_text,
            link_map,
            links,
        }) = self.prefetch_data.take()
        {
            self.current_url = url.clone();
            self.url_input = url;
            self.page_text = text;
            self.dense_text = dense_text;
            self.link_map = link_map;
            self.valid_links = links;
            self.scroll_y = 0;
            self.history.push(self.current_url.clone());
            self.history_index = self.history.len() - 1;

            self.trigger_random_prefetch();
            return true;
        }
        false
    }
}
