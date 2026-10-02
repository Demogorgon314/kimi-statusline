//! Terminal columns and truncation share the same grapheme boundaries.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn ansi_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.first() != Some(&0x1b) {
        return None;
    }
    match b.get(1) {
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map(|i| i + 3),
        Some(b']') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return Some(i + 1);
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return Some(i + 2);
                }
                i += 1;
            }
            Some(b.len())
        }
        _ => Some(1),
    }
}

fn plain_text(s: &str) -> String {
    let mut plain = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(n) = ansi_len(&s[i..]) {
            i += n;
        } else {
            let ch = s[i..].chars().next().unwrap();
            plain.push(ch);
            i += ch.len_utf8();
        }
    }
    plain
}

/// Escapes have no columns; emoji sequences remain a single grapheme even
/// when SGR color changes occur between their constituent characters.
pub fn visible_width(s: &str) -> usize {
    plain_text(s).graphemes(true).map(str::width).sum()
}

pub fn truncate(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let plain = plain_text(s);
    if plain.graphemes(true).map(str::width).sum::<usize>() <= width {
        return s.to_string();
    }
    let mut used = 0;
    let mut keep = 0;
    for g in plain.graphemes(true) {
        if used + g.width() > width - 1 {
            break;
        }
        used += g.width();
        keep += g.len();
    }
    let mut out = String::new();
    let mut i = 0;
    let mut copied = 0;
    let mut link_open = false;
    let mut sgr = false;
    while copied < keep {
        if let Some(n) = ansi_len(&s[i..]) {
            let escape = &s[i..i + n];
            if let Some(link) = escape.strip_prefix("\x1b]8;") {
                // OSC 8 ; parameters ; URI terminator. An empty URI closes it.
                link_open = link.split_once(';').is_some_and(|(_, uri)| {
                    !uri.trim_end_matches('\x07')
                        .trim_end_matches("\x1b\\")
                        .is_empty()
                });
            }
            sgr |= escape.starts_with("\x1b[") && escape.ends_with('m');
            out.push_str(escape);
            i += n;
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            copied += ch.len_utf8();
        }
    }
    if link_open {
        out.push_str("\x1b]8;;\x1b\\");
    }
    out.push('…');
    if sgr {
        out.push_str("\x1b[0m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_and_combining_sequences_keep_their_columns() {
        for s in ["🛡️", "⚙️", "⏱️", "👩‍💻", "🇨🇳", "🛡\x1b[31m️"] {
            assert_eq!(visible_width(s), 2, "{s}");
            assert_eq!(truncate(&format!("{s}abcd"), 2), "…");
            assert_eq!(visible_width(&truncate(&format!("{s}abcd"), 3)), 3);
        }
        assert_eq!(truncate("e\u{301}abcd", 2), "e\u{301}…");
        assert_eq!(truncate("abcd", 0), "");
    }

    #[test]
    fn truncation_closes_hyperlinks_and_resets_sgr() {
        for end in ["\x07", "\x1b\\"] {
            let s = format!("\x1b[31m\x1b]8;;https://example.test{end}PR#123\x1b]8;;{end}\x1b[0m");
            let truncated = truncate(&s, 4);
            assert!(
                truncated.ends_with("PR#\x1b]8;;\x1b\\…\x1b[0m"),
                "{truncated:?}"
            );
            assert_eq!(visible_width(&truncated), 4);
        }
    }
}
