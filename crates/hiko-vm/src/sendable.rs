//! SendableValue: the process boundary type.
//!
//! Only SendableValue may cross process boundaries.
//! It contains no GcRef, no closures, no continuations.

use std::sync::Arc;

use crate::heap::{Heap, HeapLimitExceeded};
use crate::value::{GcRef, HeapObject, Value};
use crate::vm::{TAG_CONS, TAG_NIL};

/// A value that can safely cross process boundaries via message passing.
/// Contains no process-local heap references.
#[derive(Debug, Clone)]
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
    Tuple(Vec<SendableValue>),
    List(Vec<SendableValue>),
    Data {
        tag: u16,
        fields: Vec<SendableValue>,
    },
}

impl SendableValue {
    /// Approximate payload bytes represented by this value.
    pub fn estimated_bytes(&self) -> usize {
        match self {
            SendableValue::Int(_)
            | SendableValue::Word(_)
            | SendableValue::Pid(_)
            | SendableValue::Float(_) => 8,
            SendableValue::Bool(_) => 1,
            SendableValue::Char(_) => 4,
            SendableValue::Unit => 0,
            SendableValue::String(text) => text.len(),
            SendableValue::Bytes(bytes) => bytes.len(),
            SendableValue::Tuple(fields) | SendableValue::List(fields) => {
                fields.iter().map(Self::estimated_bytes).sum()
            }
            SendableValue::Data { fields, .. } => fields.iter().map(Self::estimated_bytes).sum(),
        }
    }
}

/// Serialize a VM Value into a SendableValue.
/// Returns Err if the value contains non-sendable types
/// (closures, continuations, Rng).
pub fn serialize(value: Value, heap: &Heap) -> Result<SendableValue, String> {
    match value {
        Value::Int(n) => Ok(SendableValue::Int(n)),
        Value::Word(w) => Ok(SendableValue::Word(w)),
        Value::Pid(pid) => Ok(SendableValue::Pid(pid)),
        Value::Float(f) => Ok(SendableValue::Float(f)),
        Value::Bool(b) => Ok(SendableValue::Bool(b)),
        Value::Char(c) => Ok(SendableValue::Char(c)),
        Value::Unit => Ok(SendableValue::Unit),
        Value::Builtin(_) => Err("cannot send builtin functions across processes".into()),
        Value::Heap(r) => serialize_heap(r, heap),
    }
}

fn serialize_heap(r: GcRef, heap: &Heap) -> Result<SendableValue, String> {
    match heap.get(r).map_err(|e| e.to_string())? {
        HeapObject::String(s) => Ok(SendableValue::String(Arc::from(s.as_str()))),
        HeapObject::Bytes(b) => Ok(SendableValue::Bytes(Arc::from(b.as_slice()))),
        HeapObject::Tuple(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for &v in fields.iter() {
                out.push(serialize(v, heap)?);
            }
            Ok(SendableValue::Tuple(out))
        }
        HeapObject::Data { tag, fields } => {
            // Special case: lists → serialize as SendableValue::List
            if *tag == TAG_NIL && fields.is_empty() {
                return Ok(SendableValue::List(Vec::new()));
            }
            if *tag == TAG_CONS && fields.len() == 2 {
                return serialize_list(fields[0], fields[1], heap);
            }
            let mut out = Vec::with_capacity(fields.len());
            for &v in fields.iter() {
                out.push(serialize(v, heap)?);
            }
            Ok(SendableValue::Data {
                tag: *tag,
                fields: out,
            })
        }
        HeapObject::Closure { .. } => Err("cannot send closures across processes".into()),
        HeapObject::Continuation { .. } => Err("cannot send continuations across processes".into()),
        HeapObject::Rng { .. } => Err("cannot send Rng state across processes".into()),
    }
}

/// Serialize a cons-list into a flat Vec for efficient transfer.
fn serialize_list(head: Value, tail: Value, heap: &Heap) -> Result<SendableValue, String> {
    let mut items = vec![serialize(head, heap)?];
    let mut cur = tail;
    loop {
        match cur {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, fields } if *tag == TAG_NIL && fields.is_empty() => {
                    break;
                }
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    items.push(serialize(fields[0], heap)?);
                    cur = fields[1];
                }
                _ => return Err("malformed list during serialization".into()),
            },
            _ => return Err("malformed list during serialization".into()),
        }
    }
    Ok(SendableValue::List(items))
}

/// Deserialize a SendableValue into a VM Value, allocating into the given heap.
pub fn deserialize(msg: SendableValue, heap: &mut Heap) -> Result<Value, HeapLimitExceeded> {
    use smallvec::smallvec;

    match msg {
        SendableValue::Int(n) => Ok(Value::Int(n)),
        SendableValue::Word(w) => Ok(Value::Word(w)),
        SendableValue::Pid(pid) => Ok(Value::Pid(pid)),
        SendableValue::Float(f) => Ok(Value::Float(f)),
        SendableValue::Bool(b) => Ok(Value::Bool(b)),
        SendableValue::Char(c) => Ok(Value::Char(c)),
        SendableValue::Unit => Ok(Value::Unit),
        SendableValue::String(s) => Ok(Value::Heap(heap.alloc(HeapObject::String(s.to_string()))?)),
        SendableValue::Bytes(b) => Ok(Value::Heap(heap.alloc(HeapObject::Bytes(b.to_vec()))?)),
        SendableValue::Tuple(fields) => {
            let mut values = smallvec::SmallVec::<[Value; 2]>::with_capacity(fields.len());
            for v in fields {
                values.push(deserialize(v, heap)?);
            }
            Ok(Value::Heap(heap.alloc(HeapObject::Tuple(values))?))
        }
        SendableValue::List(items) => {
            // Build cons-list in reverse
            let mut list = Value::Heap(heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })?);
            for item in items.into_iter().rev() {
                let val = deserialize(item, heap)?;
                list = Value::Heap(heap.alloc(HeapObject::Data {
                    tag: TAG_CONS,
                    fields: smallvec![val, list],
                })?);
            }
            Ok(list)
        }
        SendableValue::Data { tag, fields } => {
            let mut values = smallvec::SmallVec::<[Value; 2]>::with_capacity(fields.len());
            for v in fields {
                values.push(deserialize(v, heap)?);
            }
            Ok(Value::Heap(heap.alloc(HeapObject::Data {
                tag,
                fields: values,
            })?))
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
    fn test_closure_rejected() {
        let mut heap = Heap::new();
        let closure = Value::Heap(
            heap.alloc(HeapObject::Closure {
                proto_idx: 0,
                captures: Arc::from(vec![].into_boxed_slice()),
            })
            .unwrap(),
        );
        let result = serialize(closure, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("closure"));
    }

    #[test]
    fn test_continuation_rejected() {
        let mut heap = Heap::new();
        let cont = Value::Heap(
            heap.alloc(HeapObject::Continuation {
                saved_frames: vec![],
                saved_stack: vec![],
                saved_handler: None,
            })
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

    #[test]
    fn test_builtin_rejected() {
        let heap = Heap::new();
        let result = serialize(Value::Builtin(0), &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("builtin"));
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
    fn test_tuple_with_closure_rejected() {
        let mut heap = Heap::new();
        let closure = Value::Heap(
            heap.alloc(HeapObject::Closure {
                proto_idx: 0,
                captures: Arc::from(vec![].into_boxed_slice()),
            })
            .unwrap(),
        );
        let t = Value::Heap(
            heap.alloc(HeapObject::Tuple(smallvec![Value::Int(1), closure]))
                .unwrap(),
        );
        let result = serialize(t, &heap);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("closure"));
    }
}
