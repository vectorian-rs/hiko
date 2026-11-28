//! Shared input-independent cursor. Only short-lived public Jiter leases borrow input.
use crate::reader::{Kind, Limits, MAX_DEPTH, Scratch, Stats};
use crate::{Error, ErrorKind, Result};
use jiter::{Jiter, Peek};
use std::{mem, ops::Range};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    First,
    Pending,
    Ready,
    Ended,
}
#[derive(Clone, Copy)]
struct Frame {
    kind: Kind,
    phase: Phase,
}
const EMPTY: Frame = Frame {
    kind: Kind::Array,
    phase: Phase::First,
};

pub(crate) struct State {
    pub(crate) offset: usize,
    pub(crate) poisoned: bool,
    pub(crate) stats: Stats,
    limits: Limits,
    frames: [Frame; MAX_DEPTH],
    depth: usize,
    root_done: bool,
    peek: Option<Peek>,
}

pub(crate) fn at(kind: ErrorKind, offset: usize) -> Error {
    Error { kind, offset }
}
fn syntax(e: jiter::JiterError, base: usize) -> Error {
    at(ErrorKind::Syntax(e.error_type), base + e.index)
}

impl State {
    pub(crate) fn new<'i>(
        input: &'i [u8],
        scratch: &mut Scratch,
        limits: Limits,
    ) -> Result<(Self, &'i str)> {
        if input.len() > limits.max_input_bytes {
            return Err(at(ErrorKind::InputLimit, 0));
        }
        if limits.max_depth > MAX_DEPTH {
            return Err(at(ErrorKind::DepthLimit, 0));
        }
        let prepaid = u64::try_from(input.len())
            .ok()
            .and_then(|n| n.checked_mul(32))
            .and_then(|n| n.checked_add(256))
            .ok_or_else(|| at(ErrorKind::WorkLimit, 0))?;
        if prepaid > limits.max_work {
            return Err(at(ErrorKind::WorkLimit, 0));
        }
        let peak = scratch
            .peak_for(input.len())
            .ok_or_else(|| at(ErrorKind::ScratchLimit, 0))?;
        if peak > limits.max_scratch_bytes {
            return Err(at(ErrorKind::ScratchLimit, 0));
        }
        let input =
            std::str::from_utf8(input).map_err(|e| at(ErrorKind::InvalidUtf8, e.valid_up_to()))?;
        scratch.prepare(input.len())?;
        Ok((
            Self {
                offset: 0,
                poisoned: false,
                stats: Stats {
                    charged_work: prepaid,
                    peak_scratch_bytes: peak,
                    ..Stats::default()
                },
                limits,
                frames: [EMPTY; MAX_DEPTH],
                depth: 0,
                root_done: false,
                peek: None,
            },
            input,
        ))
    }
    pub(crate) fn healthy(&self) -> Result<()> {
        if self.poisoned {
            Err(at(ErrorKind::Poisoned, self.offset))
        } else {
            Ok(())
        }
    }
    pub(crate) fn fail<T>(&mut self, kind: ErrorKind) -> Result<T> {
        self.fail_at(kind, self.offset)
    }
    pub(crate) fn fail_at<T>(&mut self, kind: ErrorKind, offset: usize) -> Result<T> {
        self.poisoned = true;
        Err(at(kind, offset))
    }
    fn debit(&mut self) -> Result<()> {
        self.healthy()?;
        if self.stats.charged_work >= self.limits.max_work {
            return self.fail(ErrorKind::WorkLimit);
        }
        self.stats.charged_work += 1;
        Ok(())
    }
    fn pending(&self) -> bool {
        if self.depth == 0 {
            !self.root_done
        } else {
            self.frames[self.depth - 1].phase == Phase::Pending
        }
    }
    fn complete(&mut self) {
        self.peek = None;
        if self.depth == 0 {
            self.root_done = true;
        } else {
            self.frames[self.depth - 1].phase = Phase::Ready;
        }
    }
    /// No backend work: borrowing guards call this on completed container Drop.
    pub(crate) fn pop_ended(&mut self) {
        if self.depth == 0 || self.frames[self.depth - 1].phase != Phase::Ended {
            self.poisoned = true;
            return;
        }
        self.depth -= 1;
        self.complete();
    }
}

pub(crate) struct Context<'a> {
    pub(crate) input: &'a str,
    pub(crate) scratch: &'a mut Scratch,
    pub(crate) state: &'a mut State,
}

