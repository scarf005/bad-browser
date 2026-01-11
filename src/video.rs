use crate::types::BgEvent;
use crate::utils::log_msg;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct VideoEngine {
    pub buffer: Arc<Mutex<Vec<u8>>>,
    pub source_width: Arc<Mutex<usize>>,
    pub source_height: Arc<Mutex<usize>>,

    pub current_stopper: Option<Arc<AtomicBool>>,
    pub pause_signal: Arc<AtomicBool>,

    pub audio_process: Option<Child>,
    pub ffmpeg_process: Option<Child>,

    pub seek_time: f64,
    pub duration: f64,
    pub start_instant: Instant,
    pub session_id: usize,
    pub is_paused: bool,

    video_path: String,
    tx: std::sync::mpsc::SyncSender<BgEvent>,
}

impl VideoEngine {
    pub fn new(video_path: String, tx: std::sync::mpsc::SyncSender<BgEvent>) -> Self {
        let duration = Self::get_video_duration(&video_path).unwrap_or(0.0);
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            source_width: Arc::new(Mutex::new(100)),
            source_height: Arc::new(Mutex::new(50)),
            current_stopper: None,
            pause_signal: Arc::new(AtomicBool::new(false)),
            audio_process: None,
            ffmpeg_process: None,
            seek_time: 0.0,
            duration,
            start_instant: Instant::now(),
            session_id: 0,
            is_paused: false,
            video_path,
            tx,
        }
    }

    pub fn start(&mut self, term_w: usize, term_h: usize, seek_seconds: f64) {
        log_msg("info", "Starting Video...");
        self.stop_processes();

        self.session_id += 1;
        let current_session_id = self.session_id;

        let new_stopper = Arc::new(AtomicBool::new(false));
        self.current_stopper = Some(new_stopper.clone());

        self.is_paused = false;
        self.pause_signal.store(false, Ordering::Relaxed);

        self.spawn_audio(seek_seconds);

        self.seek_time = seek_seconds;
        self.start_instant = Instant::now();

        let buf = self.buffer.clone();
        let w = self.source_width.clone();
        let h = self.source_height.clone();
        let path = self.video_path.clone();
        let tx = self.tx.clone();
        let pause_sig = self.pause_signal.clone();

        *w.lock().unwrap() = term_w;
        *h.lock().unwrap() = term_h;

        {
            let mut lock = buf.lock().unwrap();
            *lock = vec![0u8; term_w * term_h];
        }

        let seek_str = format!("{seek_seconds:.2}");
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
                &format!("{term_w}x{term_h}"),
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
            self.ffmpeg_process = Some(child);

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
                log_msg("info", &format!("Video Thread {current_session_id} Ended"));
            });
        }
    }

    pub fn stop(&mut self) {
        self.stop_processes();
        self.seek_time = 0.0;
        self.is_paused = false;
        self.pause_signal.store(false, Ordering::Relaxed);
    }

    pub fn toggle_pause(&mut self) {
        self.is_paused = !self.is_paused;
        self.pause_signal.store(self.is_paused, Ordering::Relaxed);

        if self.is_paused {
            let elapsed = self.start_instant.elapsed().as_secs_f64();
            self.seek_time += elapsed;

            if let Some(child) = &self.audio_process {
                let pid = child.id().to_string();
                let status = Command::new("kill").arg("-STOP").arg(&pid).output();

                if status.is_err() || !status.unwrap().status.success() {
                    if let Some(mut c) = self.audio_process.take() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                }
            }
        } else {
            self.start_instant = Instant::now();

            let mut need_respawn = true;
            if let Some(child) = &self.audio_process {
                let pid = child.id().to_string();
                let status = Command::new("kill").arg("-CONT").arg(&pid).output();

                if status.is_ok() && status.unwrap().status.success() {
                    need_respawn = false;
                }
            }

            if need_respawn {
                self.spawn_audio(self.seek_time);
            }
        }
    }

    pub fn seek(&mut self, delta: f64, term_w: usize, term_h: usize) {
        let elapsed = if self.is_paused {
            0.0
        } else {
            self.start_instant.elapsed().as_secs_f64()
        };

        let current_real_time = self.seek_time + elapsed;
        let mut new_time = current_real_time + delta;
        if new_time < 0.0 {
            new_time = 0.0;
        }
        if self.duration > 0.0 && new_time > self.duration {
            new_time = self.duration - 1.0;
        }

        self.start(term_w, term_h, new_time);
    }

    fn stop_processes(&mut self) {
        log_msg("info", "Stopping Video Processes");

        if let Some(stopper) = self.current_stopper.take() {
            stopper.store(true, Ordering::Relaxed);
        }

        if let Some(mut child) = self.audio_process.take() {
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            // Ensure it's really dead
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }

        if let Some(mut child) = self.ffmpeg_process.take() {
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            // Ensure it's really dead
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }
    }

    fn spawn_audio(&mut self, seek_seconds: f64) {
        if let Some(mut old) = self.audio_process.take() {
            let pid = old.id();
            let _ = old.kill();
            let _ = old.wait();
            // Ensure it's really dead
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }

        let seek_str = format!("{seek_seconds:.2}");

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

        self.audio_process = child;
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
}
