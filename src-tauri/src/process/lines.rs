/// Splits a byte stream into lines on `\n` or `\r`, tolerating invalid UTF-8 and runaway lines.
///
/// Carriage returns count as line ends because tools redraw progress in place with them.
#[derive(Debug)]
pub struct LineAssembler {
    buffer: Vec<u8>,
    max_line_length: usize,
    /// True while discarding the remainder of a line that exceeded `max_line_length`.
    discarding: bool,
}

impl LineAssembler {
    pub fn new(max_line_length: usize) -> Self {
        Self { buffer: Vec::new(), max_line_length: max_line_length.max(1), discarding: false }
    }

    pub fn append(&mut self, data: &[u8], mut emit: impl FnMut(String)) {
        self.buffer.extend_from_slice(data);
        let mut cursor = 0;
        while let Some(offset) = self.buffer[cursor..].iter().position(|&b| b == b'\n' || b == b'\r') {
            let terminator = cursor + offset;
            if self.discarding {
                self.discarding = false;
            } else if terminator > cursor {
                emit(self.decode(&self.buffer[cursor..terminator]));
            }
            cursor = terminator + 1;
        }
        self.buffer.drain(..cursor);

        if self.discarding {
            self.buffer.clear();
        } else if self.buffer.len() > self.max_line_length {
            emit(self.decode(&self.buffer));
            self.buffer.clear();
            self.discarding = true;
        }
    }

    /// Emits whatever is left when the stream ends without a trailing newline.
    pub fn flush(&mut self, mut emit: impl FnMut(String)) {
        if !self.discarding && !self.buffer.is_empty() {
            emit(self.decode(&self.buffer));
        }
        self.buffer.clear();
        self.discarding = false;
    }

    fn decode(&self, bytes: &[u8]) -> String {
        let end = bytes.len().min(self.max_line_length);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(assembler: &mut LineAssembler, chunks: &[&[u8]]) -> Vec<String> {
        let mut lines = Vec::new();
        for chunk in chunks {
            assembler.append(chunk, |line| lines.push(line));
        }
        assembler.flush(|line| lines.push(line));
        lines
    }

    #[test]
    fn reassembles_lines_split_across_chunks() {
        let mut assembler = LineAssembler::new(1024);
        assert_eq!(collect(&mut assembler, &[b"hel", b"lo\nwor", b"ld\n"]), ["hello", "world"]);
    }

    #[test]
    fn treats_cr_lf_and_crlf_as_terminators_and_skips_blank_lines() {
        let mut assembler = LineAssembler::new(1024);
        assert_eq!(collect(&mut assembler, &[b"a\r\nb\rc\n\n\nd\n"]), ["a", "b", "c", "d"]);
    }

    #[test]
    fn flushes_a_final_line_with_no_terminator() {
        let mut assembler = LineAssembler::new(1024);
        assert_eq!(collect(&mut assembler, &[b"first\nlast"]), ["first", "last"]);
    }

    #[test]
    fn truncates_a_runaway_line_and_resumes_at_the_next_terminator() {
        let mut assembler = LineAssembler::new(8);
        let lines = collect(&mut assembler, &[b"0123456789abcdef", b"ghij\nnext\n"]);
        assert_eq!(lines, ["01234567", "next"]);
    }

    #[test]
    fn survives_invalid_utf8() {
        let mut assembler = LineAssembler::new(64);
        let lines = collect(&mut assembler, &[b"caf\xc3\xa9 \xff\xfe ok\n"]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("café"));
        assert!(lines[0].ends_with("ok"));
    }
}
