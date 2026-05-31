//! Runtime shape verification for builtin type signatures.
//!
//! Each builtin has a hand-written type signature string (in
//! `hiko-builtin-meta::signatures`). This module parses the return-type portion
//! of that string into a lightweight `Shape` AST, then checks whether a
//! `SendableValue` matches that shape. The goal is to catch mismatches between
//! the declared type and what the Rust code actually produces — the class of
//! bug documented in GitHub issue #76.

use crate::heap::Heap;
use crate::sendable::SendableValue;
use crate::value::{HeapObject, HostHandleKind, Value};
use crate::vm::{TAG_CONS, TAG_NIL};

/// Lightweight type shape used for runtime verification.
/// Covers only the types that appear in builtin return signatures.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Int,
    Float,
    Bool,
    String,
    Char,
    Unit,
    Bytes,
    Word,
    Pid,
    Rng,
    AwsConfig,
    AwsS3Client,
    AwsSqsClient,
    /// Type variable — matches anything (polymorphic builtins like json_parse).
    Var,
    /// Tuple with N fields of specific shapes.
    Tuple(Vec<Shape>),
    /// List with element shape.
    List(Box<Shape>),
    /// Option.option wrapping a shape.
    Option(Box<Shape>),
}

/// A parsed return type: the part of the signature after the last `->`.
#[derive(Debug)]
pub struct ReturnType {
    pub shape: Shape,
}

/// Parse the return type from a full signature string.
/// Example: "string -> int" returns Shape::Int.
/// Example: "aws_s3_client -> bool * (string * string * string) list * string" returns a tuple shape.
pub fn parse_return_type(sig: &str) -> Result<ReturnType, String> {
    // Find the last -> to isolate the return type
    let return_str = if let Some(idx) = sig.rfind("->") {
        sig[idx + 2..].trim()
    } else {
        sig.trim()
    };
    let mut parser = ShapeParser::new(return_str);
    let shape = parser.parse()?;
    Ok(ReturnType { shape })
}

struct ShapeParser {
    chars: Vec<char>,
    pos: usize,
}

impl ShapeParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse(&mut self) -> Result<Shape, String> {
        let ty = self.parse_tuple_or_star()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(format!(
                "unexpected trailing input at position {}: '{}'",
                self.pos,
                &self.chars[self.pos..].iter().collect::<String>()
            ));
        }
        Ok(ty)
    }

    fn parse_tuple_or_star(&mut self) -> Result<Shape, String> {
        let first = self.parse_app()?;
        let mut elems = vec![first];
        loop {
            self.skip_ws();
            if self.peek_char() == Some('*') && !self.peek_word("list") {
                self.advance();
                elems.push(self.parse_app()?);
            } else {
                break;
            }
        }
        if elems.len() == 1 {
            Ok(elems.pop().unwrap())
        } else {
            Ok(Shape::Tuple(elems))
        }
    }

    fn parse_app(&mut self) -> Result<Shape, String> {
        let mut ty = self.parse_atom()?;
        loop {
            self.skip_ws();
            if self.consume_word("list") {
                ty = Shape::List(Box::new(ty));
            } else if self.consume_word("Option.option") {
                ty = Shape::Option(Box::new(ty));
            } else {
                break;
            }
        }
        Ok(ty)
    }

    fn parse_atom(&mut self) -> Result<Shape, String> {
        self.skip_ws();
        if self.peek_char() == Some('(') {
            self.advance();
            let ty = self.parse_tuple_or_star()?;
            self.skip_ws();
            if self.peek_char() != Some(')') {
                return Err(format!("expected ')' at position {}", self.pos));
            }
            self.advance();
            return Ok(ty);
        }
        if self.peek_char() == Some('\'') {
            self.advance();
            let _name = self.read_ident()?;
            // Type variables match anything at runtime
            return Ok(Shape::Var);
        }
        let ident = self.read_ident()?;
        match ident.as_str() {
            "int" => Ok(Shape::Int),
            "float" => Ok(Shape::Float),
            "bool" => Ok(Shape::Bool),
            "string" => Ok(Shape::String),
            "char" => Ok(Shape::Char),
            "unit" => Ok(Shape::Unit),
            "bytes" => Ok(Shape::Bytes),
            "word" => Ok(Shape::Word),
            "pid" => Ok(Shape::Pid),
            "rng" => Ok(Shape::Rng),
            "aws_config" => Ok(Shape::AwsConfig),
            "aws_s3_client" => Ok(Shape::AwsS3Client),
            "aws_sqs_client" => Ok(Shape::AwsSqsClient),
            other => Err(format!("unknown type '{}'", other)),
        }
    }

    fn read_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric()
                || self.chars[self.pos] == '_'
                || self.chars[self.pos] == '.')
        {
            self.pos += 1;
        }
        if self.pos == start {
            Err(format!("expected identifier at position {}", self.pos))
        } else {
            Ok(self.chars[start..self.pos].iter().collect())
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek_char(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            Some(self.chars[self.pos])
        } else {
            None
        }
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    fn peek_word(&mut self, word: &str) -> bool {
        let saved = self.pos;
        let result = self.consume_word(word);
        self.pos = saved;
        result
    }

    fn consume_word(&mut self, expected: &str) -> bool {
        self.skip_ws();
        let start = self.pos;
        let expected_chars: Vec<char> = expected.chars().collect();
        if !self.chars[self.pos..].starts_with(&expected_chars) {
            return false;
        }
        self.pos += expected_chars.len();
        if self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '_')
        {
            self.pos = start;
            false
        } else {
            true
        }
    }
}

