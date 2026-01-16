use crate::i18n::t;
use crate::text::{
    clamp_cursor, delete_next_grapheme, delete_prev_grapheme, delete_word, insert_grapheme,
    move_left_grapheme, move_right_grapheme, move_word_backward, move_word_forward,
};
use crate::types::*;
use crate::utils::log_msg;
use crate::video::VideoEngine;
use crate::web::WebEngine;
use crossterm::event::{KeyCode, KeyModifiers};
use rand::prelude::IndexedRandom;
use reqwest::Url;
use std::collections::HashMap;
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};
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
    pub page_text: Arc<String>,
    pub dense_text: Arc<Vec<char>>,

    pub link_map: Arc<HashMap<String, String>>,
    pub hint_buffer: String,
    pub hint_mode_active: bool,
    pub valid_links: Arc<Vec<String>>,

    pub history: Vec<String>,
    pub history_index: usize,
    pub scroll_y: u16,

    pub auto_scroll: AutoScroll,
    pub scroll_speed_multiplier: f32,
    pub last_scroll_tick: Instant,

    pub engine: VideoEngine,

    pub demo: Vec<ScriptEntry>,
    pub demo_index: usize,
    pub last_prefetch_index: Option<usize>,
    pub demo_cache: HashMap<
        String,
        (
            Arc<String>,
            Arc<Vec<char>>,
            Arc<HashMap<String, String>>,
            Arc<Vec<String>>,
        ),
    >,
}

impl App {
    pub fn new(video_path: String, start_url: String, demo: Vec<ScriptEntry>) -> Self {
        let _ = std::fs::write("bad-browser.log", "");
        log_msg("info", "App initialized");

        let (tx, rx) = mpsc::sync_channel(5);
        let web = WebEngine::new(tx.clone());
        let engine = VideoEngine::new(video_path, tx);

        let duration = engine.duration;
        log_msg("info", &format!("Video Duration: {duration:.2}s"));

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
            page_text: Arc::new(String::new()),
            dense_text: Arc::new(Vec::new()),
            link_map: Arc::new(HashMap::new()),
            hint_buffer: String::new(),
            hint_mode_active: false,
            valid_links: Arc::new(Vec::new()),
            history: vec![],
            history_index: 0,
            scroll_y: 0,
            auto_scroll: AutoScroll::Off,
            scroll_speed_multiplier: 1.0,
            last_scroll_tick: Instant::now(),
            engine,
            demo_index: 0,
            last_prefetch_index: None,
            demo_cache: HashMap::new(),
            demo,
        };

        app.trigger_fetch(start_url, false, false);

        // Preload ALL demo pages for instant transitions
        let demo_urls: Vec<String> = app.demo.iter().map(|e| e.url.clone()).collect();
        for url in demo_urls {
            app.trigger_fetch(url, true, false);
        }

