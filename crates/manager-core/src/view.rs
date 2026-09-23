//! Working out how to show the contents of a file.
//!
//! This module never touches the disk. It takes a window of bytes and decides
//! what they are, turns them into text, and lays that text out in lines of a
//! given width. Keeping it separate from any reading means the awkward parts —
//! encodings, line endings, tabs, wrapping — are decided by plain functions
//! with plain tests, and the same answers serve the terminal and the GUI.

/// How a file should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Readable text in this encoding.
    Text(Encoding),
    /// Not text: show a hex dump.
    Binary,
}

/// The encodings the viewer understands.
///
/// Deliberately few. Anything that isn't valid UTF-8 and isn't announced by a
/// byte-order mark is read as Latin-1, which maps every possible byte to some
/// character, so a file always shows *something* rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Latin1,
}

/// How many bytes are examined before deciding what a file is.
pub const SNIFF_BYTES: usize = 8 * 1024;

/// Decides how to show a file, from the start of its contents.
pub fn sniff(bytes: &[u8]) -> Content {
    for (mark, encoding) in BYTE_ORDER_MARKS {
        if bytes.starts_with(mark) {
            return Content::Text(encoding);
        }
    }

    let window = &bytes[..bytes.len().min(SNIFF_BYTES)];
    // Real text doesn't contain a zero byte; practically every binary format does.
    if window.contains(&0) {
        return Content::Binary;
    }

    match std::str::from_utf8(window) {
        Ok(_) => Content::Text(Encoding::Utf8),
        // `error_len() == None` means the window simply stops in the middle of
        // a character, which says nothing about the file. Anything else is a
        // byte that can't be UTF-8 at all, so read the file as Latin-1.
        Err(problem) if problem.error_len().is_none() => Content::Text(Encoding::Utf8),
        Err(_) => Content::Text(Encoding::Latin1),
    }
}

/// The marks a file can start with to announce its encoding.
const BYTE_ORDER_MARKS: [(&[u8], Encoding); 3] = [
    (&[0xef, 0xbb, 0xbf], Encoding::Utf8),
    (&[0xff, 0xfe], Encoding::Utf16Le),
    (&[0xfe, 0xff], Encoding::Utf16Be),
];

/// Turns bytes into text, replacing anything that can't be decoded.
///
/// A byte-order mark at the start is removed: it announces the encoding, it
/// isn't part of the text.
pub fn decode(bytes: &[u8], encoding: Encoding) -> String {
    let bytes = strip_mark(bytes, encoding);
    match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        // Latin-1 is the first 256 code points, so every byte maps to a
        // character and no file can fail to decode.
        Encoding::Latin1 => bytes.iter().map(|&byte| byte as char).collect(),
        Encoding::Utf16Le => utf16(bytes, u16::from_le_bytes),
        Encoding::Utf16Be => utf16(bytes, u16::from_be_bytes),
    }
}

fn strip_mark(bytes: &[u8], encoding: Encoding) -> &[u8] {
    match BYTE_ORDER_MARKS
        .iter()
        .find(|(mark, marked)| *marked == encoding && bytes.starts_with(mark))
    {
        Some((mark, _)) => &bytes[mark.len()..],
        None => bytes,
    }
}

fn utf16(bytes: &[u8], order: fn([u8; 2]) -> u16) -> String {
    // A trailing odd byte is dropped: half a character can't be shown.
    let units = bytes.chunks_exact(2).map(|pair| order([pair[0], pair[1]]));
    char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Splits text into the lines to put on screen.
///
/// Line endings may be LF, CRLF or CR. Tabs are expanded to the next multiple
/// of `tab_width`. When `width` is `Some`, long lines are wrapped at that many
/// characters; when it's `None` they are left whole for the UI to scroll.
pub fn lay_out(text: &str, width: Option<usize>, tab_width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in split_lines(text) {
        let line = expand_tabs(line, tab_width);
        match width {
            Some(width) if width > 0 => {
                let characters: Vec<char> = line.chars().collect();
                if characters.is_empty() {
                    out.push(String::new());
                }
                // Wrapping by character, never by byte, so nothing is torn.
                out.extend(characters.chunks(width).map(|part| part.iter().collect()));
            }
            _ => out.push(line),
        }
    }
    out
}

/// Splits on LF, CRLF or CR, all of which are in use somewhere.
///
/// A newline ends a line rather than starting an empty one, so a file that
/// ends with one doesn't show a phantom last line.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                lines.push(&text[start..at]);
                at += 1;
                start = at;
            }
            b'\r' => {
                lines.push(&text[start..at]);
                at += if bytes.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = at;
            }
            _ => at += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Replaces tabs with spaces up to the next tab stop, so columns line up.
fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let tab_width = tab_width.max(1);
    let mut out = String::new();
    let mut column = 0;
    for character in line.chars() {
        if character == '\t' {
            let spaces = tab_width - (column % tab_width);
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(character);
            column += 1;
        }
    }
    out
}

/// One row of a hex dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexRow {
    /// Offset of the first byte, counted from the start of the file.
    pub offset: u64,
    pub bytes: Vec<u8>,
}

impl HexRow {
    /// The right-hand column: printable ASCII, everything else a dot.
    pub fn text(&self) -> String {
        self.bytes
            .iter()
            .map(|&byte| {
                if byte.is_ascii_graphic() || byte == b' ' {
                    byte as char
                } else {
                    '.'
                }
            })
            .collect()
    }
}

