use chrono::{DateTime, Local};
use percent_encoding::percent_decode_str;
use std::fs::OpenOptions;
use std::io::Write;

pub fn log_msg(level: &str, msg: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("bad-browser.log")
    {
        let now: DateTime<Local> = Local::now();
        let _ = writeln!(
            file,
            "time=\"{time}\" level={level} msg=\"{msg}\"",
            time = now.format("%Y-%m-%dT%H:%M:%S%z"),
        );
    }
}

pub fn decode_url(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().to_string()
}
