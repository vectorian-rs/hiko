//! SendableValue: the process boundary type.
//!
//! Only SendableValue may cross process boundaries.
//! It contains no GcRef and no continuations. A closure is sent as its
//! function index and its captures, which is enough because every process
//! runs the same compiled program.
//!
//! Every walk over a value here (serialize, deserialize, size estimate, clone
//! and drop) uses an explicit work stack rather than recursion, so a deeply
//! nested value cannot overflow the Rust stack. Serialization also counts the
//! size of what it produces, so a small heap value with heavy sharing (a DAG
//! that expands into an exponentially large tree) fails fast with an error.

use std::collections::HashMap;
use std::sync::Arc;

use crate::heap::{Heap, HeapLimitExceeded};
#[cfg(feature = "builtin-aws-config")]
use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostResource};
use crate::value::{Fields, GcRef, HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL};

pub const SENDABLE_BOUNDARY_BASE_HOST_WORK: u64 = 10;
const SENDABLE_HOST_WORK_PER_KIB: u64 = 1;

/// Default for `max_message_bytes`: the largest serialized value one spawn
/// capture, child result or similar transfer may produce.
pub const DEFAULT_MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Deepest nesting a serialized value may have. Lists count as one level,
/// however long they are.
pub const MAX_MESSAGE_DEPTH: usize = 100_000;

/// Bytes counted against `max_message_bytes` for each node, on top of the
/// payload of strings and bytes. This is the in-memory size of one node.
const MESSAGE_NODE_BYTES: u64 = std::mem::size_of::<SendableValue>() as u64;

pub fn host_work_for_sendable(value: &SendableValue) -> u64 {
    (value.estimated_bytes() as u64)
        .div_ceil(1024)
        .saturating_mul(SENDABLE_HOST_WORK_PER_KIB)
}

/// A value that can safely cross process boundaries via message passing.
/// Contains no process-local heap references.
#[derive(Debug)]
pub enum SendableValue {
    Int(i64),
    Word(u64),
    Pid(u64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    #[cfg(feature = "builtin-aws-config")]
    AwsConfigSsoProfile {
        profile: Arc<str>,
        sdk_config: Arc<aws_config::SdkConfig>,
    },
    #[cfg(feature = "builtin-aws-config")]
    AwsConfigInstanceProfile {
        sdk_config: Arc<aws_config::SdkConfig>,
    },
    Tuple(Vec<SendableValue>),
    List(Vec<SendableValue>),
    Data {
        tag: u16,
        fields: Vec<SendableValue>,
    },
    Closure {
        proto_idx: usize,
        captures: Vec<SendableValue>,
    },
    /// Index into the builtin table, which is the same in every process.
    Builtin(u16),
}

/// How to rebuild a container from its already-converted children.
enum Shape {
    Tuple,
    List,
    Data(u16),
    Closure(usize),
}

impl SendableValue {
    fn children(&self) -> &[SendableValue] {
        match self {
            SendableValue::Tuple(fields)
            | SendableValue::List(fields)
            | SendableValue::Data { fields, .. }
            | SendableValue::Closure {
                captures: fields, ..
            } => fields,
            _ => &[],
        }
    }

    fn children_mut(&mut self) -> Option<&mut Vec<SendableValue>> {
        match self {
            SendableValue::Tuple(fields)
            | SendableValue::List(fields)
            | SendableValue::Data { fields, .. }
            | SendableValue::Closure {
                captures: fields, ..
            } => Some(fields),
            _ => None,
        }
    }

    fn container(shape: Shape, fields: Vec<SendableValue>) -> Self {
        match shape {
            Shape::Tuple => SendableValue::Tuple(fields),
            Shape::List => SendableValue::List(fields),
            Shape::Data(tag) => SendableValue::Data { tag, fields },
            Shape::Closure(proto_idx) => SendableValue::Closure {
                proto_idx,
                captures: fields,
            },
        }
    }