/// Check if a `SendableValue` matches a `Shape`.
/// Returns `Ok(())` if it matches, `Err(description)` if not.
pub fn check_shape(value: &SendableValue, shape: &Shape) -> Result<(), String> {
    match shape {
        Shape::Var => Ok(()), // type variables match anything
        Shape::Int => match value {
            SendableValue::Int(_) => Ok(()),
            _ => Err(format!("expected int, got {}", value_kind(value))),
        },
        Shape::Float => match value {
            SendableValue::Float(_) => Ok(()),
            _ => Err(format!("expected float, got {}", value_kind(value))),
        },
        Shape::Bool => match value {
            SendableValue::Bool(_) => Ok(()),
            _ => Err(format!("expected bool, got {}", value_kind(value))),
        },
        Shape::String => match value {
            SendableValue::String(_) => Ok(()),
            _ => Err(format!("expected string, got {}", value_kind(value))),
        },
        Shape::Char => match value {
            SendableValue::Char(_) => Ok(()),
            _ => Err(format!("expected char, got {}", value_kind(value))),
        },
        Shape::Unit => match value {
            SendableValue::Unit => Ok(()),
            _ => Err(format!("expected unit, got {}", value_kind(value))),
        },
        Shape::Bytes => match value {
            SendableValue::Bytes(_) => Ok(()),
            _ => Err(format!("expected bytes, got {}", value_kind(value))),
        },
        Shape::Word => match value {
            SendableValue::Word(_) => Ok(()),
            _ => Err(format!("expected word, got {}", value_kind(value))),
        },
        Shape::Pid => match value {
            SendableValue::Pid(_) => Ok(()),
            _ => Err(format!("expected pid, got {}", value_kind(value))),
        },
        Shape::Rng => Err("rng cannot cross process boundary".into()),
        Shape::AwsConfig => {
            #[cfg(feature = "builtin-aws-config")]
            match value {
                SendableValue::AwsConfigSsoProfile { .. }
                | SendableValue::AwsConfigInstanceProfile { .. } => Ok(()),
                _ => Err(format!("expected aws_config, got {}", value_kind(value))),
            }
            #[cfg(not(feature = "builtin-aws-config"))]
            {
                let _ = value;
                Err("aws_config not available in this build".into())
            }
        }
        Shape::AwsS3Client => Err("aws_s3_client cannot cross process boundary".into()),
        Shape::AwsSqsClient => Err("aws_sqs_client cannot cross process boundary".into()),
        Shape::Tuple(expected_elems) => match value {
            SendableValue::Tuple(fields) => {
                if fields.len() != expected_elems.len() {
                    return Err(format!(
                        "expected tuple with {} fields, got {}",
                        expected_elems.len(),
                        fields.len()
                    ));
                }
                for (i, (field, elem_shape)) in fields.iter().zip(expected_elems.iter()).enumerate()
                {
                    check_shape(field, elem_shape)
                        .map_err(|e| format!("tuple field {}: {}", i, e))?;
                }
                Ok(())
            }
            _ => Err(format!(
                "expected tuple with {} fields, got {}",
                expected_elems.len(),
                value_kind(value)
            )),
        },
        Shape::List(elem_shape) => match value {
            SendableValue::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    check_shape(item, elem_shape).map_err(|e| format!("list[{}]: {}", i, e))?;
                }
                Ok(())
            }
            _ => Err(format!("expected list, got {}", value_kind(value))),
        },
        Shape::Option(inner_shape) => match value {
            SendableValue::Data { tag, fields } => {
                // Option None = tag 0, 0 fields; Option Some = tag 1, 1 field
                match *tag {
                    0 => Ok(()), // None
                    1 => {
                        if fields.len() != 1 {
                            return Err(format!(
                                "Option.Some expected 1 field, got {}",
                                fields.len()
                            ));
                        }
                        check_shape(&fields[0], inner_shape)
                            .map_err(|e| format!("Option.Some: {}", e))
                    }
                    _ => Err(format!("Option expected tag 0 or 1, got {}", tag)),
                }
            }
            _ => Err(format!("expected Option (Data), got {}", value_kind(value))),
        },
    }
}

