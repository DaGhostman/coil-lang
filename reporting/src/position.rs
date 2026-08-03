use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

pub fn byte_offset_to_lsp_position(text: &str, byte: usize) -> LspPosition {
    let byte = byte.min(text.len());
    let mut line = 0;
    let mut line_start = 0;
    for (index, character) in text.char_indices() {
        if index >= byte {
            break;
        }
        if character == '\n' {
            line += 1;
            line_start = index + character.len_utf8();
        }
    }
    let character = text[line_start..byte]
        .chars()
        .map(|character| character.len_utf16() as u32)
        .sum();
    LspPosition { line, character }
}

pub fn byte_range_to_lsp_range(text: &str, range: &Range<usize>) -> LspRange {
    LspRange {
        start: byte_offset_to_lsp_position(text, range.start),
        end: byte_offset_to_lsp_position(text, range.end),
    }
}
