use crate::{Error, ErrorKind, Result};
use jiter::{Jiter, Peek};
use std::{mem, ops::Range};

const MAX_DEPTH: usize = 128;
// Canonical conservative allowance for fixed backend decoder/conversion payloads.
// lexical-parse-float uses fixed StackVec limbs, not an allocating bigint.
// This is an accounting estimate, never an exact native stack/RSS measurement.
const DECODER_FIXED_ALLOWANCE: usize = 64 * 1024;

/// Independent experimental reader limits, not Hiko VM configuration.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_scratch_bytes: usize,
    pub max_work: u64,
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1 << 20,
            max_scratch_bytes: 4 << 20,
            max_work: 1_000_000_000,
            max_depth: MAX_DEPTH,
        }
    }
}

/// Reusable exclusively borrowed storage. No cached document or tree.
#[derive(Default, Debug)]
pub struct Scratch {
    tape: Vec<u8>,
    keys: String,
}

impl Scratch {
    pub fn retained_bytes(&self) -> usize {
        self.tape.capacity().saturating_add(self.keys.capacity())
    }
    pub fn capacities(&self) -> (usize, usize) {
        (self.tape.capacity(), self.keys.capacity())
    }

    fn peak_for(&self, n: usize) -> Option<usize> {
        let mut live = self.tape.capacity().checked_add(self.keys.capacity())?;
        let mut peak = live;
        if self.tape.capacity() < n {
            peak = peak.max(live.checked_add(n)?);
            live = n.checked_add(self.keys.capacity())?;
        }
        if self.keys.capacity() < n {
            peak = peak.max(live.checked_add(n)?);
        }
        peak.checked_add(fixed_state_bytes())
    }

    fn prepare(&mut self, n: usize) -> Result<()> {
        if self.tape.capacity() < n {
            let mut tape = Vec::new();
            tape.try_reserve_exact(n)
                .map_err(|_| at(ErrorKind::Allocation, 0))?;
            self.tape = tape;
        }
        if self.keys.capacity() < n {
            let mut keys = String::new();
            keys.try_reserve_exact(n)
                .map_err(|_| at(ErrorKind::Allocation, 0))?;
            self.keys = keys;
        }
        self.tape.clear();
        self.keys.clear();
        Ok(())
    }
}

/// Canonical estimated state/guard/decoder payload, not machine stack or RSS.
fn fixed_state_bytes() -> usize {
    mem::size_of::<Reader<'_, '_>>()
        + MAX_DEPTH
            * (mem::size_of::<Value<'_, '_, '_>>()
                + mem::size_of::<Field<'_, '_, '_>>()
                + mem::size_of::<Object<'_, '_, '_>>()
                + mem::size_of::<Array<'_, '_, '_>>())
        + MAX_DEPTH
        + DECODER_FIXED_ALLOWANCE
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub advanced_bytes: usize,
    pub backend_calls: u64,
    pub charged_work: u64,
    pub peak_scratch_bytes: usize,
}

/// One forward-only cursor borrowing both immutable input and reusable scratch.
pub struct Reader<'input, 'scratch> {
    cursor: Cursor<'input, 'scratch>,
    root_started: bool,
    root_done: bool,
}

impl<'i, 's> Reader<'i, 's> {
    pub fn new(input: &'i [u8], scratch: &'s mut Scratch, limits: Limits) -> Result<Self> {
        if input.len() > limits.max_input_bytes {
            return Err(at(ErrorKind::InputLimit, 0));
        }
        if limits.max_depth > MAX_DEPTH {
            return Err(at(ErrorKind::DepthLimit, 0));
        }
        // Canonical abstract byte-work schedule: 32 credits per input byte for
        // validation/scanning/decoding/key copies/conversion, plus 256 fixed;
        // every backend invocation additionally costs one. Not CPU calibration.
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
        // Range-only jiter string skipping does not check UTF-8. Validate once,
        // before any parsing; this alone is NOT whole-document JSON validation.
        let input =
            std::str::from_utf8(input).map_err(|e| at(ErrorKind::InvalidUtf8, e.valid_up_to()))?;
        scratch.prepare(input.len())?;
        let parser = Jiter::with_tape(input.as_bytes(), mem::take(&mut scratch.tape));
        Ok(Self {
            cursor: Cursor {
                input,
                parser: Some(parser),
                scratch,
                limits,
                poisoned: false,
                depth: 0,
                stats: Stats {
                    charged_work: prepaid,
                    peak_scratch_bytes: peak,
                    ..Stats::default()
                },
            },
            root_started: false,
            root_done: false,
        })
    }

    pub fn root(&mut self) -> Result<Value<'_, 'i, 's>> {
        self.cursor.healthy()?;
        if self.root_started {
            return self.cursor.fail(ErrorKind::Consumed);
        }
        self.root_started = true;
        Ok(Value::new(&mut self.cursor, &mut self.root_done))
    }

    /// Validate everything remaining, then require EOF. Earlier extraction alone
    /// does not certify the input. Dropped unfinished children cannot be resumed.
    pub fn finish(&mut self) -> Result<()> {
        self.cursor.healthy()?;
        if !self.root_started {
            self.root()?.skip()?;
        }
        if !self.root_done {
            return self.cursor.fail(ErrorKind::UnconsumedChild);
        }
        self.cursor.step(|p| p.finish())
    }

    pub fn stats(&self) -> Stats {
        Stats {
            advanced_bytes: self.cursor.position(),
            ..self.cursor.stats
        }
    }
}

