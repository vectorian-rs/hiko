use crate::core::{Context, State};
use crate::reader::{DECODER_FIXED_ALLOWANCE, fixed_state_bytes};
use crate::{Kind, Limits, Result, Scratch, Stats};
use std::{mem, sync::Arc};

/// Owned input and state; no parser/guard borrows live across calls.
/// A runtime must pre-admit `admission_bytes` before constructing this value,
/// and account outputs separately. Standalone limits are not VM work credits.
pub struct OwnedReader {
    input: Arc<str>,
    scratch: Scratch,
    state: State,
}
impl OwnedReader {
    pub fn new(input: Arc<str>, limits: Limits) -> Result<Self> {
        let mut scratch = Scratch::default();
        let (state, _) = State::new(input.as_bytes(), &mut scratch, limits)?;
        Ok(Self {
            input,
            scratch,
            state,
        })
    }
    /// Conservative fresh-construction estimate: input, two n-byte buffers,
    /// fixed state/decoder allowance. Not actual allocated bytes/RSS.
    pub fn admission_bytes(input_bytes: usize) -> Option<usize> {
        input_bytes
            .checked_mul(3)?
            .checked_add(fixed_state_bytes())?
            .checked_add(mem::size_of::<Self>())?
            .checked_add(2 * mem::size_of::<usize>())
    }
    /// Canonical retained payload estimate, including temporary decoder allowance.
    pub fn estimated_bytes(&self) -> usize {
        self.input.len()
            + self.scratch.retained_bytes()
            + mem::size_of::<Self>()
            + DECODER_FIXED_ALLOWANCE
            + 2 * mem::size_of::<usize>()
    }
    pub fn input_bytes(&self) -> usize {
        self.input.len()
    }
    pub fn capacities(&self) -> (usize, usize) {
        self.scratch.capacities()
    }
    pub fn stats(&self) -> Stats {
        Stats {
            advanced_bytes: self.state.offset,
            ..self.state.stats
        }
    }
    fn call<T>(&mut self, f: impl FnOnce(&mut Context<'_>) -> Result<T>) -> Result<T> {
        let mut c = Context {
            input: &self.input,
            scratch: &mut self.scratch,
            state: &mut self.state,
        };
        c.api()?;
        f(&mut c)
    }
    pub fn kind(&mut self) -> Result<Kind> {
        self.call(|c| c.kind())
    }
    pub fn begin_object(&mut self) -> Result<()> {
        self.call(|c| c.enter(Kind::Object))
    }
    pub fn begin_array(&mut self) -> Result<()> {
        self.call(|c| c.enter(Kind::Array))
    }
    pub fn next_field(&mut self) -> Result<Option<&str>> {
        let range = self.call(|c| c.key(true))?;
        if let Some(range) = range {
            self.scratch.text_start = Some(range.start);
            Ok(Some(&self.scratch.keys[range]))
        } else {
            Ok(None)
        }
    }
    pub fn find_next_field(&mut self, name: &str) -> Result<bool> {
        self.call(|c| {
            while let Some(range) = c.key(true)? {
                let matches = c.scratch.keys[range.clone()] == *name;
                c.scratch.keys.truncate(range.start);
                if matches {
                    return Ok(true);
                }
                c.skip()?;
            }
            Ok(false)
        })
    }
    pub fn next_item(&mut self) -> Result<bool> {
        self.call(|c| c.array_next())
    }
    pub fn read_null(&mut self) -> Result<()> {
        self.call(|c| c.null())
    }
    pub fn read_bool(&mut self) -> Result<bool> {
        self.call(|c| c.boolean())
    }
    pub fn read_string(&mut self) -> Result<&str> {
        let range = self.call(|c| c.string())?;
        Ok(&self.scratch.keys[range])
    }
    pub fn read_number_text(&mut self) -> Result<&str> {
        let range = self.call(|c| c.number())?;
        Ok(&self.input[range])
    }
    pub fn read_int(&mut self) -> Result<i64> {
        self.call(|c| c.integer())
    }
    pub fn read_word(&mut self) -> Result<u64> {
        self.call(|c| c.integer())
    }
    pub fn read_float(&mut self) -> Result<f64> {
        self.call(|c| c.float())
    }
    pub fn skip(&mut self) -> Result<()> {
        self.call(|c| c.skip())
    }
    pub fn finish_object(&mut self) -> Result<()> {
        self.call(|c| c.finish_container(Kind::Object))
    }
    pub fn finish_array(&mut self) -> Result<()> {
        self.call(|c| c.finish_container(Kind::Array))
    }
    /// Drain all open frames/root and require EOF. Runtime finish then releases
    /// the owner; this standalone frontend, like Reader, permits repeated EOF.
    pub fn finish(&mut self) -> Result<()> {
        self.call(|c| c.finish_all())
    }
}