/// Splits bytes into hex-dump rows, the first one starting at `start`.
pub fn hex_rows(bytes: &[u8], start: u64, per_row: usize) -> Vec<HexRow> {
    let per_row = per_row.max(1);
    bytes
        .chunks(per_row)
        .enumerate()
        .map(|(row, bytes)| HexRow {
            offset: start + (row * per_row) as u64,
            bytes: bytes.to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_counts_as_text() {
        assert_eq!(sniff(b""), Content::Text(Encoding::Utf8));
    }

    #[test]
    fn plain_text_is_utf8() {
        assert_eq!(sniff(b"hello, world\n"), Content::Text(Encoding::Utf8));
        assert_eq!(
            sniff("caf\u{e9} \u{2014} na\u{ef}ve".as_bytes()),
            Content::Text(Encoding::Utf8)
        );
    }

    #[test]
    fn a_byte_order_mark_settles_the_encoding() {
        assert_eq!(sniff(b"\xef\xbb\xbfhello"), Content::Text(Encoding::Utf8));
        assert_eq!(sniff(b"\xff\xfeh\0i\0"), Content::Text(Encoding::Utf16Le));
        assert_eq!(sniff(b"\xfe\xff\0h\0i"), Content::Text(Encoding::Utf16Be));
    }

    #[test]
    fn a_null_byte_means_binary() {
        // Nothing in real text contains one, and every binary format does.
        assert_eq!(sniff(b"hello\0world"), Content::Binary);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Content::Binary);
    }

    #[test]
    fn old_single_byte_text_is_read_as_latin1() {
        // `café` written in Latin-1 is not valid UTF-8, but it is still text.
        assert_eq!(sniff(b"caf\xe9 au lait\n"), Content::Text(Encoding::Latin1));
    }

    #[test]
    fn a_cut_off_character_at_the_end_does_not_spoil_the_guess() {
        // The window stops mid-character; the file is still UTF-8.
        let mut bytes = "hello \u{4e16}\u{754c}".as_bytes().to_vec();
        bytes.truncate(bytes.len() - 1);
        assert_eq!(sniff(&bytes), Content::Text(Encoding::Utf8));
    }

    #[test]
    fn utf8_decodes_unchanged_without_its_mark() {
        assert_eq!(decode(b"hello", Encoding::Utf8), "hello");
        assert_eq!(decode(b"\xef\xbb\xbfhello", Encoding::Utf8), "hello");
    }

    #[test]
    fn utf16_decodes_from_either_end() {
        assert_eq!(decode(b"\xff\xfeh\0i\0", Encoding::Utf16Le), "hi");
        assert_eq!(decode(b"\xfe\xff\0h\0i", Encoding::Utf16Be), "hi");
    }

    #[test]
    fn latin1_maps_every_byte_to_something() {
        assert_eq!(decode(b"caf\xe9", Encoding::Latin1), "caf\u{e9}");
        // All 256 of them, so no file can fail to decode.
        let every: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&every, Encoding::Latin1).chars().count(), 256);
    }

    #[test]
    fn broken_utf8_becomes_replacement_characters_rather_than_an_error() {
        assert_eq!(decode(b"a\xffb", Encoding::Utf8), "a\u{fffd}b");
    }

    #[test]
    fn every_kind_of_line_ending_splits() {
        assert_eq!(lay_out("a\nb", None, 4), ["a", "b"]);
        assert_eq!(lay_out("a\r\nb", None, 4), ["a", "b"]);
        assert_eq!(lay_out("a\rb", None, 4), ["a", "b"]);
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_line() {
        assert_eq!(lay_out("a\nb\n", None, 4), ["a", "b"]);
        // But a blank line in the middle is a real line.
        assert_eq!(lay_out("a\n\nb", None, 4), ["a", "", "b"]);
    }

    #[test]
    fn tabs_line_up_to_the_next_stop() {
        assert_eq!(lay_out("a\tb", None, 4), ["a   b"]);
        assert_eq!(lay_out("abc\td", None, 4), ["abc d"]);
        assert_eq!(lay_out("abcd\te", None, 4), ["abcd    e"]);
    }

    #[test]
    fn long_lines_wrap_when_asked() {
        assert_eq!(lay_out("abcdefgh", Some(3), 4), ["abc", "def", "gh"]);
        assert_eq!(lay_out("abcdefgh", None, 4), ["abcdefgh"]);
    }

    #[test]
    fn wrapping_counts_characters_not_bytes() {
        // Three characters, six bytes. Splitting by bytes would tear them.
        assert_eq!(
            lay_out("\u{e9}\u{e9}\u{e9}\u{e9}", Some(3), 4),
            ["\u{e9}\u{e9}\u{e9}", "\u{e9}"]
        );
    }

    #[test]
    fn hex_rows_are_numbered_from_where_the_window_starts() {
        let rows = hex_rows(b"0123456789abcdefghij", 0x100, 16);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].offset, 0x100);
        assert_eq!(rows[0].bytes.len(), 16);
        assert_eq!(rows[1].offset, 0x110);
        assert_eq!(rows[1].bytes, b"ghij", "the last row is short");
    }

    #[test]
    fn the_hex_text_column_shows_only_printable_characters() {
        let row = HexRow {
            offset: 0,
            bytes: b"ab\0\x1f~\x7f\xff".to_vec(),
        };
        assert_eq!(row.text(), "ab..~..");
    }

    #[test]
    fn nothing_at_all_produces_no_rows() {
        assert!(hex_rows(b"", 0, 16).is_empty());
    }
}