    /// Payload bytes of this node alone, not counting children.
    fn own_bytes(&self) -> usize {
        match self {
            SendableValue::Int(_)
            | SendableValue::Word(_)
            | SendableValue::Pid(_)
            | SendableValue::Float(_) => 8,
            SendableValue::Bool(_) => 1,
            SendableValue::Builtin(_) => 2,
            SendableValue::Char(_) => 4,
            SendableValue::Unit => 0,
            SendableValue::String(text) => text.len(),
            SendableValue::Bytes(bytes) => bytes.len(),
            #[cfg(feature = "builtin-aws-config")]
            SendableValue::AwsConfigSsoProfile { profile, .. } => profile.len(),
            #[cfg(feature = "builtin-aws-config")]
            SendableValue::AwsConfigInstanceProfile { .. } => 0,
            SendableValue::Tuple(_)
            | SendableValue::List(_)
            | SendableValue::Data { .. }
            | SendableValue::Closure { .. } => 0,
        }
    }

    /// Approximate payload bytes represented by this value.
    pub fn estimated_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut stack = vec![self];
        while let Some(value) = stack.pop() {
            total = total.saturating_add(value.own_bytes());
            stack.extend(value.children());
        }
        total
    }

    /// Copy a leaf. Strings, bytes and host configs share their `Arc`.
    fn clone_leaf(&self) -> Self {
        match self {
            SendableValue::Int(n) => SendableValue::Int(*n),
            SendableValue::Word(w) => SendableValue::Word(*w),
            SendableValue::Pid(pid) => SendableValue::Pid(*pid),
            SendableValue::Float(f) => SendableValue::Float(*f),
            SendableValue::Bool(b) => SendableValue::Bool(*b),
            SendableValue::Char(c) => SendableValue::Char(*c),
            SendableValue::Unit => SendableValue::Unit,
            SendableValue::Builtin(idx) => SendableValue::Builtin(*idx),
            SendableValue::String(s) => SendableValue::String(s.clone()),
            SendableValue::Bytes(b) => SendableValue::Bytes(b.clone()),
            #[cfg(feature = "builtin-aws-config")]
            SendableValue::AwsConfigSsoProfile {
                profile,
                sdk_config,
            } => SendableValue::AwsConfigSsoProfile {
                profile: profile.clone(),
                sdk_config: sdk_config.clone(),
            },
            #[cfg(feature = "builtin-aws-config")]
            SendableValue::AwsConfigInstanceProfile { sdk_config } => {
                SendableValue::AwsConfigInstanceProfile {
                    sdk_config: sdk_config.clone(),
                }
            }
            SendableValue::Tuple(_)
            | SendableValue::List(_)
            | SendableValue::Data { .. }
            | SendableValue::Closure { .. } => {
                unreachable!("clone_leaf called on a container")
            }
        }
    }

    fn shape(&self) -> Option<Shape> {
        match self {
            SendableValue::Tuple(_) => Some(Shape::Tuple),
            SendableValue::List(_) => Some(Shape::List),
            SendableValue::Data { tag, .. } => Some(Shape::Data(*tag)),
            SendableValue::Closure { proto_idx, .. } => Some(Shape::Closure(*proto_idx)),
            _ => None,
        }
    }
}

/// Pop the last `count` values off `out`, in order.
fn take_last<T>(out: &mut Vec<T>, count: usize) -> Vec<T> {
    out.split_off(out.len() - count)
}

impl Clone for SendableValue {
    fn clone(&self) -> Self {
        enum Task<'a> {
            Visit(&'a SendableValue),
            Build(Shape, usize),
        }
        let mut tasks = vec![Task::Visit(self)];
        let mut out = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(value) => match value.shape() {
                    Some(shape) => {
                        let children = value.children();
                        tasks.push(Task::Build(shape, children.len()));
                        tasks.extend(children.iter().rev().map(Task::Visit));
                    }
                    None => out.push(value.clone_leaf()),
                },
                Task::Build(shape, count) => {
                    let fields = take_last(&mut out, count);
                    out.push(SendableValue::container(shape, fields));
                }
            }
        }
        out.pop().expect("clone produces one value")
    }
}

impl Drop for SendableValue {
    fn drop(&mut self) {
        // Move nested children onto a heap-allocated stack so that dropping a
        // deep value does not recurse once per level.
        let Some(fields) = self.children_mut() else {
            return;
        };
        if fields.iter().all(|field| field.children().is_empty()) {
            return;
        }
        let mut stack = std::mem::take(fields);
        while let Some(mut value) = stack.pop() {
            if let Some(fields) = value.children_mut() {
                stack.append(fields);
            }
        }
    }
}

