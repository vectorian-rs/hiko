use smallvec::SmallVec;
use std::fmt;
use std::mem::size_of;
use std::sync::Arc;

/// Inline storage for up to 2 Values (32 bytes) without heap allocation.
/// Covers cons cells (2 fields), pairs, and nullary/unary constructors.
/// Larger tuples spill to heap transparently.
pub type Fields = SmallVec<[Value; 2]>;

/// Index into the GC heap. Copy, 4 bytes, no Drop.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GcRef(pub(crate) u32);

/// Runtime value. Copy, no reference counting, no Drop.
/// Heap-allocated objects are referenced via GcRef indices.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Int(i64),
    Word(u64),
    Pid(u64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    Heap(GcRef),
    Builtin(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostHandleId(pub u64);

/// Runtime kind tag for an opaque host resource handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostHandleKind {
    #[cfg(feature = "builtin-aws-config")]
    AwsConfig,
    #[cfg(feature = "builtin-aws-s3")]
    AwsS3Client,
    #[cfg(feature = "builtin-aws-sqs")]
    AwsSqsClient,
    #[doc(hidden)]
    Unsupported,
}

#[cfg(feature = "builtin-aws-config")]
pub struct AwsConfigHandle {
    pub auth: AwsConfigAuthMethod,
    pub sdk_config: Arc<aws_config::SdkConfig>,
}

#[cfg(feature = "builtin-aws-config")]
impl fmt::Debug for AwsConfigHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsConfigHandle")
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "builtin-aws-config")]
#[derive(Debug)]
pub enum AwsConfigAuthMethod {
    SsoProfile { profile: String },
    InstanceProfile,
}

#[cfg(feature = "builtin-aws-s3")]
pub struct AwsS3ClientHandle {
    pub client: Arc<aws_sdk_s3::Client>,
}

#[cfg(feature = "builtin-aws-s3")]
impl fmt::Debug for AwsS3ClientHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsS3ClientHandle").finish_non_exhaustive()
    }
}

#[cfg(feature = "builtin-aws-sqs")]
pub struct AwsSqsClientHandle {
    pub client: Arc<aws_sdk_sqs::Client>,
}

#[cfg(feature = "builtin-aws-sqs")]
impl fmt::Debug for AwsSqsClientHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsSqsClientHandle").finish_non_exhaustive()
    }
}

/// VM-owned opaque native resource. Heap values store only a small typed handle;
/// the native Rust object is kept in the owning heap's host-resource table.
#[derive(Debug)]
pub enum HostResource {
    #[cfg(feature = "builtin-aws-config")]
    AwsConfig(AwsConfigHandle),
    #[cfg(feature = "builtin-aws-s3")]
    AwsS3Client(AwsS3ClientHandle),
    #[cfg(feature = "builtin-aws-sqs")]
    AwsSqsClient(AwsSqsClientHandle),
    #[doc(hidden)]
    Unsupported,
}

impl HostResource {
    pub fn kind(&self) -> HostHandleKind {
        match self {
            #[cfg(feature = "builtin-aws-config")]
            HostResource::AwsConfig(_) => HostHandleKind::AwsConfig,
            #[cfg(feature = "builtin-aws-s3")]
            HostResource::AwsS3Client(_) => HostHandleKind::AwsS3Client,
            #[cfg(feature = "builtin-aws-sqs")]
            HostResource::AwsSqsClient(_) => HostHandleKind::AwsSqsClient,
            HostResource::Unsupported => HostHandleKind::Unsupported,
        }
    }
}

/// Heap-allocated objects managed by the GC.
#[derive(Debug)]
pub enum HeapObject {
    String(String),
    Tuple(Fields),
    Data {
        tag: u16,
        fields: Fields,
    },
    Closure {
        proto_idx: usize,
        captures: Arc<[Value]>,
    },
    Bytes(Vec<u8>),
    /// Opaque native host resource. The actual Rust object lives in the VM heap's host-resource table.
    HostHandle {
        kind: HostHandleKind,
        id: HostHandleId,
    },
    /// Opaque RNG state (PCG-XSH-RR-64/32).
    Rng {
        state: u64,
        inc: u64,
    },
    Continuation(Box<ContinuationData>),
}

