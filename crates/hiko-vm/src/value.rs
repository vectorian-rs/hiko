use std::fmt;
use std::rc::Rc;

/// Index into the GC heap. Copy, 4 bytes, no Drop.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GcRef(pub u32);

/// Runtime value. Copy, no reference counting, no Drop.
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
        captures: Rc<[Value]>,
    },
    Continuation {
        saved_frames: Vec<SavedFrame>,
        saved_stack: Vec<Value>,
    },
}

#[derive(Clone)]
pub struct SavedFrame {
    pub proto_idx: usize,
    pub ip: usize,
    pub base_offset: usize,
    pub captures: Rc<[Value]>,
}

impl HeapObject {
    /// Visit all GcRefs directly contained in this object.
    pub fn for_each_gc_ref(&self, mut f: impl FnMut(GcRef)) {
        let visit = |values: &[Value], f: &mut dyn FnMut(GcRef)| {
            for v in values {
                if let Value::Heap(r) = v {
                    f(*r);
                }
            }
        };
        match self {
            HeapObject::String(_) => {}
            HeapObject::Tuple(elems) => visit(elems, &mut f),
            HeapObject::Data { fields, .. } => visit(fields, &mut f),
            HeapObject::Closure { captures, .. } => visit(captures, &mut f),
            HeapObject::Continuation {
                saved_stack,
                saved_frames,
            } => {
                visit(saved_stack, &mut f);
                for frame in saved_frames {
                    visit(&frame.captures, &mut f);
                }
            }
        }
    }
}

/// Builtin function entry, stored in a VM-level table.
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
