//! Streaming state machine that splits PTY output into
//! (a) plain bytes that go to the VT emulator and
//! (b) media sequences we handle ourselves:
//!   - APC `ESC _ G ... ST`      — kitty graphics protocol
//!   - OSC  `ESC ] 1337;... BEL` — iTerm2 inline images
//!   - OSC  `ESC ] 1338;... BEL` — rterm video frames (fast path over PTY)
//!
//! Handles sequences split arbitrarily across read() boundaries.

use std::time::{Duration, Instant};

/// Maximum size of a half-received sequence before we give up and pass it through.
const MAX_BUF: usize = 96 * 1024 * 1024;
/// If a sequence stalls with no bytes at all for this long, flush it (lone ESC etc.).
const STALE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// Payload of `ESC _ G <payload> ST`.
    KittyGfx(String),
    /// Full content of OSC 1337, e.g. `File=name=x;AAAA`.
    Iterm1337(String),
    /// Full content of OSC 1338.
    Media1338(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Osc,
    OscEsc,
    Apc,
    ApcEsc,
}

pub struct Interceptor {
    state: State,
    buf: Vec<u8>,
    last_byte: Instant,
}

impl Default for Interceptor {
    fn default() -> Self {
        Self::new()
    }
}

impl Interceptor {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::with_capacity(4096),
            last_byte: Instant::now(),
        }
    }

    pub fn in_sequence(&self) -> bool {
        self.state != State::Ground
    }

    /// Feed raw PTY bytes; every capture records its position in `out`, so the
    /// caller can keep text and media in their original interleaved order.
    pub fn feed_marked(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
        cap: &mut Vec<(usize, Captured)>,
    ) {
        if !input.is_empty() {
            self.last_byte = Instant::now();
        }
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            let mut reprocess = false;
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.buf.clear();
                        self.buf.push(b);
                        self.state = State::Esc;
                    } else {
                        out.push(b);
                    }
                }
                State::Esc => match b {
                    b'_' => {
                        self.buf.push(b);
                        self.state = State::Apc;
                    }
                    b']' => {
                        self.buf.push(b);
                        self.state = State::Osc;
                    }
                    0x1b => {
                        out.push(0x1b);
                        self.buf.clear();
                        self.buf.push(0x1b);
                    }
                    _ => {
                        out.extend_from_slice(&self.buf);
                        self.buf.clear();
                        out.push(b);
                        self.state = State::Ground;
                    }
                },
                State::Apc => match b {
                    0x1b => {
                        self.buf.push(b);
                        self.state = State::ApcEsc;
                    }
                    0x9c => {
                        self.buf.push(b);
                        let mark = out.len();
                        self.finish_apc(mark, cap);
                    }
                    _ => {
                        self.buf.push(b);
                        if self.buf.len() > MAX_BUF {
                            out.extend_from_slice(&self.buf);
                            self.buf.clear();
                            self.state = State::Ground;
                        }
                    }
                },
                State::ApcEsc => {
                    if b == b'\\' {
                        self.buf.push(b);
                        let mark = out.len();
                        self.finish_apc(mark, cap);
                    } else {
                        // Not a string terminator: flush what we have and re-dispatch.
                        out.extend_from_slice(&self.buf[..self.buf.len() - 1]);
                        self.buf.clear();
                        self.buf.push(0x1b);
                        self.state = State::Esc;
                        reprocess = true;
                    }
                }
                State::Osc => match b {
                    0x07 => {
                        self.buf.push(b);
                        let mark = out.len();
                        self.finish_osc(mark, out, cap);
                    }
                    0x1b => {
                        self.buf.push(b);
                        self.state = State::OscEsc;
                    }
                    0x9c => {
                        self.buf.push(b);
                        let mark = out.len();
                        self.finish_osc(mark, out, cap);
                    }
                    _ => {
                        self.buf.push(b);
                        if self.buf.len() > MAX_BUF {
                            out.extend_from_slice(&self.buf);
                            self.buf.clear();
                            self.state = State::Ground;
                        }
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.buf.push(b);
                        let mark = out.len();
                        self.finish_osc(mark, out, cap);
                    } else {
                        out.extend_from_slice(&self.buf[..self.buf.len() - 1]);
                        self.buf.clear();
                        self.buf.push(0x1b);
                        self.state = State::Esc;
                        reprocess = true;
                    }
                }
            }
            if !reprocess {
                i += 1;
            }
        }
    }

    /// Flush a stalled partial sequence (e.g. a lone ESC key echo).
    pub fn flush_stale(&mut self, out: &mut Vec<u8>) {
        if self.state != State::Ground && self.last_byte.elapsed() > STALE {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
            self.state = State::Ground;
        }
    }

    /// Flush everything on PTY EOF so the emulator sees the tail.
    pub fn flush_all(&mut self, out: &mut Vec<u8>) {
        if self.state != State::Ground {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
            self.state = State::Ground;
        }
    }

    fn finish_apc(&mut self, mark: usize, cap: &mut Vec<(usize, Captured)>) {
        // buf = ESC _ <payload> [ESC \]
        let end = self.buf.len() - if self.state == State::ApcEsc { 2 } else { 1 };
        let payload = String::from_utf8_lossy(&self.buf[2..end]).into_owned();
        if let Some(rest) = payload.strip_prefix('G') {
            cap.push((mark, Captured::KittyGfx(rest.to_string())));
        }
        // Unknown APC: dropped on purpose (never valid VT input).
        self.buf.clear();
        self.state = State::Ground;
    }

    fn finish_osc(&mut self, mark: usize, out: &mut Vec<u8>, cap: &mut Vec<(usize, Captured)>) {
        // buf = ESC ] <content> [BEL | ESC \ | 0x9c]
        let content = if self.state == State::OscEsc {
            &self.buf[2..self.buf.len() - 2]
        } else {
            &self.buf[2..self.buf.len() - 1]
        };
        let text = String::from_utf8_lossy(content).into_owned();
        if let Some(rest) = text.strip_prefix("1337;") {
            cap.push((mark, Captured::Iterm1337(rest.to_string())));
        } else if let Some(rest) = text.strip_prefix("1338;") {
            cap.push((mark, Captured::Media1338(rest.to_string())));
        } else {
            // Regular OSC (title, colors, hyperlinks...) goes to the emulator untouched.
            out.extend_from_slice(&self.buf);
        }
        self.buf.clear();
        self.state = State::Ground;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(data: &[u8]) -> (String, Vec<Captured>) {
        run_chunks(&[data])
    }

    fn run_chunks(chunks: &[&[u8]]) -> (String, Vec<Captured>) {
        let mut it = Interceptor::new();
        let mut out = Vec::new();
        let mut cap = Vec::new();
        for c in chunks {
            let mut marked = Vec::new();
            it.feed_marked(c, &mut out, &mut marked);
            cap.extend(marked.into_iter().map(|(_, captured)| captured));
        }
        it.flush_all(&mut out);
        (String::from_utf8_lossy(&out).into_owned(), cap)
    }

    #[test]
    fn plain_text_passes_through() {
        let (out, cap) = run(b"hello\r\n");
        assert_eq!(out, "hello\r\n");
        assert!(cap.is_empty());
    }

    #[test]
    fn csi_sequences_pass_through_intact() {
        let (out, cap) = run(b"\x1b[1;5C\x1b[38;5;123m");
        assert_eq!(out, "\x1b[1;5C\x1b[38;5;123m");
        assert!(cap.is_empty());
    }

    #[test]
    fn kitty_apc_split_across_reads() {
        let seq = b"\x1b_Gi=1;f=100,a=t;AAAA\x1b\\ tail";
        let chunks: Vec<&[u8]> = seq.chunks(3).collect();
        let (out, cap) = run_chunks(&chunks);
        assert_eq!(out, " tail");
        assert_eq!(cap, vec![Captured::KittyGfx("i=1;f=100,a=t;AAAA".into())]);
    }

    #[test]
    fn iterm_osc_bel_captured_title_passthrough() {
        let (out, cap) = run(b"\x1b]0;my title\x07\x1b]1337;File=name=x;QQ==\x07end");
        assert_eq!(out, "\x1b]0;my title\x07end");
        assert_eq!(cap, vec![Captured::Iterm1337("File=name=x;QQ==".into())]);
    }

    #[test]
    fn osc_with_st_terminator() {
        let (out, cap) = run(b"\x1b]1338;1,2,3,4,0;AAAA\x1b\\!");
        assert_eq!(out, "!");
        assert_eq!(cap, vec![Captured::Media1338("1,2,3,4,0;AAAA".into())]);
    }

    #[test]
    fn lone_esc_flushed_when_stale() {
        let mut it = Interceptor::new();
        let mut out = Vec::new();
        let mut cap = Vec::new();
        it.feed_marked(b"\x1b", &mut out, &mut cap);
        assert!(it.in_sequence());
        it.last_byte = Instant::now() - Duration::from_secs(1);
        it.flush_stale(&mut out);
        assert_eq!(out, b"\x1b");
        assert!(!it.in_sequence());
    }

    #[test]
    fn esc_esc_restarts() {
        let (out, cap) = run(b"\x1b\x1b[A");
        assert_eq!(out, "\x1b\x1b[A");
        assert!(cap.is_empty());
    }

    #[test]
    fn eight_bit_st_apc() {
        let (out, cap) = run(b"\x1b_Ga=t;xx\x9c");
        assert_eq!(out, "");
        assert_eq!(cap, vec![Captured::KittyGfx("a=t;xx".into())]);
    }

    #[test]
    fn marked_feeding_keeps_capture_order() {
        let mut it = Interceptor::new();
        let mut out = Vec::new();
        let mut cap = Vec::new();
        it.feed_marked(
            b"ab\x1b_Gi=1;AAAA\x1b\\cd\x1b]1337;File=x;QQ==\x07ef",
            &mut out,
            &mut cap,
        );
        assert_eq!(out, b"abcdef");
        assert_eq!(
            cap,
            vec![
                (2, Captured::KittyGfx("i=1;AAAA".into())),
                (4, Captured::Iterm1337("File=x;QQ==".into())),
            ]
        );
    }
}