#[derive(Debug)]
pub struct ContinuationData {
    pub saved_frames: Vec<SavedFrame>,
    pub saved_stack: Vec<Value>,
    /// Handler removed by Perform, for auto-reinstallation by Resume.
    pub saved_handler: Option<SavedHandler>,
}

#[derive(Clone, Debug)]
pub struct SavedHandler {
    pub clauses: Vec<(u16, usize)>,
    pub proto_idx: usize,
    pub captures: Arc<[Value]>,
    pub locals_offset: usize,        // stack_base - handler_frame.base
    pub handler_count_before: usize, // handler list length before removal
}

#[derive(Clone, Debug)]
pub struct SavedFrame {
    pub proto_idx: usize,
    pub ip: usize,
    pub base_offset: usize,
    pub captures: Arc<[Value]>,
}

impl HeapObject {
    /// Approximate bytes retained by this heap object.
    ///
    /// This is a first-pass accounting model for VM memory limits. It counts
    /// the object storage itself plus directly owned dynamic buffers, but it
    /// deliberately does not attempt to de-duplicate shared `Arc` payloads.
    pub fn estimated_bytes(&self) -> usize {
        fn spilled_value_bytes(fields: &Fields) -> usize {
            if fields.spilled() {
                fields.capacity() * size_of::<Value>()
            } else {
                0
            }
        }

        let base = size_of::<HeapObject>();
        match self {
            HeapObject::HostHandle { .. } => {
                base + size_of::<HostHandleKind>() + size_of::<HostHandleId>()
            }
            HeapObject::String(s) => base + s.capacity(),
            HeapObject::Tuple(fields) => base + spilled_value_bytes(fields),
            HeapObject::Data { fields, .. } => base + spilled_value_bytes(fields),
            HeapObject::Closure { .. } => base,
            HeapObject::Bytes(bytes) => base + bytes.capacity(),
            HeapObject::Rng { .. } => base,
            HeapObject::Continuation(data) => {
                let handler_clause_bytes = data
                    .saved_handler
                    .as_ref()
                    .map(|handler| handler.clauses.capacity() * size_of::<(u16, usize)>())
                    .unwrap_or(0);
                base + size_of::<ContinuationData>()
                    + data.saved_frames.capacity() * size_of::<SavedFrame>()
                    + data.saved_stack.capacity() * size_of::<Value>()
                    + handler_clause_bytes
            }
        }
    }

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
            HeapObject::HostHandle { .. } => {}
            HeapObject::String(_) | HeapObject::Bytes(_) | HeapObject::Rng { .. } => {}
            HeapObject::Tuple(elems) => visit(elems, &mut f),
            HeapObject::Data { fields, .. } => visit(fields, &mut f),
            HeapObject::Closure { captures, .. } => visit(captures, &mut f),
            HeapObject::Continuation(data) => {
                visit(&data.saved_stack, &mut f);
                for frame in &data.saved_frames {
                    visit(&frame.captures, &mut f);
                }
            }
        }
    }
}

/// Builtin function entry, stored in a VM-level table.
pub struct BuiltinEntry {
    pub name: Arc<str>,
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
            Value::Word(w) => write!(f, "0w{w}"),
            Value::Pid(pid) => write!(f, "<pid {pid}>"),
            Value::Float(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Char(c) => write!(f, "{c}"),
            Value::Unit => write!(f, "()"),
            Value::Heap(_) => write!(f, "<heap>"),
            Value::Builtin(id) => write!(f, "<builtin:{id}>"),
        }
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn continuation_boxing_reduces_heap_object_size() {
        const OLD_INLINE_CONTINUATION_HEAP_OBJECT_SIZE: usize = 112;
        assert!(std::mem::size_of::<HeapObject>() < OLD_INLINE_CONTINUATION_HEAP_OBJECT_SIZE);
        assert_eq!(std::mem::size_of::<Value>(), 16);
        assert_eq!(std::mem::size_of::<SavedFrame>(), 40);
        assert_eq!(std::mem::size_of::<SavedHandler>(), 64);
        assert_eq!(std::mem::size_of::<Fields>(), 48);
    }
}
