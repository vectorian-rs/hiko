use std::fmt;
use std::rc::Rc;

pub type BuiltinFn = fn(&[Value]) -> Result<Value, String>;

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Unit,
    String(Rc<String>),
    Tuple(Rc<Vec<Value>>),
    Data(Rc<DataValue>),
    Closure(Rc<ClosureValue>),
    Builtin { name: &'static str, func: BuiltinFn },
}

#[derive(Debug, Clone)]
pub struct DataValue {
    pub tag: u16,
    pub fields: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ClosureValue {
    pub proto_idx: usize,
    pub captures: Vec<Value>,
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Char(c) => write!(f, "#\"{c}\""),
            Value::Unit => write!(f, "()"),
            Value::String(s) => write!(f, "\"{s}\""),
            Value::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e:?}")?;
                }
                write!(f, ")")
            }
            Value::Data(d) => write!(f, "Data({}, {:?})", d.tag, d.fields),
            Value::Closure(c) => write!(f, "<fn:{}>", c.proto_idx),
            Value::Builtin { name, .. } => write!(f, "<builtin:{name}>"),
        }
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
            Value::String(s) => write!(f, "{s}"),
            Value::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e}")?;
                }
                write!(f, ")")
            }
            Value::Data(d) => {
                if d.tag == 0 && d.fields.is_empty() {
                    write!(f, "[]")
                } else if d.tag == 1 && d.fields.len() == 2 {
                    write!(f, "{} :: {}", d.fields[0], d.fields[1])
                } else {
                    write!(f, "Data({}, {:?})", d.tag, d.fields)
                }
            }
            Value::Closure(_) => write!(f, "<fn>"),
            Value::Builtin { name, .. } => write!(f, "<builtin:{name}>"),
        }
    }
}
