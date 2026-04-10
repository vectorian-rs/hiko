use std::fmt;

/// Index into the GC heap. Copy, 4 bytes, no Drop.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GcRef(pub u32);

/// Runtime value. Copy — no reference counting, no Drop.
/// Heap-allocated objects are referenced via GcRef indices.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    Heap(GcRef),
    Builtin(u16),
}

/// Heap-allocated objects managed by the GC.
pub enum HeapObject {
    String(String),
    Tuple(Vec<Value>),
    Data {
        tag: u16,
        fields: Vec<Value>,
    },
    Closure {
        proto_idx: usize,
        captures: Vec<Value>,
    },
}

impl HeapObject {
    /// Iterate over all GcRefs directly contained in this object.
    pub fn gc_refs(&self) -> impl Iterator<Item = GcRef> + '_ {
        let values: &[Value] = match self {
            HeapObject::String(_) => &[],
            HeapObject::Tuple(elems) => elems,
            HeapObject::Data { fields, .. } => fields,
            HeapObject::Closure { captures, .. } => captures,
        };
        values.iter().filter_map(|v| match v {
            Value::Heap(r) => Some(*r),
            _ => None,
        })
    }
}

/// Builtin function entry — stored in a VM-level table.
pub struct BuiltinEntry {
    pub name: &'static str,
    pub func: BuiltinFn,
}

pub type BuiltinFn = fn(&[Value], &mut crate::heap::Heap) -> Result<Value, String>;

impl Value {
    pub fn is_heap(&self) -> bool {
        matches!(self, Value::Heap(_))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Char(c) => write!(f, "{c}"),
            Value::Unit => write!(f, "()"),
            Value::Heap(_) => write!(f, "<heap>"),
            Value::Builtin(id) => write!(f, "<builtin:{id}>"),
        }
    }
}