struct Cursor<'i, 's> {
    input: &'i str,
    parser: Option<Jiter<'i>>,
    scratch: &'s mut Scratch,
    limits: Limits,
    poisoned: bool,
    depth: usize,
    stats: Stats,
}

impl Drop for Cursor<'_, '_> {
    fn drop(&mut self) {
        if let Some(parser) = self.parser.take() {
            self.scratch.tape = parser.into_tape();
        }
        self.scratch.tape.clear();
        self.scratch.keys.clear();
    }
}

fn at(kind: ErrorKind, offset: usize) -> Error {
    Error { kind, offset }
}

impl<'i> Cursor<'i, '_> {
    fn position(&self) -> usize {
        self.parser.as_ref().map_or(0, Jiter::current_index)
    }
    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            Err(at(ErrorKind::Poisoned, self.position()))
        } else {
            Ok(())
        }
    }
    fn fail<T>(&mut self, kind: ErrorKind) -> Result<T> {
        self.poisoned = true;
        Err(at(kind, self.position()))
    }
    fn charge(&mut self) -> Result<()> {
        self.healthy()?;
        if self.stats.charged_work >= self.limits.max_work {
            return self.fail(ErrorKind::WorkLimit);
        }
        self.stats.charged_work += 1;
        self.stats.backend_calls += 1;
        Ok(())
    }
    fn step<T>(&mut self, f: impl FnOnce(&mut Jiter<'i>) -> jiter::JiterResult<T>) -> Result<T> {
        self.charge()?;
        let p = self
            .parser
            .as_mut()
            .ok_or_else(|| at(ErrorKind::InternalInvariant, 0))?;
        f(p).map_err(|e| {
            self.poisoned = true;
            at(ErrorKind::Syntax(e.error_type), e.index)
        })
    }
    fn kind(&mut self) -> Result<Kind> {
        let p = self.step(|p| p.peek())?;
        match p {
            Peek::Null => Ok(Kind::Null),
            Peek::True | Peek::False => Ok(Kind::Boolean),
            Peek::String => Ok(Kind::String),
            Peek::Array => Ok(Kind::Array),
            Peek::Object => Ok(Kind::Object),
            _ if p.is_num() => Ok(Kind::Number),
            _ => self.fail(ErrorKind::WrongKind),
        }
    }
    fn require_kind(&mut self, expected: Kind) -> Result<()> {
        if self.kind()? != expected {
            return self.fail(ErrorKind::WrongKind);
        }
        Ok(())
    }
    fn enter(&mut self, expected: Kind) -> Result<()> {
        self.require_kind(expected)?;
        if self.depth >= self.limits.max_depth {
            return self.fail(ErrorKind::DepthLimit);
        }
        self.depth += 1;
        Ok(())
    }
    fn number(&mut self) -> Result<&'i str> {
        self.require_kind(Kind::Number)?;
        let start = self.position();
        self.step(|p| p.next_number_bytes().map(|_| ()))?;
        self.input
            .get(start..self.position())
            .ok_or_else(|| at(ErrorKind::InternalInvariant, start))
    }
    fn string(&mut self) -> Result<&str> {
        // Typed jiter next_* errors can call wrong_num -> NumberAny. Reject
        // mismatched kinds first, and use known_str so that path is unreachable.
        self.require_kind(Kind::String)?;
        self.charge()?;
        let p = self
            .parser
            .as_mut()
            .ok_or_else(|| at(ErrorKind::InternalInvariant, 0))?;
        match p.known_str() {
            Ok(s) => Ok(s),
            Err(e) => {
                self.poisoned = true;
                Err(at(ErrorKind::Syntax(e.error_type), e.index))
            }
        }
    }
    fn key(&mut self, first: bool, retain: bool) -> Result<Option<Range<usize>>> {
        self.charge()?;
        let p = self
            .parser
            .as_mut()
            .ok_or_else(|| at(ErrorKind::InternalInvariant, 0))?;
        let key = if first { p.next_object() } else { p.next_key() };
        match key {
            Ok(Some(key)) => {
                let start = self.scratch.keys.len();
                if retain {
                    if key.len() > self.scratch.keys.capacity() - start {
                        self.poisoned = true;
                        return Err(at(ErrorKind::InternalInvariant, p.current_index()));
                    }
                    self.scratch.keys.push_str(key);
                }
                Ok(Some(start..self.scratch.keys.len()))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                self.poisoned = true;
                Err(at(ErrorKind::Syntax(e.error_type), e.index))
            }
        }
    }

    // Iterative skip using public primitives; no DOM and no backend aggregate
    // skip (which has its own independent depth policy). Stack storage is fixed.
    fn skip(&mut self) -> Result<()> {
        let mut objects = [false; MAX_DEPTH];
        let mut level = 0;
        loop {
            match self.kind()? {
                Kind::Object => {
                    self.enter(Kind::Object)?;
                    if self.key(true, false)?.is_some() {
                        objects[level] = true;
                        level += 1;
                        continue;
                    }
                    self.depth -= 1;
                }
                Kind::Array => {
                    self.enter(Kind::Array)?;
                    if self.step(|p| p.next_array())?.is_some() {
                        objects[level] = false;
                        level += 1;
                        continue;
                    }
                    self.depth -= 1;
                }
                Kind::String => {
                    self.step(|p| p.next_bytes().map(|_| ()))?;
                }
                Kind::Number => {
                    self.number()?;
                }
                Kind::Null => self.step(|p| p.next_null())?,
                Kind::Boolean => {
                    self.step(|p| p.next_bool())?;
                }
            }
            loop {
                if level == 0 {
                    return Ok(());
                }
                let more = if objects[level - 1] {
                    self.key(false, false)?.is_some()
                } else {
                    self.step(|p| p.array_step())?.is_some()
                };
                if more {
                    break;
                }
                level -= 1;
                self.depth -= 1;
            }
        }
    }
}

