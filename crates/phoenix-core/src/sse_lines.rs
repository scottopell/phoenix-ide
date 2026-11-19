//! Line framing for Server-Sent Events streams (`\n`, `\r\n` or bare `\r`).

/// Splits a byte stream into lines, independent of how it is chunked.
#[derive(Debug, Default)]
pub struct LineSplitter {
    line: Vec<u8>,
    after_cr: bool,
}

impl LineSplitter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes, calling `emit` once per completed line (terminator excluded).
    pub fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        for &b in bytes {
            match b {
                // The `\n` half of a `\r\n` pair whose `\r` already ended a line.
                b'\n' if self.after_cr => self.after_cr = false,
                b'\n' | b'\r' => {
                    self.after_cr = b == b'\r';
                    emit(&self.line);
                    self.line.clear();
                }
                _ => {
                    self.after_cr = false;
                    self.line.push(b);
                }
            }
        }
    }

    /// Take the unterminated final line, if any (end of stream acts as its
    /// terminator).
    pub fn take_partial(&mut self) -> Option<Vec<u8>> {
        (!self.line.is_empty()).then(|| std::mem::take(&mut self.line))
    }

    /// Bytes of the current incomplete line.
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.line.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, Copy)]
    enum Term {
        Lf,
        CrLf,
        Cr,
    }

    impl Term {
        fn bytes(self) -> &'static [u8] {
            match self {
                Term::Lf => b"\n",
                Term::CrLf => b"\r\n",
                Term::Cr => b"\r",
            }
        }
    }

    fn split(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut s = LineSplitter::new();
        let mut out = Vec::new();
        for c in chunks {
            s.push(c, |l| out.push(l.to_vec()));
        }
        out
    }

    proptest! {
        /// Rendering logical lines with any mix of terminators and cutting the
        /// stream anywhere recovers exactly the logical lines.
        #[test]
        fn lines_survive_any_terminator_mix_and_chunking(
            lines in proptest::collection::vec(
                ("[a-z\u{00e9}\u{1f600} ]{0,8}", prop_oneof![
                    Just(Term::Lf), Just(Term::CrLf), Just(Term::Cr)
                ]),
                0..16,
            ),
            cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..10),
        ) {
            let mut bytes = Vec::new();
            let mut expected: Vec<Vec<u8>> = Vec::new();
            let mut prev_cr = false;
            for (text, term) in &lines {
                // `\r` then an empty line ended by `\n` is one `\r\n` on the
                // wire, so that rendering names a different line sequence.
                let term = if text.is_empty() && prev_cr && matches!(term, Term::Lf) {
                    Term::CrLf
                } else {
                    *term
                };
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(term.bytes());
                prev_cr = matches!(term, Term::Cr);
                expected.push(text.as_bytes().to_vec());
            }
            let mut points: Vec<usize> =
                cuts.iter().map(|i| i.index(bytes.len() + 1)).collect();
            points.sort_unstable();
            let mut chunks: Vec<&[u8]> = Vec::new();
            let mut prev = 0;
            for p in points {
                chunks.push(&bytes[prev..p]);
                prev = p;
            }
            chunks.push(&bytes[prev..]);
            prop_assert_eq!(split(&chunks), expected);
        }
    }

    #[test]
    fn crlf_split_across_chunks_is_one_terminator() {
        assert_eq!(
            split(&[b"a\r", b"\nb\n"]),
            vec![b"a".to_vec(), b"b".to_vec()]
        );
    }
}