/// Check if a VM `Value` matches a `Shape` in the context of its heap.
///
/// Unlike `check_shape`, this can validate process-local runtime values such as
/// RNG states and opaque host handles before they are serialized across process
/// boundaries.
pub fn check_value_shape(value: Value, heap: &Heap, shape: &Shape) -> Result<(), String> {
    match shape {
        Shape::Var => Ok(()),
        Shape::Int => match value {
            Value::Int(_) => Ok(()),
            _ => Err(format!("expected int, got {}", vm_value_kind(value, heap))),
        },
        Shape::Word => match value {
            Value::Word(_) => Ok(()),
            _ => Err(format!("expected word, got {}", vm_value_kind(value, heap))),
        },
        Shape::Pid => match value {
            Value::Pid(_) => Ok(()),
            _ => Err(format!("expected pid, got {}", vm_value_kind(value, heap))),
        },
        Shape::Float => match value {
            Value::Float(_) => Ok(()),
            _ => Err(format!(
                "expected float, got {}",
                vm_value_kind(value, heap)
            )),
        },
        Shape::Bool => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(format!("expected bool, got {}", vm_value_kind(value, heap))),
        },
        Shape::Char => match value {
            Value::Char(_) => Ok(()),
            _ => Err(format!("expected char, got {}", vm_value_kind(value, heap))),
        },
        Shape::Unit => match value {
            Value::Unit => Ok(()),
            _ => Err(format!("expected unit, got {}", vm_value_kind(value, heap))),
        },
        Shape::String => match value {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::String(_) => Ok(()),
                _ => Err(format!(
                    "expected string, got {}",
                    vm_value_kind(value, heap)
                )),
            },
            _ => Err(format!(
                "expected string, got {}",
                vm_value_kind(value, heap)
            )),
        },
        Shape::Bytes => match value {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Bytes(_) => Ok(()),
                _ => Err(format!(
                    "expected bytes, got {}",
                    vm_value_kind(value, heap)
                )),
            },
            _ => Err(format!(
                "expected bytes, got {}",
                vm_value_kind(value, heap)
            )),
        },
        Shape::Rng => match value {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Rng { .. } => Ok(()),
                _ => Err(format!("expected rng, got {}", vm_value_kind(value, heap))),
            },
            _ => Err(format!("expected rng, got {}", vm_value_kind(value, heap))),
        },
        Shape::AwsConfig => {
            #[cfg(feature = "builtin-aws-config")]
            {
                check_host_handle(value, heap, HostHandleKind::AwsConfig, "aws_config")
            }
            #[cfg(not(feature = "builtin-aws-config"))]
            {
                let _ = (value, heap);
                Err("aws_config not available in this build".into())
            }
        }
        Shape::AwsS3Client => {
            #[cfg(feature = "builtin-aws-s3")]
            {
                check_host_handle(value, heap, HostHandleKind::AwsS3Client, "aws_s3_client")
            }
            #[cfg(not(feature = "builtin-aws-s3"))]
            {
                let _ = (value, heap);
                Err("aws_s3_client not available in this build".into())
            }
        }
        Shape::AwsSqsClient => {
            #[cfg(feature = "builtin-aws-sqs")]
            {
                check_host_handle(value, heap, HostHandleKind::AwsSqsClient, "aws_sqs_client")
            }
            #[cfg(not(feature = "builtin-aws-sqs"))]
            {
                let _ = (value, heap);
                Err("aws_sqs_client not available in this build".into())
            }
        }
        Shape::Tuple(expected_elems) => match value {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Tuple(fields) => {
                    if fields.len() != expected_elems.len() {
                        return Err(format!(
                            "expected tuple with {} fields, got {}",
                            expected_elems.len(),
                            fields.len()
                        ));
                    }
                    for (i, (field, elem_shape)) in fields
                        .iter()
                        .copied()
                        .zip(expected_elems.iter())
                        .enumerate()
                    {
                        check_value_shape(field, heap, elem_shape)
                            .map_err(|e| format!("tuple field {}: {}", i, e))?;
                    }
                    Ok(())
                }
                _ => Err(format!(
                    "expected tuple with {} fields, got {}",
                    expected_elems.len(),
                    vm_value_kind(value, heap)
                )),
            },
            _ => Err(format!(
                "expected tuple with {} fields, got {}",
                expected_elems.len(),
                vm_value_kind(value, heap)
            )),
        },
        Shape::List(elem_shape) => check_value_list_shape(value, heap, elem_shape),
        Shape::Option(inner_shape) => match value {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag: 0, fields } if fields.is_empty() => Ok(()),
                HeapObject::Data { tag: 1, fields } if fields.len() == 1 => {
                    check_value_shape(fields[0], heap, inner_shape)
                        .map_err(|e| format!("Option.Some: {}", e))
                }
                HeapObject::Data { tag, .. } => {
                    Err(format!("Option expected tag 0 or 1, got {tag}"))
                }
                _ => Err(format!(
                    "expected Option, got {}",
                    vm_value_kind(value, heap)
                )),
            },
            _ => Err(format!(
                "expected Option, got {}",
                vm_value_kind(value, heap)
            )),
        },
    }
}

fn check_value_list_shape(value: Value, heap: &Heap, elem_shape: &Shape) -> Result<(), String> {
    let mut current = value;
    let mut index = 0usize;
    loop {
        match current {
            Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
                HeapObject::Data { tag, fields } if *tag == TAG_NIL && fields.is_empty() => {
                    return Ok(());
                }
                HeapObject::Data { tag, fields } if *tag == TAG_CONS && fields.len() == 2 => {
                    check_value_shape(fields[0], heap, elem_shape)
                        .map_err(|e| format!("list[{}]: {}", index, e))?;
                    current = fields[1];
                    index += 1;
                }
                _ => {
                    return Err(format!(
                        "expected list, got {}",
                        vm_value_kind(current, heap)
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "expected list, got {}",
                    vm_value_kind(current, heap)
                ));
            }
        }
    }
}

#[allow(dead_code)]
fn check_host_handle(
    value: Value,
    heap: &Heap,
    expected: HostHandleKind,
    expected_name: &str,
) -> Result<(), String> {
    match value {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::HostHandle { kind, .. } if *kind == expected => Ok(()),
            _ => Err(format!(
                "expected {expected_name}, got {}",
                vm_value_kind(value, heap)
            )),
        },
        _ => Err(format!(
            "expected {expected_name}, got {}",
            vm_value_kind(value, heap)
        )),
    }
}

fn vm_value_kind(value: Value, heap: &Heap) -> &'static str {
    match value {
        Value::Int(_) => "int",
        Value::Word(_) => "word",
        Value::Pid(_) => "pid",
        Value::Float(_) => "float",
        Value::Bool(_) => "bool",
        Value::Char(_) => "char",
        Value::Unit => "unit",
        Value::Builtin(_) => "builtin",
        Value::Heap(r) => match heap.get(r) {
            Ok(HeapObject::String(_)) => "string",
            Ok(HeapObject::Bytes(_)) => "bytes",
            Ok(HeapObject::Tuple(_)) => "tuple",
            Ok(HeapObject::Data { .. }) => "data",
            Ok(HeapObject::Closure { .. }) => "closure",
            Ok(HeapObject::Continuation(_)) => "continuation",
            Ok(HeapObject::Rng { .. }) => "rng",
            Ok(HeapObject::HostHandle { .. }) => "host-handle",
            Err(_) => "dangling-heap-ref",
        },
    }
}