/// Lending single-use value. Drop poisons; it never silently parses/skips.
pub struct Value<'r, 'i, 's> {
    cursor: Option<&'r mut Cursor<'i, 's>>,
    complete: Option<&'r mut bool>,
}

impl<'r, 'i, 's> Value<'r, 'i, 's> {
    fn new(cursor: &'r mut Cursor<'i, 's>, complete: &'r mut bool) -> Self {
        Self {
            cursor: Some(cursor),
            complete: Some(complete),
        }
    }
    fn cursor(&mut self) -> Result<&mut Cursor<'i, 's>> {
        let c = self
            .cursor
            .as_deref_mut()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        c.healthy()?;
        if self.complete.as_deref().copied().unwrap_or(true) {
            return c.fail(ErrorKind::Consumed);
        }
        Ok(c)
    }
    fn mark_complete(&mut self) {
        if let Some(done) = &mut self.complete {
            **done = true;
        }
    }
    pub fn kind(&mut self) -> Result<Kind> {
        self.cursor()?.kind()
    }
    pub fn null(&mut self) -> Result<()> {
        self.cursor()?.require_kind(Kind::Null)?;
        self.cursor()?.step(|p| p.known_null())?;
        self.mark_complete();
        Ok(())
    }
    pub fn bool(&mut self) -> Result<bool> {
        self.cursor()?.require_kind(Kind::Boolean)?;
        let b = self.cursor()?.step(|p| p.next_bool())?;
        self.mark_complete();
        Ok(b)
    }
    pub fn number_text(&mut self) -> Result<&'i str> {
        let n = self.cursor()?.number()?;
        self.mark_complete();
        Ok(n)
    }
    fn integer<T: std::str::FromStr>(&mut self) -> Result<T> {
        let n = self.number_text()?;
        let c = self
            .cursor
            .as_deref_mut()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        let start = c.position() - n.len();
        if !integer_token(n) {
            c.poisoned = true;
            return Err(at(ErrorKind::IntegerTokenRequired, start));
        }
        n.parse().map_err(|_| {
            c.poisoned = true;
            at(ErrorKind::NumberRange, start)
        })
    }
    pub fn i64(&mut self) -> Result<i64> {
        self.integer()
    }
    pub fn u64(&mut self) -> Result<u64> {
        self.integer()
    }
    pub fn f64(&mut self) -> Result<f64> {
        let n = self.number_text()?;
        let c = self
            .cursor
            .as_deref_mut()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        let start = c.position() - n.len();
        c.charge()?;
        // lexical-parse-float's fixed-capacity bigint fallback is audited in the
        // resource note. Never invoke JsonValue/NumberAny or bigint allocation.
        let f = jiter::NumberFloat::from_bytes(n.as_bytes(), false)
            .map_err(|e| {
                c.poisoned = true;
                at(
                    ErrorKind::Syntax(jiter::JiterErrorType::JsonError(e.error_type)),
                    start + e.index,
                )
            })?
            .0;
        if !f.is_finite() {
            return c.fail(ErrorKind::NonFinite);
        }
        if f == 0.0
            && n.bytes()
                .take_while(|b| *b != b'e' && *b != b'E')
                .any(|b| matches!(b, b'1'..=b'9'))
        {
            return c.fail(ErrorKind::Underflow);
        }
        Ok(f)
    }
    pub fn string(&mut self) -> Result<&str> {
        self.cursor()?;
        self.mark_complete();
        self.cursor
            .as_deref_mut()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?
            .string()
    }
    pub fn skip(&mut self) -> Result<()> {
        self.cursor()?.skip()?;
        self.mark_complete();
        Ok(())
    }
    pub fn object(mut self) -> Result<Object<'r, 'i, 's>> {
        self.cursor()?.enter(Kind::Object)?;
        let cursor = self
            .cursor
            .take()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        let complete = self
            .complete
            .take()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        Ok(Object {
            cursor,
            complete,
            first: true,
            child_done: true,
        })
    }
    pub fn array(mut self) -> Result<Array<'r, 'i, 's>> {
        self.cursor()?.enter(Kind::Array)?;
        let cursor = self
            .cursor
            .take()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        let complete = self
            .complete
            .take()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        Ok(Array {
            cursor,
            complete,
            first: true,
            child_done: true,
        })
    }
}

