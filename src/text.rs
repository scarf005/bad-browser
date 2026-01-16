use unicode_segmentation::UnicodeSegmentation;

pub fn prev_grapheme_boundary(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    text.grapheme_indices(true)
        .take_while(|(i, _)| *i < pos)
        .map(|(i, _)| i)
        .last()
        .unwrap_or(0)
}

pub fn next_grapheme_boundary(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    if pos >= text.len() {
        return text.len();
    }

    let mut iter = text.grapheme_indices(true).skip_while(|(i, _)| *i < pos);
    if let Some((idx, _)) = iter.next() {
        if idx == pos {
            if let Some((next, _)) = iter.next() {
                return next;
            }
        } else {
            return idx;
        }
    }

    text.len()
}

pub fn delete_prev_grapheme(text: &mut String, cursor: &mut usize) -> bool {
    if *cursor == 0 {
        return false;
    }
    let prev = prev_grapheme_boundary(text, *cursor);
    text.drain(prev..*cursor);
    *cursor = prev;
    true
}

pub fn delete_next_grapheme(text: &mut String, cursor: &mut usize) -> bool {
    if *cursor >= text.len() {
        return false;
    }
    let next = next_grapheme_boundary(text, *cursor);
    text.drain(*cursor..next);
    true
}

pub fn insert_grapheme(text: &mut String, cursor: &mut usize, c: char) {
    text.insert(*cursor, c);
    *cursor += c.len_utf8();
}

pub fn move_left_grapheme(text: &str, cursor: &mut usize) {
    *cursor = prev_grapheme_boundary(text, *cursor);
}

pub fn move_right_grapheme(text: &str, cursor: &mut usize) {
    *cursor = next_grapheme_boundary(text, *cursor);
}

pub fn delete_word(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }

    let mut end = *cursor;
    while end > 0 {
        let prev = prev_grapheme_boundary(text, end);
        if text[prev..end].chars().all(char::is_whitespace) {
            end = prev;
        } else {
            break;
        }
    }

    let mut start = end;
    while start > 0 {
        let prev = prev_grapheme_boundary(text, start);
        if text[prev..start].chars().all(|c| !c.is_whitespace()) {
            start = prev;
        } else {
            break;
        }
    }

    text.drain(start..*cursor);
    *cursor = start;
}

pub fn move_word_backward(text: &str, cursor: &mut usize) {
    let mut pos = (*cursor).min(text.len());

    while pos > 0 {
        let prev = prev_grapheme_boundary(text, pos);
        if text[prev..pos].chars().all(char::is_whitespace) {
            pos = prev;
        } else {
            break;
        }
    }

    while pos > 0 {
        let prev = prev_grapheme_boundary(text, pos);
        if text[prev..pos].chars().all(|c| !c.is_whitespace()) {
            pos = prev;
        } else {
            break;
        }
    }

    *cursor = pos;
}

pub fn move_word_forward(text: &str, cursor: &mut usize) {
    let mut pos = (*cursor).min(text.len());
    let len = text.len();

    while pos < len {
        let next = next_grapheme_boundary(text, pos);
        if text[pos..next].chars().all(|c| !c.is_whitespace()) {
            pos = next;
        } else {
            break;
        }
    }

    while pos < len {
        let next = next_grapheme_boundary(text, pos);
        if text[pos..next].chars().all(char::is_whitespace) {
            pos = next;
        } else {
            break;
        }
    }

    *cursor = pos;
}

pub fn clamp_cursor(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    if text.is_empty() {
        return 0;
    }
    if text.is_char_boundary(pos) {
        pos
    } else {
        prev_grapheme_boundary(text, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_multibyte_insertion_and_backspace() {
        let mut text = "https://".to_string();
        let mut cursor = text.len();

        insert_grapheme(&mut text, &mut cursor, '한');
        insert_grapheme(&mut text, &mut cursor, '국');
        assert_eq!(text, "https://한국");
        assert_eq!(cursor, "https://한국".len());

        assert!(delete_prev_grapheme(&mut text, &mut cursor));
        assert_eq!(text, "https://한");
        assert_eq!(cursor, "https://한".len());

        assert!(delete_prev_grapheme(&mut text, &mut cursor));
        assert_eq!(text, "https://");
        assert_eq!(cursor, "https://".len());
    }

    #[test]
    fn moves_across_graphemes_safely() {
        let text = "テスト 日本";
        let mut cursor = text.len();
        move_left_grapheme(text, &mut cursor);
        move_left_grapheme(text, &mut cursor);
        assert_eq!(&text[..cursor], "テスト ");
        move_word_backward(text, &mut cursor);
        assert_eq!(&text[..cursor], "");
    }

    #[test]
    fn word_navigation_skips_spaces() {
        let text = "alpha  beta   gamma";
        let mut cursor = 0;
        move_word_forward(text, &mut cursor);
        assert_eq!(&text[..cursor], "alpha  ");
        move_word_forward(text, &mut cursor);
        assert_eq!(&text[..cursor], "alpha  beta   ");

        let mut owned = text.to_string();
        delete_word(&mut owned, &mut cursor);
        assert_eq!(owned, "alpha  gamma");
        assert_eq!(cursor, "alpha  ".len());
    }
}