fn value_kind(value: &SendableValue) -> &'static str {
    match value {
        SendableValue::Int(_) => "int",
        SendableValue::Word(_) => "word",
        SendableValue::Pid(_) => "pid",
        SendableValue::Float(_) => "float",
        SendableValue::Bool(_) => "bool",
        SendableValue::Char(_) => "char",
        SendableValue::Unit => "unit",
        SendableValue::String(_) => "string",
        SendableValue::Bytes(_) => "bytes",
        #[cfg(feature = "builtin-aws-config")]
        SendableValue::AwsConfigSsoProfile { .. } => "aws_config",
        #[cfg(feature = "builtin-aws-config")]
        SendableValue::AwsConfigInstanceProfile { .. } => "aws_config",
        SendableValue::Tuple(_) => "tuple",
        SendableValue::List(_) => "list",
        SendableValue::Data { .. } => "data",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::builtin_entries;
    use hiko_builtin_meta::{builtin_type_signature, builtin_type_signatures};
    use smallvec::smallvec;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    // --- Parser tests ---

    #[test]
    fn parse_int() {
        let rt = parse_return_type("string -> int").unwrap();
        assert_eq!(rt.shape, Shape::Int);
    }

    #[test]
    fn parse_string() {
        let rt = parse_return_type("string -> string").unwrap();
        assert_eq!(rt.shape, Shape::String);
    }

    #[test]
    fn parse_unit() {
        let rt = parse_return_type("string -> unit").unwrap();
        assert_eq!(rt.shape, Shape::Unit);
    }

    #[test]
    fn parse_bool() {
        let rt = parse_return_type("int -> bool").unwrap();
        assert_eq!(rt.shape, Shape::Bool);
    }

    #[test]
    fn parse_simple_tuple() {
        let rt = parse_return_type("string -> int * string").unwrap();
        assert_eq!(rt.shape, Shape::Tuple(vec![Shape::Int, Shape::String]));
    }

    #[test]
    fn parse_list() {
        let rt = parse_return_type("string -> string list").unwrap();
        assert_eq!(rt.shape, Shape::List(Box::new(Shape::String)));
    }

    #[test]
    fn parse_tuple_with_list() {
        let rt = parse_return_type("string -> int * (string * string) list * string").unwrap();
        assert_eq!(
            rt.shape,
            Shape::Tuple(vec![
                Shape::Int,
                Shape::List(Box::new(Shape::Tuple(vec![Shape::String, Shape::String]))),
                Shape::String,
            ])
        );
    }

    #[test]
    fn parse_aws_s3_signature() {
        let rt =
            parse_return_type("aws_s3_client -> bool * (string * string * string) list * string")
                .unwrap();
        assert_eq!(
            rt.shape,
            Shape::Tuple(vec![
                Shape::Bool,
                Shape::List(Box::new(Shape::Tuple(vec![
                    Shape::String,
                    Shape::String,
                    Shape::String,
                ]))),
                Shape::String,
            ])
        );
    }

    #[test]
    fn parse_option() {
        let rt = parse_return_type("unit -> string Option.option").unwrap();
        assert_eq!(rt.shape, Shape::Option(Box::new(Shape::String)));
    }

    #[test]
    fn parse_type_var() {
        let rt = parse_return_type("string -> 'a").unwrap();
        assert_eq!(rt.shape, Shape::Var);
    }

    #[test]
    fn parse_http_get_signature() {
        let rt = parse_return_type("string -> int * (string * string) list * string").unwrap();
        assert_eq!(
            rt.shape,
            Shape::Tuple(vec![
                Shape::Int,
                Shape::List(Box::new(Shape::Tuple(vec![Shape::String, Shape::String]))),
                Shape::String,
            ])
        );
    }

    #[test]
    fn parse_exec_signature() {
        let rt = parse_return_type("string * string list -> int * string * string").unwrap();
        assert_eq!(
            rt.shape,
            Shape::Tuple(vec![Shape::Int, Shape::String, Shape::String])
        );
    }

    #[test]
    fn parse_rng_signature() {
        let rt = parse_return_type("bytes -> rng").unwrap();
        assert_eq!(rt.shape, Shape::Rng);
    }

    #[test]
    fn parse_no_arrow() {
        // A signature with no -> is just the type itself (e.g., a value)
        let rt = parse_return_type("unit -> unit").unwrap();
        assert_eq!(rt.shape, Shape::Unit);
    }

    // --- Shape check tests ---

    #[test]
    fn check_int_matches() {
        assert!(check_shape(&SendableValue::Int(42), &Shape::Int).is_ok());
    }

    #[test]
    fn check_int_mismatch() {
        assert!(check_shape(&SendableValue::Bool(true), &Shape::Int).is_err());
    }

    #[test]
    fn check_string_matches() {
        assert!(check_shape(&SendableValue::String(Arc::from("hello")), &Shape::String).is_ok());
    }

    #[test]
    fn check_tuple_matches() {
        let value = SendableValue::Tuple(vec![SendableValue::Int(1), SendableValue::Bool(true)]);
        let shape = Shape::Tuple(vec![Shape::Int, Shape::Bool]);
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_tuple_wrong_arity() {
        let value = SendableValue::Tuple(vec![SendableValue::Int(1)]);
        let shape = Shape::Tuple(vec![Shape::Int, Shape::Bool]);
        assert!(check_shape(&value, &shape).is_err());
    }

    #[test]
    fn check_tuple_wrong_field_type() {
        let value = SendableValue::Tuple(vec![SendableValue::Int(1), SendableValue::Int(2)]);
        let shape = Shape::Tuple(vec![Shape::Int, Shape::Bool]);
        assert!(check_shape(&value, &shape).is_err());
    }

    #[test]
    fn check_list_matches() {
        let value = SendableValue::List(vec![
            SendableValue::String(Arc::from("a")),
            SendableValue::String(Arc::from("b")),
        ]);
        let shape = Shape::List(Box::new(Shape::String));
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_list_wrong_elem_type() {
        let value = SendableValue::List(vec![SendableValue::Int(1)]);
        let shape = Shape::List(Box::new(Shape::String));
        assert!(check_shape(&value, &shape).is_err());
    }

    #[test]
    fn check_option_some() {
        let value = SendableValue::Data {
            tag: 1,
            fields: vec![SendableValue::String(Arc::from("hello"))],
        };
        let shape = Shape::Option(Box::new(Shape::String));
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_option_none() {
        let value = SendableValue::Data {
            tag: 0,
            fields: vec![],
        };
        let shape = Shape::Option(Box::new(Shape::String));
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_var_matches_anything() {
        assert!(check_shape(&SendableValue::Int(42), &Shape::Var).is_ok());
        assert!(check_shape(&SendableValue::String(Arc::from("hi")), &Shape::Var).is_ok());
        assert!(check_shape(&SendableValue::Unit, &Shape::Var).is_ok());
    }

    #[test]
    fn check_nested_tuple_with_list() {
        // Simulates the aws_s3_list_buckets success result:
        // bool * (string * string * string) list * string
        let bucket = SendableValue::Tuple(vec![
            SendableValue::String(Arc::from("my-bucket")),
            SendableValue::String(Arc::from("2026-01-01")),
            SendableValue::String(Arc::from("arn:aws:s3:::my-bucket")),
        ]);
        let value = SendableValue::Tuple(vec![
            SendableValue::Bool(true),
            SendableValue::List(vec![bucket]),
            SendableValue::String(Arc::from("")),
        ]);
        let shape = Shape::Tuple(vec![
            Shape::Bool,
            Shape::List(Box::new(Shape::Tuple(vec![
                Shape::String,
                Shape::String,
                Shape::String,
            ]))),
            Shape::String,
        ]);
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_empty_list_matches() {
        let value = SendableValue::List(vec![]);
        let shape = Shape::List(Box::new(Shape::String));
        assert!(check_shape(&value, &shape).is_ok());
    }

    #[test]
    fn check_unit_matches() {
        assert!(check_shape(&SendableValue::Unit, &Shape::Unit).is_ok());
    }

    #[test]
    fn check_bool_matches() {
        assert!(check_shape(&SendableValue::Bool(false), &Shape::Bool).is_ok());
    }

    #[test]
    fn check_bytes_matches() {
        assert!(check_shape(&SendableValue::Bytes(Arc::from([1, 2, 3])), &Shape::Bytes).is_ok());
    }

    // --- Regression: the exact bug from issue #76 ---
    // The old signature declared Option.option wrappers that the Rust code
    // did not produce. This test verifies the current correct shape.

    #[test]
    fn aws_s3_bucket_shape_no_options() {
        // The current (fixed) return shape for aws_s3_list_buckets
        let shape =
            parse_return_type("aws_s3_client -> bool * (string * string * string) list * string")
                .unwrap()
                .shape;

        // Simulate what sendable_list_buckets_output actually produces
        let bucket = SendableValue::Tuple(vec![
            SendableValue::String(Arc::from("my-bucket")),
            SendableValue::String(Arc::from("2026-01-01")),
            SendableValue::String(Arc::from("arn:aws:s3:::my-bucket")),
        ]);
        let success = SendableValue::Tuple(vec![
            SendableValue::Bool(true),
            SendableValue::List(vec![bucket]),
            SendableValue::String(Arc::from("")),
        ]);

        assert!(check_shape(&success, &shape).is_ok());
    }

    #[test]
    fn aws_s3_old_wrong_shape_would_fail() {
        // The OLD (buggy) shape would have expected Option.option wrappers.
        // Verify that the actual (non-Option) values would NOT match the old shape.
        let old_shape = parse_return_type(
            "aws_config -> bool * ((string Option.option * string Option.option * string Option.option) list Option.option * ((string Option.option * string Option.option) Option.option) * string Option.option * string Option.option) * string",
        ).unwrap().shape;

        // The actual value (plain strings) should NOT match this old shape
        let bucket = SendableValue::Tuple(vec![
            SendableValue::String(Arc::from("my-bucket")),
            SendableValue::String(Arc::from("2026-01-01")),
            SendableValue::String(Arc::from("arn:aws:s3:::my-bucket")),
        ]);
        let success = SendableValue::Tuple(vec![
            SendableValue::Bool(true),
            SendableValue::List(vec![bucket]),
            SendableValue::String(Arc::from("")),
        ]);

        assert!(check_shape(&success, &old_shape).is_err());
    }

    // --- Parse and sample all real signatures ---

    #[test]
    fn parse_all_builtin_signatures_from_registry() {
        for (name, sig) in builtin_type_signatures() {
            let result = parse_return_type(sig.ty);
            assert!(
                result.is_ok(),
                "failed to parse signature for '{}': {:?}",
                name,
                result.err()
            );
        }
    }

    type ArgsBuilder = fn(&mut Heap) -> Vec<Value>;

    struct BuiltinShapeCase {
        name: &'static str,
        args: ArgsBuilder,
    }

    fn heap_string(heap: &mut Heap, text: &str) -> Value {
        Value::Heap(heap.alloc(HeapObject::String(text.to_string())).unwrap())
    }

    fn heap_bytes(heap: &mut Heap, bytes: &[u8]) -> Value {
        Value::Heap(heap.alloc(HeapObject::Bytes(bytes.to_vec())).unwrap())
    }

    fn heap_tuple(heap: &mut Heap, fields: Vec<Value>) -> Value {
        Value::Heap(
            heap.alloc(HeapObject::Tuple(fields.into_iter().collect()))
                .unwrap(),
        )
    }

    fn heap_list(heap: &mut Heap, values: Vec<Value>) -> Value {
        let mut current = Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: TAG_NIL,
                fields: smallvec![],
            })
            .unwrap(),
        );
        for value in values.into_iter().rev() {
            current = Value::Heap(
                heap.alloc(HeapObject::Data {
                    tag: TAG_CONS,
                    fields: smallvec![value, current],
                })
                .unwrap(),
            );
        }
        current
    }

    fn temp_path(name: &str) -> String {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("hiko-shape-{name}-{}-{unique}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    fn no_args(_: &mut Heap) -> Vec<Value> {
        vec![]
    }

    fn one_int(_: &mut Heap) -> Vec<Value> {
        vec![Value::Int(7)]
    }

    fn one_word(_: &mut Heap) -> Vec<Value> {
        vec![Value::Word(7)]
    }

    fn one_float(_: &mut Heap) -> Vec<Value> {
        vec![Value::Float(9.0)]
    }

    fn one_string(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, "42")]
    }

    fn one_char(_: &mut Heap) -> Vec<Value> {
        vec![Value::Char('A')]
    }

    fn one_bytes(heap: &mut Heap) -> Vec<Value> {
        vec![heap_bytes(heap, &[1, 2, 3, 4])]
    }

    fn int_pair(heap: &mut Heap) -> Vec<Value> {
        vec![heap_tuple(heap, vec![Value::Int(5), Value::Int(2)])]
    }

    fn word_pair(heap: &mut Heap) -> Vec<Value> {
        vec![heap_tuple(heap, vec![Value::Word(5), Value::Word(2)])]
    }

    fn float_pair(heap: &mut Heap) -> Vec<Value> {
        vec![heap_tuple(heap, vec![Value::Float(6.0), Value::Float(2.0)])]
    }

    fn string_pair(heap: &mut Heap) -> Vec<Value> {
        let a = heap_string(heap, "hello world");
        let b = heap_string(heap, "world");
        vec![heap_tuple(heap, vec![a, b])]
    }

    fn substring_args(heap: &mut Heap) -> Vec<Value> {
        let s = heap_string(heap, "hello");
        vec![heap_tuple(heap, vec![s, Value::Int(1), Value::Int(3)])]
    }

    fn string_replace_args(heap: &mut Heap) -> Vec<Value> {
        let s = heap_string(heap, "hello world");
        let from = heap_string(heap, "world");
        let to = heap_string(heap, "hiko");
        vec![heap_tuple(heap, vec![s, from, to])]
    }

    fn split_args(heap: &mut Heap) -> Vec<Value> {
        let s = heap_string(heap, "a,b");
        let sep = heap_string(heap, ",");
        vec![heap_tuple(heap, vec![s, sep])]
    }

    fn string_join_args(heap: &mut Heap) -> Vec<Value> {
        let a = heap_string(heap, "a");
        let b = heap_string(heap, "b");
        let list = heap_list(heap, vec![a, b]);
        let sep = heap_string(heap, ",");
        vec![heap_tuple(heap, vec![list, sep])]
    }

    fn path_join_args(heap: &mut Heap) -> Vec<Value> {
        let a = heap_string(heap, "/tmp");
        let b = heap_string(heap, "file.txt");
        vec![heap_tuple(heap, vec![a, b])]
    }

    fn read_file_arg(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("read-file.txt");
        std::fs::write(&path, "hello").unwrap();
        vec![heap_string(heap, &path)]
    }

    fn read_file_bytes_arg(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("read-file-bytes.bin");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        vec![heap_string(heap, &path)]
    }

    fn write_file_args(heap: &mut Heap) -> Vec<Value> {
        let path = heap_string(heap, &temp_path("write-file.txt"));
        let content = heap_string(heap, "hello");
        vec![heap_tuple(heap, vec![path, content])]
    }

    fn file_exists_arg(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("exists.txt");
        std::fs::write(&path, "hello").unwrap();
        vec![heap_string(heap, &path)]
    }

    fn list_dir_arg(heap: &mut Heap) -> Vec<Value> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = format!("target/hiko-shape-list-dir-{}-{unique}", std::process::id());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), "hello").unwrap();
        vec![heap_string(heap, &dir)]
    }

    fn remove_file_arg(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("remove-file.txt");
        std::fs::write(&path, "hello").unwrap();
        vec![heap_string(heap, &path)]
    }

    fn create_dir_arg(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, &temp_path("create-dir"))]
    }

    fn glob_arg(heap: &mut Heap) -> Vec<Value> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = format!("target/hiko-shape-glob-{}-{unique}", std::process::id());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), "hello").unwrap();
        vec![heap_string(heap, &format!("{dir}/*.txt"))]
    }

    fn read_file_tagged_args(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("tagged.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let path = heap_string(heap, &path);
        vec![heap_tuple(heap, vec![path, Value::Int(0), Value::Int(0)])]
    }

    fn edit_file_tagged_args(heap: &mut Heap) -> Vec<Value> {
        let path = temp_path("edit-tagged.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let path = heap_string(heap, &path);
        let edits = heap_string(heap, "");
        vec![heap_tuple(heap, vec![path, edits])]
    }

    fn bytes_get_args(heap: &mut Heap) -> Vec<Value> {
        let bytes = heap_bytes(heap, &[10, 20, 30]);
        vec![heap_tuple(heap, vec![bytes, Value::Int(1)])]
    }

    fn bytes_slice_args(heap: &mut Heap) -> Vec<Value> {
        let bytes = heap_bytes(heap, &[10, 20, 30]);
        vec![heap_tuple(heap, vec![bytes, Value::Int(0), Value::Int(2)])]
    }

    fn rng_pair_args(heap: &mut Heap) -> Vec<Value> {
        let rng = Value::Heap(heap.alloc(HeapObject::Rng { state: 1, inc: 1 }).unwrap());
        vec![heap_tuple(heap, vec![rng, Value::Int(4)])]
    }

    fn json_path_args(heap: &mut Heap) -> Vec<Value> {
        let json = heap_string(heap, r#"{"name":"hiko","xs":[1,2]}"#);
        let path = heap_string(heap, "name");
        vec![heap_tuple(heap, vec![json, path])]
    }

    fn json_string_arg(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, r#"{"name":"hiko","xs":[1,2]}"#)]
    }

    fn json_value_arg(heap: &mut Heap) -> Vec<Value> {
        vec![Value::Heap(
            heap.alloc(HeapObject::Data {
                tag: 2,
                fields: smallvec![Value::Int(42)],
            })
            .unwrap(),
        )]
    }

    fn regex_match_args(heap: &mut Heap) -> Vec<Value> {
        let s = heap_string(heap, "hello");
        let re = heap_string(heap, "^h");
        vec![heap_tuple(heap, vec![s, re])]
    }

    fn regex_replace_args(heap: &mut Heap) -> Vec<Value> {
        let s = heap_string(heap, "hello");
        let re = heap_string(heap, "l+");
        let repl = heap_string(heap, "L");
        vec![heap_tuple(heap, vec![s, re, repl])]
    }

    fn set_stdin_and_no_args(heap: &mut Heap) -> Vec<Value> {
        heap.set_stdin_override("stdin".to_string());
        vec![]
    }

    fn assert_args(heap: &mut Heap) -> Vec<Value> {
        let msg = heap_string(heap, "shape check");
        vec![heap_tuple(heap, vec![Value::Bool(true), msg])]
    }

    fn assert_eq_args(heap: &mut Heap) -> Vec<Value> {
        let msg = heap_string(heap, "shape check");
        vec![heap_tuple(heap, vec![Value::Int(1), Value::Int(1), msg])]
    }

    fn timezone_arg(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, "UTC")]
    }

    fn date_instant_tz_args(heap: &mut Heap) -> Vec<Value> {
        let tz = heap_string(heap, "UTC");
        vec![heap_tuple(heap, vec![Value::Int(0), tz])]
    }

    fn date_tz_pair_args(heap: &mut Heap) -> Vec<Value> {
        let instant = heap_string(heap, "1970-01-01T00:00:00Z[UTC]");
        let tz = heap_string(heap, "UTC");
        vec![heap_tuple(heap, vec![instant, tz])]
    }

    fn date_format_args(heap: &mut Heap) -> Vec<Value> {
        let fmt = heap_string(heap, "%Y");
        let instant = heap_string(heap, "1970-01-01T00:00:00Z[UTC]");
        vec![heap_tuple(heap, vec![fmt, instant])]
    }

    fn date_zoned_arg(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, "1970-01-01T00:00:00Z[UTC]")]
    }

    fn date_rfc3339_arg(heap: &mut Heap) -> Vec<Value> {
        vec![heap_string(heap, "1970-01-01T00:00:00Z")]
    }

    fn sample_cases() -> Vec<BuiltinShapeCase> {
        vec![
            BuiltinShapeCase {
                name: "read_stdin",
                args: set_stdin_and_no_args,
            },
            BuiltinShapeCase {
                name: "int_to_string",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "float_to_string",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "string_to_int",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "char_to_int",
                args: one_char,
            },
            BuiltinShapeCase {
                name: "int_to_char",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "int_to_float",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "word_to_int",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "int_to_word",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "word_to_string",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "string_to_word",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "numeric_int32_min_value",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "numeric_int32_max_value",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "numeric_int32_of_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_int32_checked_of_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_int32_to_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_int32_add",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_checked_add",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_wrapping_add",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_saturating_add",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_sub",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_mul",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_div",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_rem",
                args: int_pair,
            },
            BuiltinShapeCase {
                name: "numeric_int32_neg",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_word32_min_value",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "numeric_word32_max_value",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "numeric_word32_of_word",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "numeric_word32_checked_of_word",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "numeric_word32_of_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_word32_checked_of_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "numeric_word32_to_word",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "numeric_word32_to_int",
                args: one_word,
            },
            BuiltinShapeCase {
                name: "numeric_word32_add",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_checked_add",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_saturating_add",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_sub",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_mul",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_div",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_word32_rem",
                args: word_pair,
            },
            BuiltinShapeCase {
                name: "numeric_float32_of_float",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "numeric_float32_to_float",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "numeric_float32_neg",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "numeric_float32_add",
                args: float_pair,
            },
            BuiltinShapeCase {
                name: "numeric_float32_sub",
                args: float_pair,
            },
            BuiltinShapeCase {
                name: "numeric_float32_mul",
                args: float_pair,
            },
            BuiltinShapeCase {
                name: "numeric_float32_div",
                args: float_pair,
            },
            BuiltinShapeCase {
                name: "string_length",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "substring",
                args: substring_args,
            },
            BuiltinShapeCase {
                name: "string_contains",
                args: string_pair,
            },
            BuiltinShapeCase {
                name: "starts_with",
                args: string_pair,
            },
            BuiltinShapeCase {
                name: "ends_with",
                args: string_pair,
            },
            BuiltinShapeCase {
                name: "trim",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "to_upper",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "to_lower",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "split",
                args: split_args,
            },
            BuiltinShapeCase {
                name: "string_replace",
                args: string_replace_args,
            },
            BuiltinShapeCase {
                name: "string_join",
                args: string_join_args,
            },
            BuiltinShapeCase {
                name: "sqrt",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "abs_float",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "abs_int",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "floor",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "ceil",
                args: one_float,
            },
            BuiltinShapeCase {
                name: "read_file",
                args: read_file_arg,
            },
            BuiltinShapeCase {
                name: "read_file_bytes",
                args: read_file_bytes_arg,
            },
            BuiltinShapeCase {
                name: "write_file",
                args: write_file_args,
            },
            BuiltinShapeCase {
                name: "file_exists",
                args: file_exists_arg,
            },
            BuiltinShapeCase {
                name: "list_dir",
                args: list_dir_arg,
            },
            BuiltinShapeCase {
                name: "remove_file",
                args: remove_file_arg,
            },
            BuiltinShapeCase {
                name: "create_dir",
                args: create_dir_arg,
            },
            BuiltinShapeCase {
                name: "is_dir",
                args: list_dir_arg,
            },
            BuiltinShapeCase {
                name: "is_file",
                args: file_exists_arg,
            },
            BuiltinShapeCase {
                name: "read_file_tagged",
                args: read_file_tagged_args,
            },
            BuiltinShapeCase {
                name: "edit_file_tagged",
                args: edit_file_tagged_args,
            },
            BuiltinShapeCase {
                name: "glob",
                args: glob_arg,
            },
            BuiltinShapeCase {
                name: "walk_dir",
                args: list_dir_arg,
            },
            BuiltinShapeCase {
                name: "path_join",
                args: path_join_args,
            },
            BuiltinShapeCase {
                name: "bytes_length",
                args: one_bytes,
            },
            BuiltinShapeCase {
                name: "bytes_to_string",
                args: one_bytes,
            },
            BuiltinShapeCase {
                name: "string_to_bytes",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "bytes_get",
                args: bytes_get_args,
            },
            BuiltinShapeCase {
                name: "bytes_slice",
                args: bytes_slice_args,
            },
            BuiltinShapeCase {
                name: "blake3",
                args: one_bytes,
            },
            BuiltinShapeCase {
                name: "random_bytes",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "rng_seed",
                args: one_bytes,
            },
            BuiltinShapeCase {
                name: "rng_bytes",
                args: rng_pair_args,
            },
            BuiltinShapeCase {
                name: "rng_int",
                args: rng_pair_args,
            },
            BuiltinShapeCase {
                name: "regex_match",
                args: regex_match_args,
            },
            BuiltinShapeCase {
                name: "regex_replace",
                args: regex_replace_args,
            },
            BuiltinShapeCase {
                name: "json_get",
                args: json_path_args,
            },
            BuiltinShapeCase {
                name: "json_keys",
                args: json_string_arg,
            },
            BuiltinShapeCase {
                name: "json_length",
                args: json_string_arg,
            },
            BuiltinShapeCase {
                name: "json_parse",
                args: json_string_arg,
            },
            BuiltinShapeCase {
                name: "json_to_string",
                args: json_value_arg,
            },
            BuiltinShapeCase {
                name: "getenv",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "epoch",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "epoch_ms",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "monotonic_ms",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "sleep",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "date_utc_tz",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "date_local_tz",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "date_timezone_of",
                args: one_string,
            },
            BuiltinShapeCase {
                name: "date_fixed_offset",
                args: one_int,
            },
            BuiltinShapeCase {
                name: "date_utc_now",
                args: no_args,
            },
            BuiltinShapeCase {
                name: "date_now_in",
                args: timezone_arg,
            },
            BuiltinShapeCase {
                name: "date_from_instant",
                args: date_instant_tz_args,
            },
            BuiltinShapeCase {
                name: "date_to_epoch_ms",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_to_timezone",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_in_timezone",
                args: date_tz_pair_args,
            },
            BuiltinShapeCase {
                name: "date_year",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_month",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_day",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_hour",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_minute",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_second",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_millisecond",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_weekday",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_to_rfc3339",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_to_rfc2822",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "date_format",
                args: date_format_args,
            },
            BuiltinShapeCase {
                name: "date_parse_rfc3339",
                args: date_rfc3339_arg,
            },
            BuiltinShapeCase {
                name: "date_parse_rfc9557",
                args: date_zoned_arg,
            },
            BuiltinShapeCase {
                name: "assert",
                args: assert_args,
            },
            BuiltinShapeCase {
                name: "assert_eq",
                args: assert_eq_args,
            },
        ]
    }

    fn unsampleable_builtin_reasons() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            (
                "print",
                "VM intercept normalizes the raw helper result to unit; covered by VM output tests",
            ),
            (
                "println",
                "VM intercept normalizes the raw helper result to unit; covered by VM output tests",
            ),
            ("read_line", "would block on host stdin in unit tests"),
            ("http_get", "network I/O is policy/runtime-dependent"),
            ("http", "network I/O is policy/runtime-dependent"),
            ("http_json", "network I/O is policy/runtime-dependent"),
            ("http_msgpack", "network I/O is policy/runtime-dependent"),
            ("http_bytes", "network I/O is policy/runtime-dependent"),
            ("spawn", "runtime-only process request"),
            ("await_process", "runtime-only process request"),
            ("await_process_result", "runtime-only process request"),
            ("cancel", "runtime-only process request"),
            ("wait_any", "runtime-only process request"),
            (
                "exec",
                "VM intercept plus host policy; covered by VM/host exec tests",
            ),
            ("exit", "non-returning process exit"),
            ("panic", "polymorphic non-returning error path"),
            (
                "aws_config_sso_profile",
                "async I/O runtime and credentials required",
            ),
            (
                "aws_config_instance_profile",
                "async I/O runtime and credentials required",
            ),
            ("aws_s3_client", "requires an aws_config host handle"),
            (
                "aws_s3_list_buckets",
                "async I/O runtime and AWS credentials required; shape regression covered separately",
            ),
            ("aws_sqs_client", "requires an aws_config host handle"),
            (
                "aws_sqs_list_queues",
                "async I/O runtime and AWS credentials required",
            ),
            (
                "github_issue_create",
                "requires gh CLI and authenticated GitHub credentials",
            ),
            (
                "github_issue_view",
                "requires gh CLI and authenticated GitHub credentials",
            ),
            (
                "github_issue_update",
                "requires gh CLI and authenticated GitHub credentials",
            ),
        ])
    }

    #[test]
    fn sampled_builtin_runtime_shapes_match_signatures() {
        let entries: HashMap<_, _> = builtin_entries().into_iter().collect();
        for case in sample_cases() {
            let Some(func) = entries.get(case.name).copied() else {
                continue;
            };
            let sig = builtin_type_signature(case.name)
                .unwrap_or_else(|| panic!("missing signature for {}", case.name));
            let shape = parse_return_type(sig.ty)
                .unwrap_or_else(|e| panic!("failed to parse {} signature: {e}", case.name))
                .shape;
            let mut heap = Heap::new();
            let args = (case.args)(&mut heap);
            let value = func(&args, &mut heap)
                .unwrap_or_else(|e| panic!("sample invocation for {} failed: {e}", case.name));
            check_value_shape(value, &heap, &shape).unwrap_or_else(|e| {
                panic!(
                    "{} returned value that does not match {}: {e}",
                    case.name, sig.ty
                )
            });
        }
    }

    #[test]
    fn runtime_shape_coverage_is_explicit_for_enabled_signatures() {
        let sampled: HashSet<_> = sample_cases().into_iter().map(|case| case.name).collect();
        let unsampleable = unsampleable_builtin_reasons();
        let missing: Vec<_> = builtin_type_signatures()
            .map(|(name, _)| name)
            .filter(|name| !sampled.contains(name) && !unsampleable.contains_key(name))
            .collect();
        assert!(
            missing.is_empty(),
            "builtin signatures need a runtime shape sample or an explicit exemption: {missing:?}"
        );
    }
}