fn integer_token(n: &str) -> bool {
    !n.bytes().any(|b| matches!(b, b'.' | b'e' | b'E'))
}

impl Drop for Value<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.complete.as_deref().copied().unwrap_or(true)
            && let Some(c) = &mut self.cursor
        {
            c.poisoned = true;
        }
    }
}

pub struct Object<'r, 'i, 's> {
    cursor: &'r mut Cursor<'i, 's>,
    complete: &'r mut bool,
    first: bool,
    child_done: bool,
}

impl<'i, 's> Object<'_, 'i, 's> {
    fn advance(&mut self) -> Result<Option<Range<usize>>> {
        self.cursor.healthy()?;
        if !self.child_done {
            return self.cursor.fail(ErrorKind::UnconsumedChild);
        }
        if *self.complete {
            return Ok(None);
        }
        let first = mem::replace(&mut self.first, false);
        let key = self.cursor.key(first, true)?;
        if key.is_some() {
            self.child_done = false;
        } else {
            *self.complete = true;
            self.cursor.depth -= 1;
        }
        Ok(key)
    }
    pub fn next_field(&mut self) -> Result<Option<Field<'_, 'i, 's>>> {
        let Some(key) = self.advance()? else {
            return Ok(None);
        };
        Ok(Some(Field {
            cursor: self.cursor,
            complete: &mut self.child_done,
            consumed: false,
            key,
        }))
    }
    /// Search only remaining fields, preserving source order and duplicates.
    pub fn find_next_field(&mut self, name: &str) -> Result<Option<Field<'_, 'i, 's>>> {
        while let Some(key) = self.advance()? {
            if &self.cursor.scratch.keys[key.clone()] == name {
                return Ok(Some(Field {
                    cursor: self.cursor,
                    complete: &mut self.child_done,
                    consumed: false,
                    key,
                }));
            }
            self.cursor.skip()?;
            self.cursor.scratch.keys.truncate(key.start);
            self.child_done = true;
        }
        Ok(None)
    }
    pub fn finish(mut self) -> Result<()> {
        while let Some(field) = self.next_field()? {
            field.finish()?;
        }
        Ok(())
    }
}
impl Drop for Object<'_, '_, '_> {
    fn drop(&mut self) {
        if !*self.complete {
            self.cursor.poisoned = true;
        }
    }
}