        app
    }

    pub fn trigger_fetch(&mut self, url: String, is_prefetch: bool, is_history: bool) {
        if !is_prefetch {
            self.is_loading = true;
            self.url_input = url.clone();
            self.cursor_pos = self.url_input.len();
        }
        self.web
            .fetch(&self.current_url, url, is_prefetch, is_history);
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
                    self.page_text = Arc::new(text);
                    self.dense_text = Arc::new(dense_text);
                    self.link_map = Arc::new(link_map);
                    self.valid_links = Arc::new(links);
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
                BgEvent::PrefetchReady {
                    url,
                    text,
                    dense_text,
                    link_map,
                    links,
                } => {
                    // Store in demo cache if it's a demo URL
                    if self.demo.iter().any(|e| e.url == url) {
                        self.demo_cache.insert(
                            url,
                            (
                                Arc::new(text),
                                Arc::new(dense_text),
                                Arc::new(link_map),
                                Arc::new(links),
                            ),
                        );
                        log_msg("info", "Demo: Cached page");
                    } else {
                        self.prefetch_data = Some(BgEvent::PrefetchReady {
                            url,
                            text,
                            dense_text,
                            link_map,
                            links,
                        });
                    }
                }
                BgEvent::VideoEnded(id) => {
                    if self.mode == AppMode::Video && id == self.engine.session_id {
                        log_msg("info", "Video Ended Naturally");
                        self.stop_video();
                    }
                }
                BgEvent::Error(e) => {
                    log_msg("error", &format!("{e}"));
                    self.is_loading = false;
                    self.page_text = Arc::new(t!("errors.generic", error = e));
                }
            }
        }
    }

    pub fn on_key(
        &mut self,
        key: KeyCode,
        modifiers: KeyModifiers,
        term_h: u16,
        term_w: u16,
    ) -> bool {
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
                            if !self.demo.is_empty() {
                                self.auto_scroll = AutoScroll::Demo;
                                self.demo_index = 0;
                                log_msg(
                                    "info",
                                    &format!(
                                        "Demo: {} entries, {} cached",
                                        self.demo.len(),
                                        self.demo_cache.len()
                                    ),
                                );
                                // Force apply first page immediately
                                self.apply_demo_page(0);
                                self.demo_index = 1;
                            }
                        }
                    }
                    KeyCode::Char(' ') if self.mode == AppMode::Video => self.engine.toggle_pause(),

                    KeyCode::Char('m') => {
                        self.render_mode = match self.render_mode {
                            RenderMode::Cast => RenderMode::Fit,
                            RenderMode::Fit => RenderMode::Cast,
                        };
                        let render_mode = self.render_mode;
                        log_msg("info", &format!("Render mode changed to {render_mode:?}"));
                    }

                    KeyCode::Char('f') => self.hint_mode_active = true,
                    KeyCode::Char('s') => {
                        if self.demo.is_empty() {
                            self.auto_scroll = match self.auto_scroll {
                                AutoScroll::Off => AutoScroll::Linear,
                                AutoScroll::Linear => AutoScroll::RandomWalk,
                                AutoScroll::RandomWalk => AutoScroll::Off,
                                _ => AutoScroll::Off,
                            };
                        }
                    }

                    KeyCode::Left if self.mode == AppMode::Video => {
                        self.engine.seek(-5.0, term_w as usize, term_h as usize);
                        self.reset_demo_index();
                    }
                    KeyCode::Right if self.mode == AppMode::Video => {
                        self.engine.seek(5.0, term_w as usize, term_h as usize);
                        self.reset_demo_index();
                    }

                    KeyCode::Up => {
                        if self.auto_scroll != AutoScroll::Off {
                            self.scroll_speed_multiplier =
                                (self.scroll_speed_multiplier + 0.25).min(3.0);
                        } else {
                            self.scroll_y = self.scroll_y.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        if self.auto_scroll != AutoScroll::Off {
                            self.scroll_speed_multiplier =
                                (self.scroll_speed_multiplier - 0.25).max(0.5);
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
                    KeyCode::Char('r') => {
                        if !self.apply_prefetch() {
                            self.trigger_random_prefetch();
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
            KeyCode::Backspace => {
                if modifiers.contains(KeyModifiers::ALT) {
                    self.delete_word();
                } else {
                    delete_prev_grapheme(&mut self.url_input, &mut self.cursor_pos);
                }
            }
            KeyCode::Char('h')
                if modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::SHIFT) =>
            {
                // Ctrl+H is backspace on many terminals
                if modifiers.contains(KeyModifiers::ALT) {
                    self.delete_word();
                } else {
                    delete_prev_grapheme(&mut self.url_input, &mut self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                delete_next_grapheme(&mut self.url_input, &mut self.cursor_pos);
            }
            KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => self.delete_word(),
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                if self.cursor_pos > 0 {
                    self.url_input.drain(..self.cursor_pos);
                }
                self.cursor_pos = 0;
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                let cursor = clamp_cursor(&self.url_input, self.cursor_pos);
                self.url_input.truncate(cursor);
                self.cursor_pos = cursor;
            }
            KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_pos = 0;
            }
            KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_pos = self.url_input.len();
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.url_input.len(),
            KeyCode::Left => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    self.move_word_backward();
                } else {
                    move_left_grapheme(&self.url_input, &mut self.cursor_pos);
                }
            }
            KeyCode::Right => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    self.move_word_forward();
                } else {
                    move_right_grapheme(&self.url_input, &mut self.cursor_pos);
                }
            }
            KeyCode::Char(c) if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                insert_grapheme(&mut self.url_input, &mut self.cursor_pos, c);
            }
            _ => {}
        }
    }

    fn delete_word(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        delete_word(&mut self.url_input, &mut self.cursor_pos);
    }

    fn move_word_backward(&mut self) {
        move_word_backward(&self.url_input, &mut self.cursor_pos);
    }

    fn move_word_forward(&mut self) {
        move_word_forward(&self.url_input, &mut self.cursor_pos);
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
        self.demo_index = 0;
        self.last_prefetch_index = None;
        self.auto_scroll = AutoScroll::Off;
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
            self.cursor_pos = self.url_input.len();
            self.page_text = Arc::new(text);
            self.dense_text = Arc::new(dense_text);
            self.link_map = Arc::new(link_map);
            self.valid_links = Arc::new(links);
            self.scroll_y = 0;
            self.history.push(self.current_url.clone());
            self.history_index = self.history.len() - 1;

            self.trigger_random_prefetch();
            return true;
        }
        false
    }

    fn reset_demo_index(&mut self) {
        if self.demo.is_empty() {
            return;
        }

        let current_time = self.engine.seek_time;
        self.demo_index = self
            .demo
            .iter()
            .position(|e| e.timestamp > current_time)
            .unwrap_or(self.demo.len());
        self.last_prefetch_index = None;
    }

    fn apply_demo_page(&mut self, index: usize) {
        if index >= self.demo.len() {
            return;
        }

        let url = &self.demo[index].url;

        if let Some((text, dense_text, link_map, links)) = self.demo_cache.get(url) {
            self.current_url = url.clone();
            self.url_input = url.clone();
            self.cursor_pos = self.url_input.len();
            self.page_text = Arc::clone(text);
            self.dense_text = Arc::clone(dense_text);
            self.link_map = Arc::clone(link_map);
            self.valid_links = Arc::clone(links);
            self.scroll_y = 0;
        } else {
            log_msg(
                "warn",
                &format!("Demo: Page {index} not in cache yet: {url}"),
            );
        }
    }

    pub fn check_demo_transitions(&mut self) {
        if self.demo.is_empty() || self.mode != AppMode::Video {
            return;
        }

        let current_time = if self.engine.is_paused {
            self.engine.seek_time
        } else {
            self.engine.seek_time + self.engine.start_instant.elapsed().as_secs_f64()
        };

        if self.demo_index < self.demo.len() {
            let entry_timestamp = self.demo[self.demo_index].timestamp;

            if current_time >= entry_timestamp {
                self.apply_demo_page(self.demo_index);
                self.demo_index += 1;
            }
        }
    }
}
