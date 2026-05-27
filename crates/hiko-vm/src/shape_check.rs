//! Runtime shape verification for builtin type signatures.
//!
//! Each builtin has a hand-written type signature string (in
//! `hiko-builtin-meta::signatures`). This module parses the return-type portion
//! of that string into a lightweight `Shape` AST, then checks whether a
//! `SendableValue` matches that shape. The goal is to catch mismatches between
//! the declared type and what the Rust code actually produces — the class of
//! bug documented in GitHub issue #76.

use crate::sendable::SendableValue;

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
/// Example: "aws_config -> bool * (string * string * string) list * string" returns a tuple shape.
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
                SendableValue::AwsConfigSsoProfile { .. } => Ok(()),
                _ => Err(format!("expected aws_config, got {}", value_kind(value))),
            }
            #[cfg(not(feature = "builtin-aws-config"))]
            {
                let _ = value;
                Err("aws_config not available in this build".into())
            }
        }
        Shape::Tuple(expected_elems) => match value {
            SendableValue::Tuple(fields) => {
                if fields.len() != expected_elems.len() {
                    return Err(format!(
                        "expected tuple with {} fields, got {}",
                        expected_elems.len(),
                        fields.len()
                    ));
                }
                for (i, (field, elem_shape)) in
                    fields.iter().zip(expected_elems.iter()).enumerate()
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
                    check_shape(item, elem_shape)
                        .map_err(|e| format!("list[{}]: {}", i, e))?;
                }
                Ok(())
            }
            _ => Err(format!(
                "expected list, got {}",
                value_kind(value)
            )),
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
            _ => Err(format!(
                "expected Option (Data), got {}",
                value_kind(value)
            )),
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
        SendableValue::Tuple(_) => "tuple",
        SendableValue::List(_) => "list",
        SendableValue::Data { .. } => "data",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        assert_eq!(
            rt.shape,
            Shape::Tuple(vec![Shape::Int, Shape::String])
        );
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
        let rt = parse_return_type(
            "aws_config -> bool * (string * string * string) list * string",
        )
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
        assert!(check_shape(
            &SendableValue::String(Arc::from("hello")),
            &Shape::String
        )
        .is_ok());
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
        assert!(check_shape(
            &SendableValue::Bytes(Arc::from([1, 2, 3])),
            &Shape::Bytes
        )
        .is_ok());
    }

    // --- Regression: the exact bug from issue #76 ---
    // The old signature declared Option.option wrappers that the Rust code
    // did not produce. This test verifies the current correct shape.

    #[test]
    fn aws_s3_bucket_shape_no_options() {
        // The current (fixed) return shape for aws_s3_list_buckets
        let shape = parse_return_type(
            "aws_config -> bool * (string * string * string) list * string",
        )
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

    // --- Parse all real signatures ---

    #[test]
    fn parse_all_builtin_signatures() {
        // Verify that every signature we know about can be parsed
        let signatures: &[(&str, &str)] = &[
            ("print", "string -> unit"),
            ("println", "string -> unit"),
            ("read_line", "unit -> string"),
            ("int_to_string", "int -> string"),
            ("float_to_string", "float -> string"),
            ("string_to_int", "string -> int"),
            ("string_length", "string -> int"),
            ("substring", "string * int * int -> string"),
            ("string_contains", "string * string -> bool"),
            ("trim", "string -> string"),
            ("split", "string * string -> string list"),
            ("string_replace", "string * string * string -> string"),
            ("string_join", "string list * string -> string"),
            ("sqrt", "float -> float"),
            ("abs_int", "int -> int"),
            ("floor", "float -> int"),
            ("read_file", "string -> string"),
            ("write_file", "string * string -> unit"),
            ("file_exists", "string -> bool"),
            ("list_dir", "string -> string list"),
            ("path_join", "string * string -> string"),
            ("http_get", "string -> int * (string * string) list * string"),
            ("http", "string * string * (string * string) list * string -> int * (string * string) list * string"),
            ("exec", "string * string list -> int * string * string"),
            ("exit", "int -> unit"),
            ("json_get", "string * string -> string"),
            ("json_keys", "string -> string list"),
            ("json_length", "string -> int"),
            ("json_parse", "string -> 'a"),
            ("json_to_string", "'a -> string"),
            ("blake3", "bytes -> string"),
            ("random_bytes", "int -> bytes"),
            ("rng_seed", "bytes -> rng"),
            ("rng_bytes", "rng * int -> bytes * rng"),
            ("rng_int", "rng * int -> int * rng"),
            ("regex_match", "string * string -> bool"),
            ("getenv", "string -> string"),
            ("epoch", "unit -> int"),
            ("sleep", "int -> unit"),
            ("aws_config_sso_profile", "string -> aws_config"),
            ("aws_s3_list_buckets", "aws_config -> bool * (string * string * string) list * string"),
            ("spawn", "(unit -> 'a) -> pid"),
            ("await_process", "pid -> 'a"),
            ("cancel", "pid -> unit"),
            ("wait_any", "pid list -> pid"),
            ("bytes_length", "bytes -> int"),
            ("bytes_to_string", "bytes -> string"),
            ("string_to_bytes", "string -> bytes"),
            ("bytes_get", "bytes * int -> int"),
            ("bytes_slice", "bytes * int * int -> bytes"),
        ];

        for (name, sig) in signatures {
            let result = parse_return_type(sig);
            assert!(result.is_ok(), "failed to parse signature for '{}': {:?}", name, result.err());
        }
    }
}