/// Bytes counted against `max_message_bytes` for a scalar leaf.
fn scalar_message_bytes(value: Value) -> u64 {
    let payload = match value {
        Value::Int(_) | Value::Word(_) | Value::Pid(_) | Value::Float(_) => 8,
        Value::Bool(_) => 1,
        Value::Builtin(_) => 2,
        Value::Char(_) => 4,
        Value::Unit | Value::Heap(_) => 0,
    };
    MESSAGE_NODE_BYTES + payload
}

/// The heap children of an object as `serialize` will visit them: a list is
/// one node whose children are its elements.
fn message_children(r: GcRef, heap: &Heap) -> Result<Vec<Value>, String> {
    match heap.get(r).map_err(|e| e.to_string())? {
        HeapObject::String(_) | HeapObject::Bytes(_) => Ok(Vec::new()),
        HeapObject::Tuple(fields) => Ok(fields.to_vec()),
        HeapObject::Data { tag, fields } if is_list(*tag, fields) => list_items(r, heap),
        HeapObject::Data { fields, .. } => Ok(fields.to_vec()),
        HeapObject::Closure { captures, .. } => Ok(captures.to_vec()),
        HeapObject::Continuation(_) => Err("cannot send continuations across processes".into()),
        HeapObject::Rng { .. } => Err("cannot send Rng state across processes".into()),
        HeapObject::HostHandle { .. } => {
            Err("cannot send host resource handles across processes".into())
        }
    }
}

/// Check, before building anything, that serializing `value` stays within
/// `max_bytes` and [`MAX_MESSAGE_DEPTH`].
///
/// Each distinct heap object is measured once and memoized, so this takes
/// time proportional to the heap objects reachable from `value`, even when
/// sharing makes the serialized tree exponentially larger.
fn check_message_size(value: Value, heap: &Heap, max_bytes: u64) -> Result<(), String> {
    let Value::Heap(root) = value else {
        return Ok(());
    };
    // Serialized size and depth of each measured object.
    let mut measured: HashMap<GcRef, (u64, usize)> = HashMap::new();
    // (object, its children once they have been pushed for measuring)
    let mut stack: Vec<(GcRef, Option<Vec<Value>>)> = vec![(root, None)];
    while let Some((r, pushed)) = stack.pop() {
        if measured.contains_key(&r) {
            continue;
        }
        let Some(children) = pushed else {
            let children = message_children(r, heap)?;
            let unmeasured: Vec<_> = children
                .iter()
                .filter_map(|child| match child {
                    Value::Heap(c) if !measured.contains_key(c) => Some((*c, None)),
                    _ => None,
                })
                .collect();
            stack.push((r, Some(children)));
            stack.extend(unmeasured);
            continue;
        };
        let payload = match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.len() as u64,
            HeapObject::Bytes(b) => b.len() as u64,
            _ => 0,
        };
        let mut bytes = MESSAGE_NODE_BYTES.saturating_add(payload);
        let mut depth = 0;
        for child in children {
            let (child_bytes, child_depth) = match child {
                Value::Heap(c) => measured[&c],
                scalar => (scalar_message_bytes(scalar), 0),
            };
            bytes = bytes.saturating_add(child_bytes);
            depth = depth.max(child_depth + 1);
        }
        if bytes > max_bytes {
            return Err(format!(
                "value exceeds max_message_bytes ({max_bytes} bytes)"
            ));
        }
        if depth > MAX_MESSAGE_DEPTH {
            return Err(format!(
                "value is nested deeper than {MAX_MESSAGE_DEPTH} levels"
            ));
        }
        measured.insert(r, (bytes, depth));
    }
    Ok(())
}

