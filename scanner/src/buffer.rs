use std::{
    fs::File,
    io::{BufReader, Read},
};

pub struct Buffer {
    cursor: usize,
    length: usize,
    content: Vec<u8>,
}

impl Buffer {
    pub fn new(path: &str) -> std::result::Result<Self, std::io::Error> {
        let file = File::open(path)?;
        let mut content = vec![];
        let _ = BufReader::new(file).read_to_end(&mut content);

        let length = content.len();

        Ok(Self {
            content,
            cursor: 0,
            length,
        })
    }

    pub fn current(&mut self) -> Option<char> {
        self.content.get(self.cursor).map(|ch| *ch as char)
    }

    pub fn peek(&mut self, offset: usize) -> Option<char> {
        self.content.get(self.cursor + offset).map(|ch| *ch as char)
    }

    pub fn next(&mut self) {
        if self.cursor < self.length {
            self.cursor += 1;
        }
    }

    pub fn is_consumed(&self) -> bool {
        self.cursor == self.length
    }

    pub fn tell(&self) -> usize {
        self.cursor
    }

    pub fn string_at_range(&self, start: usize, end: usize) -> Option<String> {
        String::from_utf8(self.content[start..end].to_vec()).ok()
    }
}

impl TryFrom<&str> for Buffer {
    type Error = std::io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(Self {
            cursor: 0,
            length: value.len(),
            content: value.as_bytes().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn test_construction_from_string() {
        let buffer = Buffer::try_from("X");
        assert!(buffer.is_ok(), "Unable to create buffer");

        if let Ok(mut buffer) = buffer {
            assert_eq!(buffer.peek(0), Some('X'));
            assert!(buffer.peek(1).is_none());

            assert_eq!(buffer.current(), Some('X'));
            assert_eq!(buffer.peek(0), buffer.current());
            assert_eq!(buffer.tell(), 0);
            assert!(!buffer.is_consumed());
            buffer.next();
            assert!(buffer.current().is_none());
            assert!(buffer.is_consumed());
        }
    }
}
