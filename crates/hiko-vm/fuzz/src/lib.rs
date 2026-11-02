//! Shared code for the hiko-vm fuzz targets: a byte encoding of
//! `CompiledProgram`, so libFuzzer can mutate real compiled programs, and
//! the bounded VM the targets run accepted programs in.

use std::sync::Arc;

use hiko_builtin_meta::builtins;
use hiko_compile::chunk::{Chunk, CompiledProgram, Constant, EffectMeta, FunctionProto};
use hiko_vm::builder::VMBuilder;
use hiko_vm::vm::OutputSink;

/// Largest code or table a decoded program may have, so one input cannot
/// ask for gigabytes.
const MAX_LEN: usize = 1 << 16;

/// Builtins a fuzzed program may call: the ones with no host access.
/// `print` and `println` write to a sink that drops the text.
const SAFE_FAMILIES: &[&str] = &[
    "capabilities.convert.",
    "capabilities.string.",
    "capabilities.regex.",
    "capabilities.json.",
    "capabilities.math.",
    "capabilities.bytes.",
    "capabilities.hash.",
    "capabilities.stdio.print",
];

struct DropOutput;

impl OutputSink for DropOutput {
    fn write(&self, _text: &str) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run a program with small work and heap limits. The result is ignored:
/// the property is only that the VM does not panic.
pub fn run_bounded(program: CompiledProgram) {
    let mut builder = VMBuilder::new(program)
        .max_work(20_000)
        .max_heap(20_000)
        .max_memory_bytes(16 << 20);
    for meta in builtins() {
        let safe = match meta.capability_path {
            Some(path) => SAFE_FAMILIES.iter().any(|family| path.starts_with(family)),
            None => false,
        };
        if safe {
            builder = builder.register_builtin_name(meta.name);
        }
    }
    let mut vm = builder.build();
    vm.set_output_sink(Arc::new(DropOutput));
    let _ = vm.run();
}

// ── Encoding ─────────────────────────────────────────────────────────
//
// Little-endian, length-prefixed. Spans are not encoded.
//
//   program  = u16 main_n_locals, chunk main,
//              u16 count, function*, u16 count, effect*, u16 count, string*
//   function = u8 has_name, [string name], u8 arity, u8 n_captures,
//              u16 n_locals, chunk
//   effect   = string name, u16 tag
//   chunk    = u32 len, code bytes, u16 count, constant*
//   constant = u8 kind (0 int, 1 float, 2 word, 3 string, 4 char), payload
//   string   = u32 len, UTF-8 bytes

pub fn encode(program: &CompiledProgram) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&program.main_n_locals.to_le_bytes());
    encode_chunk(&mut out, &program.main);
    put_len16(&mut out, program.functions.len());
    for proto in program.functions.iter() {
        match &proto.name {
            Some(name) => {
                out.push(1);
                put_str(&mut out, name);
            }
            None => out.push(0),
        }
        out.push(proto.arity);
        out.push(proto.n_captures);
        out.extend_from_slice(&proto.n_locals.to_le_bytes());
        encode_chunk(&mut out, &proto.chunk);
    }
    put_len16(&mut out, program.effects.len());
    for effect in program.effects.iter() {
        put_str(&mut out, &effect.name);
        out.extend_from_slice(&effect.tag.to_le_bytes());
    }
    put_len16(&mut out, program.globals.len());
    for name in program.globals.iter() {
        put_str(&mut out, name);
    }
    out
}

fn encode_chunk(out: &mut Vec<u8>, chunk: &Chunk) {
    out.extend_from_slice(&(chunk.code.len() as u32).to_le_bytes());
    out.extend_from_slice(&chunk.code);
    put_len16(out, chunk.constants.len());
    for constant in &chunk.constants {
        match constant {
            Constant::Int(n) => {
                out.push(0);
                out.extend_from_slice(&n.to_le_bytes());
            }
            Constant::Float(x) => {
                out.push(1);
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            Constant::Word(w) => {
                out.push(2);
                out.extend_from_slice(&w.to_le_bytes());
            }
            Constant::String(s) => {
                out.push(3);
                put_str(out, s);
            }
            Constant::Char(c) => {
                out.push(4);
                out.extend_from_slice(&(*c as u32).to_le_bytes());
            }
        }
    }
}

fn put_len16(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u16).to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// Decode a program; `None` if the bytes are not a well-formed encoding.
pub fn decode(data: &[u8]) -> Option<CompiledProgram> {
    let mut r = Reader { data, pos: 0 };
    let main_n_locals = r.u16()?;
    let main = r.chunk()?;
    let mut functions = Vec::new();
    for _ in 0..r.u16()? {
        let name = match r.u8()? {
            0 => None,
            _ => Some(r.string()?),
        };
        functions.push(FunctionProto {
            name,
            arity: r.u8()?,
            n_captures: r.u8()?,
            n_locals: r.u16()?,
            chunk: r.chunk()?,
        });
    }
    let mut effects = Vec::new();
    for _ in 0..r.u16()? {
        effects.push(EffectMeta {
            name: r.string()?,
            tag: r.u16()?,
        });
    }
    let mut globals = Vec::new();
    for _ in 0..r.u16()? {
        globals.push(Arc::<str>::from(r.string()?));
    }
    Some(CompiledProgram {
        main: Arc::new(main),
        main_n_locals,
        functions: functions.into(),
        effects: effects.into(),
        globals: globals.into(),
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }

    fn len(&mut self) -> Option<usize> {
        let len = self.u32()? as usize;
        (len <= MAX_LEN).then_some(len)
    }

    fn string(&mut self) -> Option<String> {
        let len = self.len()?;
        String::from_utf8(self.bytes(len)?.to_vec()).ok()
    }

    fn chunk(&mut self) -> Option<Chunk> {
        let len = self.len()?;
        let mut chunk = Chunk::default();
        chunk.code = self.bytes(len)?.to_vec();
        for _ in 0..self.u16()? {
            let constant = match self.u8()? {
                0 => Constant::Int(self.u64()? as i64),
                1 => Constant::Float(f64::from_bits(self.u64()?)),
                2 => Constant::Word(self.u64()?),
                3 => Constant::String(self.string()?),
                4 => Constant::Char(char::from_u32(self.u32()?)?),
                _ => return None,
            };
            chunk.constants.push(constant);
        }
        Some(chunk)
    }
}
