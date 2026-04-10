use hiko_syntax::span::Span;

use crate::op::Op;

#[derive(Debug, Clone, Default)]
pub struct Chunk {
    pub code: Vec<u8>,
    pub constants: Vec<Constant>,
    /// Maps bytecode offsets to source spans (sorted by offset).
    pub spans: Vec<(usize, Span)>,
}

#[derive(Debug, Clone)]
pub enum Constant {
    Int(i64),
    Float(f64),
    String(String),
    Char(char),
}

#[derive(Debug, Clone)]
pub struct FunctionProto {
    pub name: Option<String>,
    pub arity: u8,
    pub n_captures: u8,
    pub chunk: Chunk,
}

#[derive(Debug, Clone)]
pub struct CompiledProgram {
    pub main: Chunk,
    pub functions: Vec<FunctionProto>,
}

impl Chunk {
    pub fn emit_op(&mut self, op: Op) {
        self.code.push(op as u8);
    }

    /// Record a source span for the current bytecode position.
    pub fn record_span(&mut self, span: Span) {
        let offset = self.code.len();
        // Deduplicate: don't record if same offset already has a span
        if self.spans.last().is_some_and(|(off, _)| *off == offset) {
            return;
        }
        self.spans.push((offset, span));
    }

    /// Look up the source span for a bytecode offset.
    pub fn span_at(&self, offset: usize) -> Option<Span> {
        // Binary search for the largest offset <= target
        match self.spans.binary_search_by_key(&offset, |(off, _)| *off) {
            Ok(i) => Some(self.spans[i].1),
            Err(0) => None,
            Err(i) => Some(self.spans[i - 1].1),
        }
    }

    pub fn emit_u8(&mut self, val: u8) {
        self.code.push(val);
    }

    pub fn emit_u16(&mut self, val: u16) {
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    pub fn emit_i16(&mut self, val: i16) {
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    pub fn add_constant(&mut self, c: Constant) -> Result<u16, String> {
        let idx = self.constants.len();
        let idx = u16::try_from(idx)
            .map_err(|_| format!("constant pool overflow: {idx} exceeds u16::MAX"))?;
        self.constants.push(c);
        Ok(idx)
    }

    /// Emit a jump instruction, return the offset to patch later.
    pub fn emit_jump(&mut self, op: Op) -> usize {
        self.emit_op(op);
        let pos = self.code.len();
        self.emit_i16(0);
        pos
    }

    /// Patch a previously emitted jump to point to the current position.
    pub fn patch_jump(&mut self, pos: usize) -> Result<(), String> {
        let target = i16::try_from(self.code.len()).map_err(|_| {
            format!(
                "jump target overflow: bytecode offset {} exceeds i16::MAX",
                self.code.len()
            )
        })?;
        let origin = i16::try_from(pos + 2).map_err(|_| {
            format!(
                "jump origin overflow: bytecode offset {} exceeds i16::MAX",
                pos + 2
            )
        })?;
        let offset = target - origin;
        let bytes = offset.to_le_bytes();
        self.code[pos] = bytes[0];
        self.code[pos + 1] = bytes[1];
        Ok(())
    }

    pub fn read_u8(code: &[u8], ip: &mut usize) -> u8 {
        let val = code[*ip];
        *ip += 1;
        val
    }

    pub fn read_u16(code: &[u8], ip: &mut usize) -> u16 {
        let val = u16::from_le_bytes([code[*ip], code[*ip + 1]]);
        *ip += 2;
        val
    }

    pub fn read_i16(code: &[u8], ip: &mut usize) -> i16 {
        let val = i16::from_le_bytes([code[*ip], code[*ip + 1]]);
        *ip += 2;
        val
    }
}
