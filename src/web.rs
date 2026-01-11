use crate::types::BgEvent;
use crate::utils::{decode_url, log_msg};
use regex::{Captures, Regex};
use reqwest::blocking::Client;
use reqwest::Url;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

const USER_AGENT: &str = "bad-browser/1.0";

pub struct WebEngine {
    client: Client,
    tx: Sender<BgEvent>,
}

impl WebEngine {
    pub fn new(tx: Sender<BgEvent>) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        Self { client, tx }
    }

    pub fn fetch(
        &self,
        current_url: &str,
        target: String,
        is_prefetch: bool,
        is_history: bool,
    ) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        let base_str = current_url.to_string();

        thread::spawn(move || {
            let base = Url::parse(&base_str).ok();
            let target_url = match base {
                Some(b) => b
                    .join(&target)
                    .map(|u| u.to_string())
                    .unwrap_or(target),
                None => target,
            };

            if !is_prefetch {
                log_msg("info", &format!("Fetching URL: {}", target_url));
            }

            match client.get(&target_url).send() {
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
            }
        });
    }
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
