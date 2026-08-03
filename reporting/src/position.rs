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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_count_supplementary_plane() {
        // 'α' = 1 UTF-16 unit, '😀' = 2 UTF-16 units → column 3 at end of line.
        assert_eq!(
            byte_offset_to_lsp_position("α😀\nname", "α😀".len()),
            LspPosition {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            byte_offset_to_lsp_position("α😀\nname", "α😀\nn".len()),
            LspPosition {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn byte_range_maps_start_and_end() {
        let text = "ab\ncd";
        let range = byte_range_to_lsp_range(text, &(3..5)); // "cd"
        assert_eq!(
            range,
            LspRange {
                start: LspPosition {
                    line: 1,
                    character: 0
                },
                end: LspPosition {
                    line: 1,
                    character: 2
                },
            }
        );
    }

    #[test]
    fn byte_past_end_clamps_to_eof() {
        let text = "hi";
        assert_eq!(
            byte_offset_to_lsp_position(text, 99),
            LspPosition {
                line: 0,
                character: 2
            }
        );
    }
}