impl Context<'_> {
    /// Standalone abstract API credits are NOT calibrated VM host work.
    pub(crate) fn api(&mut self) -> Result<()> {
        self.state.debit()?;
        self.state.stats.api_calls += 1;
        self.scratch.release_text();
        Ok(())
    }
    fn backend(&mut self) -> Result<()> {
        self.state.debit()?;
        self.state.stats.backend_calls += 1;
        Ok(())
    }
    fn lease<T>(
        &mut self,
        f: impl FnOnce(&mut Jiter<'_>, &mut String, usize) -> Result<T>,
    ) -> Result<T> {
        self.backend()?;
        let base = self.state.offset;
        let bytes = self
            .input
            .as_bytes()
            .get(base..)
            .ok_or_else(|| at(ErrorKind::InternalInvariant, base))?;
        let mut parser = Jiter::with_tape(bytes, mem::take(&mut self.scratch.tape));
        let result = f(&mut parser, &mut self.scratch.keys, base);
        self.state.offset = base + parser.current_index();
        self.scratch.tape = parser.into_tape();
        if result.is_err() {
            self.state.poisoned = true;
        }
        result
    }
    fn peek(&mut self) -> Result<Peek> {
        self.state.healthy()?;
        if !self.state.pending() {
            return self.state.fail(ErrorKind::Consumed);
        }
        if let Some(p) = self.state.peek {
            return Ok(p);
        }
        let p = self.lease(|p, _, base| p.peek().map_err(|e| syntax(e, base)))?;
        self.state.stats.classification_peeks += 1;
        self.state.peek = Some(p);
        Ok(p)
    }
    pub(crate) fn kind(&mut self) -> Result<Kind> {
        let p = self.peek()?;
        match p {
            Peek::Null => Ok(Kind::Null),
            Peek::True | Peek::False => Ok(Kind::Boolean),
            Peek::String => Ok(Kind::String),
            Peek::Array => Ok(Kind::Array),
            Peek::Object => Ok(Kind::Object),
            _ if p.is_num() => Ok(Kind::Number),
            _ => self
                .state
                .fail(ErrorKind::Syntax(jiter::JiterErrorType::JsonError(
                    jiter::JsonErrorType::ExpectedSomeValue,
                ))),
        }
    }
    pub(crate) fn require(&mut self, kind: Kind) -> Result<()> {
        if self.kind()? != kind {
            return self.state.fail(ErrorKind::WrongKind);
        }
        Ok(())
    }
    pub(crate) fn enter(&mut self, kind: Kind) -> Result<()> {
        self.require(kind)?;
        if self.state.depth >= self.state.limits.max_depth {
            return self.state.fail(ErrorKind::DepthLimit);
        }
        self.state.frames[self.state.depth] = Frame { kind, ..EMPTY };
        self.state.depth += 1;
        Ok(())
    }
    fn ready(&mut self, kind: Kind) -> Result<bool> {
        self.state.healthy()?;
        if self.state.depth == 0 || self.state.frames[self.state.depth - 1].kind != kind {
            return self.state.fail(ErrorKind::WrongKind);
        }
        let frame = self.state.frames[self.state.depth - 1];
        if frame.phase == Phase::Pending {
            return self.state.fail(ErrorKind::UnconsumedChild);
        }
        Ok(frame.phase != Phase::Ended)
    }
    pub(crate) fn key(&mut self, retain: bool) -> Result<Option<Range<usize>>> {
        if !self.ready(Kind::Object)? {
            return Ok(None);
        }
        let first = self.state.frames[self.state.depth - 1].phase == Phase::First;
        let key = self.lease(|p, keys, base| {
            let key = if first {
                p.known_object()
            } else {
                p.next_key()
            }
            .map_err(|e| syntax(e, base))?;
            key.map(|key| append(keys, if retain { key } else { "" }, base))
                .transpose()
        })?;
        self.advanced(key.is_some());
        self.state.peek = None;
        Ok(key)
    }
    pub(crate) fn array_next(&mut self) -> Result<bool> {
        if !self.ready(Kind::Array)? {
            return Ok(false);
        }
        let first = self.state.frames[self.state.depth - 1].phase == Phase::First;
        let next = self.lease(|p, _, base| {
            (if first {
                p.known_array()
            } else {
                p.array_step()
            })
            .map_err(|e| syntax(e, base))
        })?;
        self.advanced(next.is_some());
        self.state.peek = next;
        Ok(next.is_some())
    }
    fn advanced(&mut self, more: bool) {
        let frame = &mut self.state.frames[self.state.depth - 1];
        frame.phase = if more { Phase::Pending } else { Phase::Ended };
    }
    pub(crate) fn null(&mut self) -> Result<()> {
        self.require(Kind::Null)?;
        self.lease(|p, _, base| p.known_null().map_err(|e| syntax(e, base)))?;
        self.state.complete();
        Ok(())
    }
    pub(crate) fn boolean(&mut self) -> Result<bool> {
        self.require(Kind::Boolean)?;
        let peek = self.peek()?;
        let value = self.lease(|p, _, base| p.known_bool(peek).map_err(|e| syntax(e, base)))?;
        self.state.complete();
        Ok(value)
    }
    pub(crate) fn number(&mut self) -> Result<Range<usize>> {
        self.require(Kind::Number)?;
        let peek = self.peek()?;
        let start = self.state.offset;
        self.lease(|p, _, base| {
            p.known_number_bytes(peek)
                .map(|_| ())
                .map_err(|e| syntax(e, base))
        })?;
        let range = start..self.state.offset;
        self.state.complete();
        Ok(range)
    }
    pub(crate) fn integer<T: std::str::FromStr>(&mut self) -> Result<T> {
        let range = self.number()?;
        let text = &self.input[range.clone()];
        if text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
            return self
                .state
                .fail_at(ErrorKind::IntegerTokenRequired, range.start);
        }
        let digits = if text == "-0" { "0" } else { text };
        digits
            .parse()
            .or_else(|_| self.state.fail_at(ErrorKind::NumberRange, range.start))
    }
    pub(crate) fn float(&mut self) -> Result<f64> {
        let range = self.number()?;
        self.backend()?;
        self.state.stats.float_conversions += 1;
        let text = &self.input[range.clone()];
        let f = match jiter::NumberFloat::from_bytes(text.as_bytes(), false) {
            Ok(f) => f.0,
            Err(e) => {
                return self.state.fail_at(
                    ErrorKind::Syntax(jiter::JiterErrorType::JsonError(e.error_type)),
                    range.start + e.index,
                );
            }
        };
        if !f.is_finite() {
            return self.state.fail(ErrorKind::NonFinite);
        }
        if f == 0.0
            && text
                .bytes()
                .take_while(|b| *b != b'e' && *b != b'E')
                .any(|b| matches!(b, b'1'..=b'9'))
        {
            return self.state.fail(ErrorKind::Underflow);
        }
        Ok(f)
    }
    pub(crate) fn string(&mut self) -> Result<Range<usize>> {
        self.require(Kind::String)?;
        let range = self.lease(|p, keys, base| {
            let text = p.known_str().map_err(|e| syntax(e, base))?;
            append(keys, text, base)
        })?;
        self.scratch.text_start = Some(range.start);
        self.state.complete();
        Ok(range)
    }
    fn skip_string(&mut self) -> Result<()> {
        self.lease(|p, _, base| p.known_bytes().map(|_| ()).map_err(|e| syntax(e, base)))?;
        self.state.complete();
        Ok(())
    }
    /// Traversal belongs to this core, not to either input-ownership frontend.
    pub(crate) fn skip(&mut self) -> Result<()> {
        if !self.state.pending() {
            return self.state.fail(ErrorKind::Consumed);
        }
        let target = self.state.depth;
        loop {
            if self.state.pending() {
                match self.kind()? {
                    Kind::Object => self.enter(Kind::Object)?,
                    Kind::Array => self.enter(Kind::Array)?,
                    Kind::String => self.skip_string()?,
                    Kind::Number => {
                        self.number()?;
                    }
                    Kind::Null => self.null()?,
                    Kind::Boolean => {
                        self.boolean()?;
                    }
                }
                if self.state.depth == target {
                    return Ok(());
                }
            } else {
                let kind = self.state.frames[self.state.depth - 1].kind;
                let more = if kind == Kind::Object {
                    self.key(false)?.is_some()
                } else {
                    self.array_next()?
                };
                if !more {
                    self.state.pop_ended();
                    if self.state.depth == target {
                        return Ok(());
                    }
                }
            }
        }
    }
    pub(crate) fn finish_container(&mut self, kind: Kind) -> Result<()> {
        if self.state.depth == 0 || self.state.frames[self.state.depth - 1].kind != kind {
            return self.state.fail(ErrorKind::WrongKind);
        }
        loop {
            if self.state.pending() {
                self.skip()?;
            }
            let more = if kind == Kind::Object {
                self.key(false)?.is_some()
            } else {
                self.array_next()?
            };
            if !more {
                self.state.pop_ended();
                return Ok(());
            }
        }
    }
    pub(crate) fn finish_all(&mut self) -> Result<()> {
        while self.state.depth != 0 {
            let kind = self.state.frames[self.state.depth - 1].kind;
            self.finish_container(kind)?;
        }
        if !self.state.root_done {
            self.skip()?;
        }
        self.eof()
    }
    pub(crate) fn eof(&mut self) -> Result<()> {
        if self.state.depth != 0 || !self.state.root_done {
            return self.state.fail(ErrorKind::UnconsumedChild);
        }
        self.lease(|p, _, base| p.finish().map_err(|e| syntax(e, base)))
    }
}

fn append(keys: &mut String, text: &str, offset: usize) -> Result<Range<usize>> {
    let start = keys.len();
    if text.len() > keys.capacity() - start {
        return Err(at(ErrorKind::InternalInvariant, offset));
    }
    keys.push_str(text);
    Ok(start..keys.len())
}