pub struct Field<'r, 'i, 's> {
    cursor: &'r mut Cursor<'i, 's>,
    complete: &'r mut bool,
    consumed: bool,
    key: Range<usize>,
}
impl<'i, 's> Field<'_, 'i, 's> {
    pub fn key(&self) -> &str {
        &self.cursor.scratch.keys[self.key.clone()]
    }
    pub fn value(&mut self) -> Result<Value<'_, 'i, 's>> {
        self.cursor.healthy()?;
        if self.consumed {
            return self.cursor.fail(ErrorKind::Consumed);
        }
        Ok(Value::new(self.cursor, &mut self.consumed))
    }
    pub fn finish(mut self) -> Result<()> {
        self.cursor.healthy()?;
        if !self.consumed {
            self.value()?.skip()?;
        }
        Ok(())
    }
}
impl Drop for Field<'_, '_, '_> {
    fn drop(&mut self) {
        self.cursor.scratch.keys.truncate(self.key.start);
        if self.consumed {
            *self.complete = true;
        } else {
            self.cursor.poisoned = true;
        }
    }
}

pub struct Array<'r, 'i, 's> {
    cursor: &'r mut Cursor<'i, 's>,
    complete: &'r mut bool,
    first: bool,
    child_done: bool,
}
impl<'i, 's> Array<'_, 'i, 's> {
    pub fn next_value(&mut self) -> Result<Option<Value<'_, 'i, 's>>> {
        self.cursor.healthy()?;
        if !self.child_done {
            return self.cursor.fail(ErrorKind::UnconsumedChild);
        }
        if *self.complete {
            return Ok(None);
        }
        let next = if mem::replace(&mut self.first, false) {
            self.cursor.step(|p| p.next_array())?
        } else {
            self.cursor.step(|p| p.array_step())?
        };
        if next.is_none() {
            *self.complete = true;
            self.cursor.depth -= 1;
            return Ok(None);
        }
        self.child_done = false;
        Ok(Some(Value::new(self.cursor, &mut self.child_done)))
    }
    pub fn finish(mut self) -> Result<()> {
        while let Some(mut value) = self.next_value()? {
            value.skip()?;
        }
        Ok(())
    }
}
impl Drop for Array<'_, '_, '_> {
    fn drop(&mut self) {
        if !*self.complete {
            self.cursor.poisoned = true;
        }
    }
}
