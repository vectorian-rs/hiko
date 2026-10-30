use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Con(String),
    Var(u32),
    App(String, Vec<Type>),
    Arrow(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
}

/// Primitive types as `(surface name, internal constructor name)`.
///
/// Source programs spell primitives in lowercase; the type checker
/// represents them as `Type::Con` with the capitalized internal name.
pub const PRIMITIVE_TYPES: &[(&str, &str)] = &[
    ("int", "Int"),
    ("float", "Float"),
    ("word", "Word"),
    ("bool", "Bool"),
    ("string", "String"),
    ("char", "Char"),
    ("unit", "Unit"),
    ("bytes", "Bytes"),
    ("rng", "Rng"),
    ("aws_config", "AwsConfig"),
    ("aws_s3_client", "AwsS3Client"),
    ("aws_sqs_client", "AwsSqsClient"),
    ("pid", "Pid"),
];

#[derive(Debug, Clone)]
pub struct Scheme {
    pub vars: Vec<u32>,
    pub ty: Type,
}

impl Type {
    pub fn int() -> Type {
        Type::Con("Int".into())
    }
    pub fn float() -> Type {
        Type::Con("Float".into())
    }
    pub fn word() -> Type {
        Type::Con("Word".into())
    }
    pub fn bool() -> Type {
        Type::Con("Bool".into())
    }
    pub fn string() -> Type {
        Type::Con("String".into())
    }
    pub fn char() -> Type {
        Type::Con("Char".into())
    }
    pub fn bytes() -> Type {
        Type::Con("Bytes".into())
    }
    pub fn rng() -> Type {
        Type::Con("Rng".into())
    }
    pub fn aws_config() -> Type {
        Type::Con("AwsConfig".into())
    }
    pub fn aws_s3_client() -> Type {
        Type::Con("AwsS3Client".into())
    }
    pub fn aws_sqs_client() -> Type {
        Type::Con("AwsSqsClient".into())
    }
    pub fn unit() -> Type {
        Type::Con("Unit".into())
    }
    pub fn pid() -> Type {
        Type::Con("Pid".into())
    }
    pub fn list(elem: Type) -> Type {
        Type::App("list".into(), vec![elem])
    }

    /// Resolve a primitive type by its surface (lowercase) name.
    pub fn primitive(surface_name: &str) -> Option<Type> {
        PRIMITIVE_TYPES
            .iter()
            .find(|(surface, _)| *surface == surface_name)
            .map(|(_, internal)| Type::Con((*internal).into()))
    }

    /// The surface name of a primitive, given its internal constructor name.
    pub fn primitive_surface_name(internal_name: &str) -> Option<&'static str> {
        PRIMITIVE_TYPES
            .iter()
            .find(|(_, internal)| *internal == internal_name)
            .map(|(surface, _)| *surface)
    }

    pub fn arrow(from: Type, to: Type) -> Type {
        Type::Arrow(Box::new(from), Box::new(to))
    }

    pub fn is_equality(&self) -> bool {
        match self {
            Type::Con(n) => matches!(
                n.as_str(),
                "Int" | "Float" | "Word" | "Bool" | "String" | "Char" | "Unit" | "Bytes" | "Pid"
            ),
            Type::Var(_) => false, // conservative: block until resolved to a concrete equality type
            Type::Tuple(elems) => elems.iter().all(|e| e.is_equality()),
            _ => false,
        }
    }

    /// The type variables in `self`, in order of first occurrence.
    pub fn free_vars(&self) -> Vec<u32> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars, &mut HashSet::new());
        vars
    }

    fn display_name(name: &str) -> &str {
        Type::primitive_surface_name(name).unwrap_or(name)
    }

    fn collect_free_vars(&self, vars: &mut Vec<u32>, seen: &mut HashSet<u32>) {
        match self {
            Type::Var(v) => {
                if seen.insert(*v) {
                    vars.push(*v);
                }
            }
            Type::Arrow(a, b) => {
                a.collect_free_vars(vars, seen);
                b.collect_free_vars(vars, seen);
            }
            Type::Tuple(ts) | Type::App(_, ts) => {
                for t in ts {
                    t.collect_free_vars(vars, seen);
                }
            }
            Type::Con(_) => {}
        }
    }
}

impl Scheme {
    pub fn mono(ty: Type) -> Scheme {
        Scheme { vars: vec![], ty }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Con(name) => write!(f, "{}", Type::display_name(name)),
            Type::Var(v) => {
                let c = (b'a' + (*v % 26) as u8) as char;
                let suffix = if *v >= 26 {
                    format!("{}", *v / 26)
                } else {
                    String::new()
                };
                write!(f, "'{c}{suffix}")
            }
            Type::Arrow(a, b) => match a.as_ref() {
                Type::Arrow(_, _) => write!(f, "({a}) -> {b}"),
                _ => write!(f, "{a} -> {b}"),
            },
            Type::Tuple(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        write!(f, " * ")?;
                    }
                    match t {
                        Type::Arrow(_, _) | Type::Tuple(_) => write!(f, "({t})")?,
                        _ => write!(f, "{t}")?,
                    }
                }
                Ok(())
            }
            Type::App(name, args) => {
                if args.len() == 1 {
                    match &args[0] {
                        Type::Arrow(_, _) | Type::Tuple(_) | Type::App(_, _) => {
                            write!(f, "({}) {}", args[0], Type::display_name(name))
                        }
                        _ => write!(f, "{} {}", args[0], Type::display_name(name)),
                    }
                } else {
                    write!(f, "(")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    write!(f, ") {}", Type::display_name(name))
                }
            }
        }
    }
}