/// Serialize a VM Value into a SendableValue.
/// Returns Err if the value contains non-sendable types
/// (continuations, Rng, provider handles), is larger than the
/// heap's `max_message_bytes`, or is nested deeper than [`MAX_MESSAGE_DEPTH`].
pub fn serialize(value: Value, heap: &Heap) -> Result<SendableValue, String> {
    enum Task {
        Visit(Value),
        Build(Shape, usize),
    }
    check_message_size(value, heap, heap.max_message_bytes())?;
    let mut tasks = vec![Task::Visit(value)];
    let mut out = Vec::new();
    while let Some(task) = tasks.pop() {
        let r = match task {
            Task::Build(shape, count) => {
                let fields = take_last(&mut out, count);
                out.push(SendableValue::container(shape, fields));
                continue;
            }
            Task::Visit(Value::Heap(r)) => r,
            Task::Visit(scalar) => {
                out.push(serialize_scalar(scalar)?);
                continue;
            }
        };
        match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => out.push(SendableValue::String(Arc::from(s.as_str()))),
            HeapObject::Bytes(b) => out.push(SendableValue::Bytes(Arc::from(b.as_slice()))),
            HeapObject::Data { tag, fields } if is_list(*tag, fields) => {
                let items = list_items(r, heap)?;
                tasks.push(Task::Build(Shape::List, items.len()));
                tasks.extend(items.into_iter().rev().map(Task::Visit));
            }
            HeapObject::Tuple(fields) => {
                tasks.push(Task::Build(Shape::Tuple, fields.len()));
                tasks.extend(fields.iter().rev().copied().map(Task::Visit));
            }
            HeapObject::Data { tag, fields } => {
                tasks.push(Task::Build(Shape::Data(*tag), fields.len()));
                tasks.extend(fields.iter().rev().copied().map(Task::Visit));
            }
            HeapObject::Closure {
                proto_idx,
                captures,
            } => {
                tasks.push(Task::Build(Shape::Closure(*proto_idx), captures.len()));
                tasks.extend(captures.iter().rev().copied().map(Task::Visit));
            }
            _ => return Err(message_children(r, heap).unwrap_err()),
        }
    }
    Ok(out.pop().expect("serialize produces one value"))
}

fn serialize_scalar(value: Value) -> Result<SendableValue, String> {
    match value {
        Value::Int(n) => Ok(SendableValue::Int(n)),
        Value::Word(w) => Ok(SendableValue::Word(w)),
        Value::Pid(pid) => Ok(SendableValue::Pid(pid)),
        Value::Float(f) => Ok(SendableValue::Float(f)),
        Value::Bool(b) => Ok(SendableValue::Bool(b)),
        Value::Char(c) => Ok(SendableValue::Char(c)),
        Value::Unit => Ok(SendableValue::Unit),
        Value::Builtin(idx) => Ok(SendableValue::Builtin(idx)),
        Value::Heap(_) => unreachable!("serialize_scalar called on a heap value"),
    }
}

fn is_list(tag: u16, fields: &Fields) -> bool {
    (tag == TAG_NIL && fields.is_empty()) || (tag == TAG_CONS && fields.len() == 2)
}

/// The elements of a cons-list, which is sent as one flat `List` node.
fn list_items(head: GcRef, heap: &Heap) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    let mut cur = Value::Heap(head);
    loop {
        match cur {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, fields } if *tag == TAG_NIL && fields.is_empty() => {
                    return Ok(items);
                }
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    items.push(fields[0]);
                    cur = fields[1];
                }
                _ => return Err("malformed list during serialization".into()),
            },
            _ => return Err("malformed list during serialization".into()),
        }
    }
}

/// Deserialize a SendableValue into a VM Value, allocating into the given heap.
pub fn deserialize(msg: SendableValue, heap: &mut Heap) -> Result<Value, HeapLimitExceeded> {
    enum Task {
        Visit(SendableValue),
        Build(Shape, usize),
    }
    let mut tasks = vec![Task::Visit(msg)];
    let mut out: Vec<Value> = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Visit(mut value) => match value.shape() {
                Some(shape) => {
                    let children = value.children_mut().map(std::mem::take).unwrap_or_default();
                    tasks.push(Task::Build(shape, children.len()));
                    tasks.extend(children.into_iter().rev().map(Task::Visit));
                }
                None => out.push(deserialize_leaf(&value, heap)?),
            },
            Task::Build(Shape::List, count) => {
                let items = take_last(&mut out, count);
                let mut list = Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_NIL,
                    fields: Fields::new(),
                })?);
                for item in items.into_iter().rev() {
                    list = Value::Heap(heap.alloc(HeapObject::Data {
                        tag: TAG_CONS,
                        fields: Fields::from_slice(&[item, list]),
                    })?);
                }
                out.push(list);
            }
            Task::Build(Shape::Tuple, count) => {
                let fields = Fields::from_vec(take_last(&mut out, count));
                out.push(Value::Heap(heap.alloc(HeapObject::Tuple(fields))?));
            }
            Task::Build(Shape::Data(tag), count) => {
                let fields = Fields::from_vec(take_last(&mut out, count));
                out.push(Value::Heap(heap.alloc(HeapObject::Data { tag, fields })?));
            }
            Task::Build(Shape::Closure(proto_idx), count) => {
                let captures = take_last(&mut out, count);
                out.push(Value::Heap(heap.alloc(HeapObject::Closure {
                    proto_idx,
                    captures: captures.into(),
                })?));
            }
        }
    }
    Ok(out.pop().expect("deserialize produces one value"))
}

