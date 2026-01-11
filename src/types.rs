use std::collections::HashMap;

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum AppMode {
    Normal,
    Insert,
    Video,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum AutoScroll {
    Off,
    Linear,
    RandomWalk,
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum RenderMode {
    Cast,
    Fit,
}

#[derive(Clone, Debug)]
pub enum BgEvent {
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
