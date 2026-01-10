use anyhow::Result;
use chrono::{DateTime, Local};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use percent_encoding::percent_decode_str;
use rand::prelude::IndexedRandom;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use regex::{Captures, Regex};
use reqwest::Url;
use reqwest::blocking::Client;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthChar;

// --- LOGGING (logfmt) ---
fn log_msg(level: &str, msg: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("badlynx.log")
    {
        let now: DateTime<Local> = Local::now();
        let _ = writeln!(
            file,
            "time=\"{time}\" level={level} msg=\"{msg}\"",
            time = now.format("%Y-%m-%dT%H:%M:%S%z"),
        );
    }
}

// --- CONFIG ---
const DEFAULT_URL: &str = "https://en.touhouwiki.net/wiki/Bad_Apple!!";
const USER_AGENT: &str = "bad-browser/1.0";

// --- CLI ---
#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(default_value = "bad_apple.mp4")]
    video_path: String,
    #[arg(default_value = DEFAULT_URL)]
    start_url: String,
}

// --- APP STATES ---
#[derive(PartialEq, Clone, Copy, Debug)]
enum AppMode {
    Normal,
    Insert,
    Video,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AutoScroll {
    Off,
    Linear,
    RandomWalk,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum RenderMode {
    Cast,
    Fit,
}

// --- THREAD MESSAGING ---
enum BgEvent {
    PageLoaded {
        url: String,
        text: String,
        links: Vec<String>,
        dense_text: Vec<char>,
        link_map: HashMap<String, String>,
        is_history_nav: bool,
    },
    PrefetchReady {
        url: String,
        text: String,
        links: Vec<String>,
        dense_text: Vec<char>,
        link_map: HashMap<String, String>,
    },
    VideoEnded(usize),
    Error(String),
}

// --- VIDEO ENGINE ---
struct VideoEngine {
    buffer: Arc<Mutex<Vec<u8>>>,
    source_width: Arc<Mutex<usize>>,
    source_height: Arc<Mutex<usize>>,

    // Thread Controls
    current_stopper: Option<Arc<AtomicBool>>,
    pause_signal: Arc<AtomicBool>,

    audio_process: Option<Child>,
    ffmpeg_process: Option<Child>,

    // State
    seek_time: f64,
    duration: f64,
    start_instant: Instant,
    session_id: usize,

    // Logic flag (UI state)
    is_paused: bool,
}

// --- MAIN APP STRUCT ---
struct App {
    mode: AppMode,
    previous_mode: AppMode,
    render_mode: RenderMode,

    client: Client,
    tx: mpsc::Sender<BgEvent>,
    rx: mpsc::Receiver<BgEvent>,
    is_loading: bool,

    prefetch_data: Option<BgEvent>,

    current_url: String,
    url_input: String,
    cursor_pos: usize,
    page_text: String,
    dense_text: Vec<char>,

    link_map: HashMap<String, String>,
    hint_buffer: String,
    hint_mode_active: bool,
    valid_links: Vec<String>,

    history: Vec<String>,
    history_index: usize,
    scroll_y: u16,

    auto_scroll: AutoScroll,
    scroll_speed_multiplier: f32,
    last_scroll_tick: Instant,

    video_path: String,
    engine: VideoEngine,
}

// --- UTILS ---
fn decode_url(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().to_string()
}

impl App {
    fn new(video_path: String, start_url: String) -> Self {
        let _ = std::fs::write("badlynx.log", "");
        log_msg("info", "App initialized");

        let duration = get_video_duration(&video_path).unwrap_or(0.0);
        log_msg("info", &format!("Video Duration: {:.2}s", duration));

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        let (tx, rx) = mpsc::channel();

        let mut app = Self {
            mode: AppMode::Normal,
            previous_mode: AppMode::Normal,
            render_mode: RenderMode::Cast,
            client: client.clone(),
            tx,
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
            video_path,
            engine: VideoEngine {
                buffer: Arc::new(Mutex::new(Vec::new())),
                source_width: Arc::new(Mutex::new(100)),
                source_height: Arc::new(Mutex::new(50)),
                current_stopper: None,
                pause_signal: Arc::new(AtomicBool::new(false)),
                audio_process: None,
                ffmpeg_process: None,
                is_paused: false,
                seek_time: 0.0,
                duration,
                start_instant: Instant::now(),
                session_id: 0,
            },
        };

        app.trigger_fetch(start_url, false, false);
        app
    }

    fn trigger_fetch(&mut self, url: String, is_prefetch: bool, is_history: bool) {
        if !is_prefetch {
            log_msg("info", &format!("Fetching URL: {}", url));
            self.is_loading = true;
            self.url_input = url.clone();
            self.cursor_pos = self.url_input.len();
        }

        let client = self.client.clone();
        let tx = self.tx.clone();

        let base =
            Url::parse(&self.current_url).unwrap_or_else(|_| Url::parse(DEFAULT_URL).unwrap());
        let target_url = match base.join(&url) {
            Ok(u) => u.to_string(),
            Err(_) => url.clone(),
        };

        thread::spawn(move || match client.get(&target_url).send() {
            Ok(resp) => {
                if resp.status().is_success() {
                    let html = resp.text().unwrap_or_default();
                    let (text, dense, map, links) = parse_html(&html);
                    let event = if is_prefetch {
                        BgEvent::PrefetchReady {
                            url: target_url,
                            text,
                            dense_text: dense,
                            link_map: map,
                            links,
                        }
                    } else {
                        BgEvent::PageLoaded {
                            url: target_url,
                            text,
                            dense_text: dense,
                            link_map: map,
                            links,
                            is_history_nav: is_history,
                        }
                    };
                    let _ = tx.send(event);
                } else if !is_prefetch {
                    let _ = tx.send(BgEvent::Error(format!("HTTP {}", resp.status())));
                }
            }
            Err(e) => {
                if !is_prefetch {
                    let _ = tx.send(BgEvent::Error(e.to_string()));
                }
            }
        });
    }

    fn check_background_events(&mut self) {
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

    fn handle_input(
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
                            self.start_video(term_w as usize, term_h as usize, 0.0);
                        }
                    }
                    KeyCode::Char(' ') if self.mode == AppMode::Video => self.toggle_pause(),

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
                        self.seek_video(-5.0, term_w as usize, term_h as usize);
                    }
                    KeyCode::Right if self.mode == AppMode::Video => {
                        self.seek_video(5.0, term_w as usize, term_h as usize);
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

    fn scroll_down(&mut self, term_h: u16) {
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

    fn start_video(&mut self, term_w: usize, term_h: usize, seek_seconds: f64) {
        log_msg("info", "Starting Video...");
        self.stop_video_processes_only();

        self.engine.session_id += 1;
        let current_session_id = self.engine.session_id;

        let new_stopper = Arc::new(AtomicBool::new(false));
        self.engine.current_stopper = Some(new_stopper.clone());

        self.engine.is_paused = false;
        self.engine.pause_signal.store(false, Ordering::Relaxed);

        self.spawn_audio(seek_seconds);

        self.engine.seek_time = seek_seconds;
        self.engine.start_instant = Instant::now();

        let buf = self.engine.buffer.clone();
        let w = self.engine.source_width.clone();
        let h = self.engine.source_height.clone();
        let path = self.video_path.clone();
        let tx = self.tx.clone();
        let pause_sig = self.engine.pause_signal.clone();

        *w.lock().unwrap() = term_w;
        *h.lock().unwrap() = term_h;

        {
            let mut lock = buf.lock().unwrap();
            *lock = vec![0u8; term_w * term_h];
        }

        let seek_str = format!("{:.2}", seek_seconds);
        // Video Process
        let ffmpeg_child = Command::new("ffmpeg")
            .args(&[
                "-ss",
                &seek_str,
                "-re",
                "-i",
                &path,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "-s",
                &format!("{}x{}", term_w, term_h),
                "-v",
                "quiet",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        if let Ok(mut child) = ffmpeg_child {
            log_msg("info", "FFmpeg process spawned");
            let mut stdout = child.stdout.take().unwrap();
            self.engine.ffmpeg_process = Some(child);

            thread::spawn(move || {
                let size = term_w * term_h;
                let mut frame = vec![0u8; size];
                let stopper = new_stopper;

                while !stopper.load(Ordering::Relaxed) {
                    if pause_sig.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }

                    match stdout.read_exact(&mut frame) {
                        Ok(_) => {
                            if stopper.load(Ordering::Relaxed) {
                                break;
                            }
                            if !pause_sig.load(Ordering::Relaxed) {
                                let mut lock = buf.lock().unwrap();
                                if lock.len() == size {
                                    lock.copy_from_slice(&frame);
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(_) => {
                            if !stopper.load(Ordering::Relaxed) {
                                let _ = tx.send(BgEvent::VideoEnded(current_session_id));
                            }
                            break;
                        }
                    }
                }
                log_msg(
                    "info",
                    &format!("Video Thread {} Ended", current_session_id),
                );
            });
        }

        self.mode = AppMode::Video;
    }

    // --- REVISED: Audio Process with Optimization Flags ---
    fn spawn_audio(&mut self, seek_seconds: f64) {
        if let Some(mut old) = self.engine.audio_process.take() {
            let _ = old.kill();
            let _ = old.wait();
        }

        let seek_str = format!("{:.2}", seek_seconds);

        // Flags to reduce probing delay and latency
        let child = Command::new("ffplay")
            .args(&[
                "-ss",
                &seek_str,
                "-nodisp",
                "-autoexit",
                "-hide_banner",
                "-loglevel",
                "panic",
                "-fflags",
                "nobuffer",
                "-flags",
                "low_delay",
                "-analyzeduration",
                "0",
                "-probesize",
                "32",
                &self.video_path,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();

        self.engine.audio_process = child;
    }

    // --- REVISED: Hybrid Pause Logic (SIGSTOP + Respawn Fallback) ---
    fn toggle_pause(&mut self) {
        self.engine.is_paused = !self.engine.is_paused;
        self.engine
            .pause_signal
            .store(self.engine.is_paused, Ordering::Relaxed);

        if self.engine.is_paused {
            // [Pause Action]
            let elapsed = self.engine.start_instant.elapsed().as_secs_f64();
            self.engine.seek_time += elapsed;

            if let Some(child) = &self.engine.audio_process {
                // Try SIGSTOP (Unix-only command, fails gracefully on Windows)
                let pid = child.id().to_string();
                let status = Command::new("kill").arg("-STOP").arg(&pid).output();

                if status.is_err() || !status.unwrap().status.success() {
                    // Fallback: Kill process if SIGSTOP failed (e.g. Windows)
                    if let Some(mut c) = self.engine.audio_process.take() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                }
                // If SIGSTOP success, we keep self.engine.audio_process Alive
            }
        } else {
            // [Resume Action]
            self.engine.start_instant = Instant::now();

            let mut need_respawn = true;
            if let Some(child) = &self.engine.audio_process {
                // Try SIGCONT
                let pid = child.id().to_string();
                let status = Command::new("kill").arg("-CONT").arg(&pid).output();

                if status.is_ok() && status.unwrap().status.success() {
                    need_respawn = false;
                }
            }

            if need_respawn {
                // If process was killed or SIGCONT failed, respawn at seek_time
                self.spawn_audio(self.engine.seek_time);
            }
        }
    }

    fn seek_video(&mut self, delta: f64, term_w: usize, term_h: usize) {
        // Calculate new time
        let elapsed = if self.engine.is_paused {
            0.0
        } else {
            self.engine.start_instant.elapsed().as_secs_f64()
        };

        let current_real_time = self.engine.seek_time + elapsed;
        let mut new_time = current_real_time + delta;
        if new_time < 0.0 {
            new_time = 0.0;
        }
        if self.engine.duration > 0.0 && new_time > self.engine.duration {
            new_time = self.engine.duration - 1.0;
        }

        // Seeking always requires respawn to jump correctly
        self.start_video(term_w, term_h, new_time);
    }

    fn stop_video_processes_only(&mut self) {
        log_msg("info", "Stopping Video Processes");

        if let Some(stopper) = self.engine.current_stopper.take() {
            stopper.store(true, Ordering::Relaxed);
        }

        if let Some(mut child) = self.engine.audio_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(mut child) = self.engine.ffmpeg_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn stop_video(&mut self) {
        self.mode = AppMode::Normal;
        self.stop_video_processes_only();
        self.engine.seek_time = 0.0;
        self.engine.is_paused = false;
        self.engine.pause_signal.store(false, Ordering::Relaxed);
    }
}

fn get_video_duration(path: &str) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args(&[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;
    let s = String::from_utf8(output.stdout).ok()?;
    s.trim().parse::<f64>().ok()
}

fn parse_html(html: &str) -> (String, Vec<char>, HashMap<String, String>, Vec<String>) {
    let mut hint_gen = (0..).map(|i| {
        let a = (b'a' + (i % 26)) as char;
        let b = (b'a' + (i / 26)) as char;
        format!("{}{}", b, a)
    });

    let mut link_map = HashMap::new();
    let mut valid_links = Vec::new();
    let link_regex = Regex::new(r#"(?i)<a[^>]+href=["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap();

    let injected = link_regex.replace_all(html, |caps: &Captures| {
        let raw_href = caps[1].to_string();
        let raw_text = &caps[2];
        let key = hint_gen.next().unwrap();

        link_map.insert(key.clone(), raw_href.clone());
        valid_links.push(raw_href.clone());

        let display_href = decode_url(&raw_href);
        format!(
            r#"<a href="{}">{} [{}][{}]</a>"#,
            raw_href, raw_text, display_href, key
        )
    });

    let text = html2text::from_read(injected.as_bytes(), 120).unwrap_or_default();
    let dense: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();

    (text, dense, link_map, valid_links)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(cli.video_path, cli.start_url);

    loop {
        app.check_background_events();

        terminal.draw(|f| ui(f, &mut app))?;

        if app.auto_scroll != AutoScroll::Off {
            let base_speed_ms = 100.0;
            let effective_delay =
                Duration::from_secs_f32((base_speed_ms / app.scroll_speed_multiplier) / 1000.0);

            if app.last_scroll_tick.elapsed() >= effective_delay {
                let h = terminal.size()?.height;
                app.scroll_down(h);
                app.last_scroll_tick = Instant::now();
            }
        }

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let size = terminal.size()?;
                    let (h, w) = (size.height, size.width);
                    if app.handle_input(key.code, key.modifiers, h, w) {
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

fn ui(f: &mut ratatui::Frame, app: &mut App) {
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
        let p = Paragraph::new(app.page_text.clone())
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_y, 0));
        f.render_widget(p, area);
    }

    // --- STATUS BAR ---
    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(chunks[1]);

    let (bg, txt) = match app.mode {
        AppMode::Normal => {
            if app.hint_mode_active {
                (Color::Magenta, " HINT ")
            } else {
                (Color::Blue, " NOR ")
            }
        }
        AppMode::Insert => (Color::Yellow, " INS "),
        AppMode::Video => {
            if app.engine.is_paused {
                (Color::Gray, " PAUSE ")
            } else {
                (Color::Red, " VID ")
            }
        }
    };

    let mut left_spans = vec![
        Span::styled(txt, Style::default().bg(bg).fg(Color::Black).bold()),
        Span::raw(" "),
    ];

    if app.hint_mode_active {
        left_spans.push(Span::styled(
            format!("GOTO: {}", app.hint_buffer),
            Style::default().fg(Color::Yellow).bold(),
        ));
    } else if app.mode == AppMode::Insert {
        let nice_input = decode_url(&app.url_input);
        let safe_cursor = app.cursor_pos.min(nice_input.len());
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
    };
    left_spans.push(Span::styled(scroll_icon, Style::default().fg(Color::Green)));

    if app.auto_scroll != AutoScroll::Off {
        left_spans.push(Span::styled(
            format!(" x{:.2}", app.scroll_speed_multiplier),
            Style::default().fg(Color::LightGreen),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(left_spans)).bg(Color::DarkGray),
        status_chunks[0],
    );

    // Right Side Info
    let mut right_spans = Vec::new();

    if app.mode == AppMode::Video {
        let current = if app.engine.is_paused {
            app.engine.seek_time
        } else {
            app.engine.seek_time + app.engine.start_instant.elapsed().as_secs_f64()
        };

        let total = app.engine.duration;
        let time_str = format!(
            "[{:02}:{:02}/{:02}:{:02}] ",
            (current as u64) / 60,
            (current as u64) % 60,
            (total as u64) / 60,
            (total as u64) % 60
        );
        right_spans.push(Span::styled(time_str, Style::default().fg(Color::Cyan)));
    }

    let render_txt = match app.render_mode {
        RenderMode::Cast => "[CST]",
        RenderMode::Fit => "[FIT]",
    };

    right_spans.push(Span::styled(
        " [m] Mode ",
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

    let hints = match app.mode {
        AppMode::Insert => "[Enter] Fetch  [Esc] Cancel",
        AppMode::Video => "[Space] Pause [q] Quit [Left/Right] Seek",
        _ => {
            if app.hint_mode_active {
                "Type keys..."
            } else {
                "[i] URL  [f] Link  [p] Play  [s] AutoScroll  [j/k] Scroll  [h/l] History [Up/Down] Speed [m] Mode"
            }
        }
    };
    f.render_widget(
        Paragraph::new(hints).bg(Color::Black).fg(Color::Gray),
        chunks[2],
    );
}

fn render_video_mask(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let (buf, src_w, src_h) = {
        let b = app.engine.buffer.lock().unwrap();
        let w = *app.engine.source_width.lock().unwrap();
        let h = *app.engine.source_height.lock().unwrap();
        if b.len() == 0 {
            f.render_widget(Paragraph::new("Buffering..."), area);
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