fn deserialize_leaf(value: &SendableValue, heap: &mut Heap) -> Result<Value, HeapLimitExceeded> {
    match value {
        SendableValue::Int(n) => Ok(Value::Int(*n)),
        SendableValue::Word(w) => Ok(Value::Word(*w)),
        SendableValue::Pid(pid) => Ok(Value::Pid(*pid)),
        SendableValue::Float(f) => Ok(Value::Float(*f)),
        SendableValue::Bool(b) => Ok(Value::Bool(*b)),
        SendableValue::Char(c) => Ok(Value::Char(*c)),
        SendableValue::Unit => Ok(Value::Unit),
        SendableValue::Builtin(idx) => Ok(Value::Builtin(*idx)),
        SendableValue::String(s) => Ok(Value::Heap(heap.alloc(HeapObject::String(s.to_string()))?)),
        SendableValue::Bytes(b) => Ok(Value::Heap(heap.alloc(HeapObject::Bytes(b.to_vec()))?)),
        #[cfg(feature = "builtin-aws-config")]
        SendableValue::AwsConfigSsoProfile {
            profile,
            sdk_config,
        } => heap.alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
            auth: AwsConfigAuthMethod::SsoProfile {
                profile: profile.to_string(),
            },
            sdk_config: sdk_config.clone(),
        })),
        #[cfg(feature = "builtin-aws-config")]
        SendableValue::AwsConfigInstanceProfile { sdk_config } => {
            heap.alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: sdk_config.clone(),
            }))
        }
        SendableValue::Tuple(_)
        | SendableValue::List(_)
        | SendableValue::Data { .. }
        | SendableValue::Closure { .. } => {
            unreachable!("deserialize_leaf called on a container")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::Heap;
    use crate::value::HeapObject;
    use smallvec::smallvec;
    use std::sync::Arc;

    fn round_trip(value: Value, heap: &Heap) -> Value {
        let sendable = serialize(value, heap).expect("serialize failed");
        let mut new_heap = Heap::new();
        deserialize(sendable, &mut new_heap).unwrap()
    }

    #[test]
    fn sendable_host_work_scales_with_payload_bytes() {
        let small = SendableValue::Bytes(Arc::from([0_u8; 1]));
        let large = SendableValue::Bytes(Arc::from([0_u8; 2048]));
        assert!(host_work_for_sendable(&large) > host_work_for_sendable(&small));
    }

    #[test]
    fn test_int() {
        let heap = Heap::new();
        match round_trip(Value::Int(42), &heap) {
            Value::Int(42) => {}
            other => panic!("expected Int(42), got {:?}", other),
        }
    }

    #[test]
    fn test_float() {
        let heap = Heap::new();
        let value = 314.0_f64 / 100.0;
        match round_trip(Value::Float(value), &heap) {
            Value::Float(f) => assert!((f - value).abs() < 1e-10),
            other => panic!("expected Float, got {:?}", other),
        }
    }

    #[test]
    fn test_pid() {
        let heap = Heap::new();
        match round_trip(Value::Pid(42), &heap) {
            Value::Pid(42) => {}
            other => panic!("expected Pid(42), got {:?}", other),
        }
    }

    #[test]
    fn test_bool() {
        let heap = Heap::new();
        match round_trip(Value::Bool(true), &heap) {
            Value::Bool(true) => {}
            other => panic!("expected Bool(true), got {:?}", other),
        }
    }

    #[test]
    fn test_char() {
        let heap = Heap::new();
        match round_trip(Value::Char('X'), &heap) {
            Value::Char('X') => {}
            other => panic!("expected Char('X'), got {:?}", other),
        }
    }

    #[test]
    fn test_unit() {
        let heap = Heap::new();
        match round_trip(Value::Unit, &heap) {
            Value::Unit => {}
            other => panic!("expected Unit, got {:?}", other),
        }
    }

    #[test]
    fn test_string() {
        let mut heap = Heap::new();
        let s = Value::Heap(
            heap.alloc(HeapObject::String("hello world".to_string()))
                .unwrap(),
        );
        let sendable = serialize(s, &heap).unwrap();
        let mut new_heap = Heap::new();
        let result = deserialize(sendable, &mut new_heap).unwrap();
        match result {
            Value::Heap(r) => match new_heap.get(r) {
                Ok(HeapObject::String(s)) => assert_eq!(s, "hello world"),
                other => panic!("expected String, got {:?}", other),
            },
            other => panic!("expected Heap, got {:?}", other),
        }
    }

    #[test]
    fn test_bytes() {
        let mut heap = Heap::new();
        let b = Value::Heap(heap.alloc(HeapObject::Bytes(vec![1, 2, 3])).unwrap());
        let sendable = serialize(b, &heap).unwrap();
        let mut new_heap = Heap::new();
        let result = deserialize(sendable, &mut new_heap).unwrap();
        match result {
            Value::Heap(r) => match new_heap.get(r) {
                Ok(HeapObject::Bytes(b)) => assert_eq!(b, &[1, 2, 3]),
                other => panic!("expected Bytes, got {:?}", other),
            },
            other => panic!("expected Heap, got {:?}", other),
        }
    }

    #[test]
    fn test_tuple() {
        let mut heap = Heap::new();
        let t = Value::Heap(
            heap.alloc(HeapObject::Tuple(smallvec![
                Value::Int(1),
                Value::Bool(true)
            ]))
            .unwrap(),
        );
        let sendable = serialize(t, &heap).unwrap();
        let mut new_heap = Heap::new();
        let result = deserialize(sendable, &mut new_heap).unwrap();
        match result {
            Value::Heap(r) => match new_heap.get(r) {
                Ok(HeapObject::Tuple(fields)) => {
                    assert_eq!(fields.len(), 2);
                    assert!(matches!(fields[0], Value::Int(1)));
                    assert!(matches!(fields[1], Value::Bool(true)));
                }
                other => panic!("expected Tuple, got {:?}", other),
            },
            other => panic!("expected Heap, got {:?}", other),
        }
    }

    #[test]
    fn test_list() {
        let mut heap = Heap::new();
        // Build [1, 2, 3]
        let nil = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })
            .unwrap(),
        );
        let c3 = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![Value::Int(3), nil],
            })
            .unwrap(),
        );
        let c2 = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![Value::Int(2), c3],
            })
            .unwrap(),
        );
        let c1 = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![Value::Int(1), c2],
            })
            .unwrap(),
        );

        let sendable = serialize(c1, &heap).unwrap();

        // Verify it serialized as a flat list
        match &sendable {
            SendableValue::List(items) => {
                assert_eq!(items.len(), 3);
            }
            other => panic!("expected List, got {:?}", other),
        }

        // Deserialize into new heap and verify
        let mut new_heap = Heap::new();
        let result = deserialize(sendable, &mut new_heap).unwrap();
        // Walk the cons-list
        let mut cur = result;
        let mut values = vec![];
        loop {
            match cur {
                Value::Heap(r) => match new_heap.get(r).unwrap() {
                    HeapObject::Data { tag, .. } if *tag == TAG_NIL => break,
                    HeapObject::Data { tag, fields } if *tag == TAG_CONS => {
                        if let Value::Int(n) = fields[0] {
                            values.push(n);
                        }
                        cur = fields[1];
                    }
                    _ => panic!("bad list"),
                },
                _ => panic!("bad list"),
            }
        }
        assert_eq!(values, vec![1, 2, 3]);
    }

    #[test]
    fn test_data() {
        let mut heap = Heap::new();
        // Some(42) — tag 1, one field
        let data = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: 1,
                fields: smallvec![Value::Int(42)],
            })
            .unwrap(),
        );
        let sendable = serialize(data, &heap).unwrap();
        let mut new_heap = Heap::new();
        let result = deserialize(sendable, &mut new_heap).unwrap();
        match result {
            Value::Heap(r) => match new_heap.get(r) {
                Ok(HeapObject::Data { tag, fields }) => {
                    assert_eq!(*tag, 1);
                    assert_eq!(fields.len(), 1);
                    assert!(matches!(fields[0], Value::Int(42)));
                }
                other => panic!("expected Data, got {:?}", other),
            },
            other => panic!("expected Heap, got {:?}", other),
        }
    }

    #[test]
    fn test_nested_tuple_with_string() {
        let mut heap = Heap::new();
        let s = Value::Heap(heap.alloc(HeapObject::String("hello".to_string())).unwrap());
        let t = Value::Heap(
            heap.alloc(HeapObject::Tuple(smallvec![Value::Int(1), s]))
                .unwrap(),
        );
        let sendable = serialize(t, &heap).unwrap();

        // Verify Arc<str> in serialized form
        match &sendable {
            SendableValue::Tuple(fields) => {
                assert!(matches!(&fields[1], SendableValue::String(s) if &**s == "hello"));
            }
            _ => panic!("expected Tuple"),
        }
    }

    #[test]
    fn test_closure_round_trips_with_captures() {
        let mut heap = Heap::new();
        let s = Value::Heap(heap.alloc(HeapObject::String("cap".to_string())).unwrap());
        let closure = Value::Heap(
            heap.alloc(HeapObject::Closure {
                proto_idx: 3,
                captures: Arc::from(vec![Value::Int(7), s].into_boxed_slice()),
            })
            .unwrap(),
        );
        let mut new_heap = Heap::new();
        let back = deserialize(serialize(closure, &heap).unwrap(), &mut new_heap).unwrap();
        let Value::Heap(r) = back else {
            panic!("expected heap closure, got {back:?}");
        };
        match new_heap.get(r).unwrap() {
            HeapObject::Closure {
                proto_idx,
                captures,
            } => {
                assert_eq!(*proto_idx, 3);
                assert!(matches!(captures[0], Value::Int(7)));
                let Value::Heap(sr) = captures[1] else {
                    panic!("expected string capture");
                };
                assert!(matches!(new_heap.get(sr).unwrap(), HeapObject::String(t) if t == "cap"));
            }
            _ => panic!("expected Closure"),
        }
    }

    #[test]
    fn test_continuation_rejected() {
        let mut heap = Heap::new();
        let cont = Value::Heap(
            heap.alloc(HeapObject::Continuation(Box::new(
                crate::value::ContinuationData {
                    saved_frames: vec![],
                    saved_stack: vec![],
                    saved_handler: None,
                },
            )))
            .unwrap(),
        );
        let result = serialize(cont, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("continuation"));
    }

    #[test]
    fn test_rng_rejected() {
        let mut heap = Heap::new();
        let rng = Value::Heap(heap.alloc(HeapObject::Rng { state: 0, inc: 1 }).unwrap());
        let result = serialize(rng, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Rng"));
    }

    #[cfg(feature = "builtin-aws-config")]
    #[test]
    fn test_host_handle_rejected() {
        use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostResource};

        let mut heap = Heap::new();
        let handle = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }))
            .unwrap();
        let result = serialize(handle, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("host resource"));
    }

    #[test]
    fn test_builtin() {
        let heap = Heap::new();
        assert!(matches!(
            round_trip(Value::Builtin(5), &heap),
            Value::Builtin(5)
        ));
    }

    #[test]
    fn test_empty_list() {
        let mut heap = Heap::new();
        let nil = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })
            .unwrap(),
        );
        let sendable = serialize(nil, &heap).unwrap();
        match &sendable {
            SendableValue::List(items) => assert!(items.is_empty()),
            other => panic!("expected empty List, got {:?}", other),
        }
    }

    #[test]
    fn test_list_with_strings() {
        let mut heap = Heap::new();
        let s1 = Value::Heap(heap.alloc(HeapObject::String("a".to_string())).unwrap());
        let s2 = Value::Heap(heap.alloc(HeapObject::String("b".to_string())).unwrap());
        let nil = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })
            .unwrap(),
        );
        let c2 = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![s2, nil],
            })
            .unwrap(),
        );
        let c1 = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_CONS,
                fields: smallvec![s1, c2],
            })
            .unwrap(),
        );

        let sendable = serialize(c1, &heap).unwrap();
        match &sendable {
            SendableValue::List(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(&items[0], SendableValue::String(s) if &**s == "a"));
                assert!(matches!(&items[1], SendableValue::String(s) if &**s == "b"));
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_closure_capturing_rng_rejected() {
        let mut heap = Heap::new();
        let rng = Value::Heap(heap.alloc(HeapObject::Rng { state: 0, inc: 1 }).unwrap());
        let closure = Value::Heap(
            heap.alloc(HeapObject::Closure {
                proto_idx: 0,
                captures: Arc::from(vec![rng].into_boxed_slice()),
            })
            .unwrap(),
        );
        let t = Value::Heap(
            heap.alloc(HeapObject::Tuple(smallvec![Value::Int(1), closure]))
                .unwrap(),
        );
        let result = serialize(t, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Rng"));
    }

    /// Run `f` on a thread with a small stack, so that a walk that recurses
    /// once per level fails the test instead of passing on a large main stack.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    fn nested_data(heap: &mut Heap, depth: usize) -> Value {
        let mut value = Value::Int(0);
        for _ in 0..depth {
            value = Value::Heap(
                heap.alloc(HeapObject::Data {
                    tag: 1,
                    fields: smallvec![value],
                })
                .unwrap(),
            );
        }
        value
    }

    #[test]
    fn test_shared_dag_fails_fast_with_size_error() {
        // x30 is 31 heap objects that expand into a tree of 2^31 nodes.
        let mut heap = Heap::new();
        let mut x = Value::Int(0);
        for _ in 0..30 {
            x = Value::Heap(heap.alloc(HeapObject::Tuple(smallvec![x, x])).unwrap());
        }
        let started = std::time::Instant::now();
        let err = serialize(x, &heap).unwrap_err();
        assert!(err.contains("max_message_bytes"), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn test_million_element_list_round_trips() {
        on_small_stack(|| {
            let mut heap = Heap::new();
            let mut list = Value::Heap(
                heap.alloc(HeapObject::Data {
                    tag: TAG_NIL,
                    fields: smallvec![],
                })
                .unwrap(),
            );
            for i in 0..1_000_000 {
                list = Value::Heap(
                    heap.alloc(HeapObject::Data {
                        tag: TAG_CONS,
                        fields: smallvec![Value::Int(i), list],
                    })
                    .unwrap(),
                );
            }
            let sendable = serialize(list, &heap).unwrap();
            assert!(matches!(&sendable, SendableValue::List(items) if items.len() == 1_000_000));
            let mut new_heap = Heap::new();
            deserialize(sendable, &mut new_heap).unwrap();
            assert_eq!(new_heap.live_count(), 1_000_001);
        });
    }

    #[test]
    fn test_deeply_nested_value_round_trips_without_recursion() {
        on_small_stack(|| {
            let mut heap = Heap::new();
            let value = nested_data(&mut heap, MAX_MESSAGE_DEPTH);
            let sendable = serialize(value, &heap).unwrap();
            assert_eq!(sendable.estimated_bytes(), 8);
            let copy = sendable.clone();
            let mut new_heap = Heap::new();
            deserialize(sendable, &mut new_heap).unwrap();
            assert_eq!(new_heap.live_count(), MAX_MESSAGE_DEPTH);
            drop(copy);
        });
    }

    #[test]
    fn test_value_nested_too_deep_is_rejected() {
        on_small_stack(|| {
            let mut heap = Heap::new();
            let value = nested_data(&mut heap, MAX_MESSAGE_DEPTH + 1);
            let err = serialize(value, &heap).unwrap_err();
            assert!(err.contains("nested deeper than"), "{err}");
        });
    }

    #[test]
    fn test_strings_and_bytes_are_shared_not_copied() {
        let text: Arc<str> = Arc::from("x".repeat(1 << 20));
        let bytes: Arc<[u8]> = Arc::from(vec![7u8; 1 << 20]);
        let value = SendableValue::Tuple(vec![
            SendableValue::String(text.clone()),
            SendableValue::List(vec![SendableValue::Bytes(bytes.clone())]),
        ]);
        let copy = value.clone();
        match &copy {
            SendableValue::Tuple(fields) => {
                assert!(matches!(&fields[0], SendableValue::String(s) if Arc::ptr_eq(s, &text)));
                assert!(matches!(
                    &fields[1],
                    SendableValue::List(items)
                        if matches!(&items[0], SendableValue::Bytes(b) if Arc::ptr_eq(b, &bytes))
                ));
            }
            other => panic!("expected Tuple, got {other:?}"),
        }
    }
}
