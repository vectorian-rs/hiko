use crate::core::{Context, State, at};
use crate::{ErrorKind, Result};
use std::{mem, ops::Range};

pub(crate) const MAX_DEPTH: usize = 128;
// Fixed decoder/conversion payload estimate; not actual native stack or RSS.
pub(crate) const DECODER_FIXED_ALLOWANCE: usize = 64 * 1024;

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
    pub(crate) tape: Vec<u8>,
    pub(crate) keys: String,
    pub(crate) text_start: Option<usize>,
}
impl Scratch {
    pub fn retained_bytes(&self) -> usize {
        self.tape.capacity().saturating_add(self.keys.capacity())
    }
    pub fn capacities(&self) -> (usize, usize) {
        (self.tape.capacity(), self.keys.capacity())
    }
    pub(crate) fn peak_for(&self, n: usize) -> Option<usize> {
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
    pub(crate) fn prepare(&mut self, n: usize) -> Result<()> {
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
        self.clear();
        Ok(())
    }
    pub(crate) fn release_text(&mut self) {
        if let Some(start) = self.text_start.take() {
            self.keys.truncate(start);
        }
    }
    pub(crate) fn clear(&mut self) {
        self.tape.clear();
        self.keys.clear();
        self.text_start = None;
    }
}

/// Canonical estimated state/guard/decoder payload, not machine stack or RSS.
pub(crate) fn fixed_state_bytes() -> usize {
    mem::size_of::<Reader<'_, '_>>()
        + MAX_DEPTH
            * (mem::size_of::<Value<'_, '_, '_>>()
                + mem::size_of::<Field<'_, '_, '_>>()
                + mem::size_of::<Object<'_, '_, '_>>()
                + mem::size_of::<Array<'_, '_, '_>>())
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

/// Untimed counters. API and backend credits are standalone abstract units.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub advanced_bytes: usize,
    pub backend_calls: u64,
    pub charged_work: u64,
    pub peak_scratch_bytes: usize,
    pub api_calls: u64,
    pub classification_peeks: u64,
    pub float_conversions: u64,
}

/// Lending frontend over the same input-independent core as OwnedReader.
pub struct Reader<'input, 'scratch> {
    cursor: Cursor<'input, 'scratch>,
    root_started: bool,
    root_done: bool,
}
impl<'i, 's> Reader<'i, 's> {
    pub fn new(input: &'i [u8], scratch: &'s mut Scratch, limits: Limits) -> Result<Self> {
        let (state, input) = State::new(input, scratch, limits)?;
        Ok(Self {
            cursor: Cursor {
                input,
                scratch,
                state,
            },
            root_started: false,
            root_done: false,
        })
    }
    pub fn root(&mut self) -> Result<Value<'_, 'i, 's>> {
        self.cursor.state.healthy()?;
        if self.root_started {
            return self.cursor.state.fail(ErrorKind::Consumed);
        }
        self.root_started = true;
        Ok(Value::new(&mut self.cursor, &mut self.root_done))
    }
    /// Unopened root is drained; an unfinished lending guard is an error.
    /// Earlier extraction does not validate the suffix. Repeated finish succeeds.
    pub fn finish(&mut self) -> Result<()> {
        self.cursor.state.healthy()?;
        if !self.root_started {
            self.root()?.skip()?;
        }
        if !self.root_done {
            return self.cursor.state.fail(ErrorKind::UnconsumedChild);
        }
        self.cursor.call(|c| c.eof())
    }
    pub fn stats(&self) -> Stats {
        Stats {
            advanced_bytes: self.cursor.state.offset,
            ..self.cursor.state.stats
        }
    }
}

struct Cursor<'i, 's> {
    input: &'i str,
    scratch: &'s mut Scratch,
    state: State,
}
impl Cursor<'_, '_> {
    fn call<T>(&mut self, f: impl FnOnce(&mut Context<'_>) -> Result<T>) -> Result<T> {
        let mut c = Context {
            input: self.input,
            scratch: self.scratch,
            state: &mut self.state,
        };
        c.api()?;
        f(&mut c)
    }
}
impl Drop for Cursor<'_, '_> {
    fn drop(&mut self) {
        self.scratch.clear();
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
        c.state.healthy()?;
        if self.complete.as_deref().copied().unwrap_or(true) {
            return c.state.fail(ErrorKind::Consumed);
        }
        Ok(c)
    }
    fn mark_complete(&mut self) {
        if let Some(done) = &mut self.complete {
            **done = true;
        }
    }
    pub fn kind(&mut self) -> Result<Kind> {
        self.cursor()?.call(|c| c.kind())
    }
    pub fn null(&mut self) -> Result<()> {
        self.cursor()?.call(|c| c.null())?;
        self.mark_complete();
        Ok(())
    }
    pub fn bool(&mut self) -> Result<bool> {
        let b = self.cursor()?.call(|c| c.boolean())?;
        self.mark_complete();
        Ok(b)
    }
    pub fn number_text(&mut self) -> Result<&'i str> {
        let c = self.cursor()?;
        let range = c.call(|c| c.number())?;
        let text = &c.input[range];
        self.mark_complete();
        Ok(text)
    }
    fn integer<T: std::str::FromStr>(&mut self) -> Result<T> {
        let value = self.cursor()?.call(|c| c.integer())?;
        self.mark_complete();
        Ok(value)
    }
    pub fn i64(&mut self) -> Result<i64> {
        self.integer()
    }
    pub fn u64(&mut self) -> Result<u64> {
        self.integer()
    }
    pub fn f64(&mut self) -> Result<f64> {
        let f = self.cursor()?.call(|c| c.float())?;
        self.mark_complete();
        Ok(f)
    }
    pub fn string(&mut self) -> Result<&str> {
        self.cursor()?;
        self.mark_complete();
        let c = self
            .cursor
            .as_deref_mut()
            .ok_or_else(|| at(ErrorKind::Consumed, 0))?;
        let range = c.call(|c| c.string())?;
        Ok(&c.scratch.keys[range])
    }
    pub fn skip(&mut self) -> Result<()> {
        self.cursor()?.call(|c| c.skip())?;
        self.mark_complete();
        Ok(())
    }
    pub fn object(self) -> Result<Object<'r, 'i, 's>> {
        Ok(Object(self.into_container(Kind::Object)?))
    }
    pub fn array(self) -> Result<Array<'r, 'i, 's>> {
        Ok(Array(self.into_container(Kind::Array)?))
    }
    fn into_container(mut self, kind: Kind) -> Result<Container<'r, 'i, 's>> {
        self.cursor()?.call(|c| c.enter(kind))?;
        match (self.cursor.take(), self.complete.take()) {
            (Some(cursor), Some(complete)) => Ok(Container {
                cursor,
                complete,
                child_done: true,
            }),
            _ => Err(at(ErrorKind::Consumed, 0)),
        }
    }
}
impl Drop for Value<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.complete.as_deref().copied().unwrap_or(true)
            && let Some(c) = &mut self.cursor
        {
            c.state.poisoned = true;
        }
    }
}

// The booleans are lending-guard completion only; navigation lives in core::State.
struct Container<'r, 'i, 's> {
    cursor: &'r mut Cursor<'i, 's>,
    complete: &'r mut bool,
    child_done: bool,
}
impl Container<'_, '_, '_> {
    fn ready(&mut self) -> Result<bool> {
        self.cursor.state.healthy()?;
        if !self.child_done {
            return self.cursor.state.fail(ErrorKind::UnconsumedChild);
        }
        Ok(!*self.complete)
    }
    fn advanced(&mut self, more: bool) {
        if more {
            self.child_done = false;
        } else {
            *self.complete = true;
        }
    }
}
impl Drop for Container<'_, '_, '_> {
    fn drop(&mut self) {
        if !*self.complete {
            self.cursor.state.poisoned = true;
        } else {
            self.cursor.state.pop_ended();
        }
    }
}
pub struct Object<'r, 'i, 's>(Container<'r, 'i, 's>);
impl<'i, 's> Object<'_, 'i, 's> {
    fn advance(&mut self) -> Result<Option<Range<usize>>> {
        if !self.0.ready()? {
            return Ok(None);
        }
        let key = self.0.cursor.call(|c| c.key(true))?;
        self.0.advanced(key.is_some());
        Ok(key)
    }
    fn field(&mut self, key: Range<usize>) -> Field<'_, 'i, 's> {
        Field {
            cursor: self.0.cursor,
            complete: &mut self.0.child_done,
            consumed: false,
            key,
        }
    }
    pub fn next_field(&mut self) -> Result<Option<Field<'_, 'i, 's>>> {
        Ok(self.advance()?.map(|key| self.field(key)))
    }
    /// Searches only remaining fields; literal names, source order, duplicates.
    pub fn find_next_field(&mut self, name: &str) -> Result<Option<Field<'_, 'i, 's>>> {
        while let Some(key) = self.advance()? {
            let c = &mut self.0;
            if c.cursor.scratch.keys[key.clone()] == *name {
                return Ok(Some(self.field(key)));
            }
            c.cursor.call(|c| c.skip())?;
            c.cursor.scratch.keys.truncate(key.start);
            c.child_done = true;
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
        self.cursor.state.healthy()?;
        if self.consumed {
            return self.cursor.state.fail(ErrorKind::Consumed);
        }
        Ok(Value::new(self.cursor, &mut self.consumed))
    }
    pub fn finish(mut self) -> Result<()> {
        self.cursor.state.healthy()?;
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
            self.cursor.state.poisoned = true;
        }
    }
}
pub struct Array<'r, 'i, 's>(Container<'r, 'i, 's>);
impl<'i, 's> Array<'_, 'i, 's> {
    pub fn next_value(&mut self) -> Result<Option<Value<'_, 'i, 's>>> {
        let c = &mut self.0;
        if !c.ready()? {
            return Ok(None);
        }
        let more = c.cursor.call(|c| c.array_next())?;
        c.advanced(more);
        Ok(more.then(|| Value::new(c.cursor, &mut c.child_done)))
    }
    pub fn finish(mut self) -> Result<()> {
        while let Some(mut value) = self.next_value()? {
            value.skip()?;
        }
        Ok(())
    }
}
