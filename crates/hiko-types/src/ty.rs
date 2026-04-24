use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Con(String),
    Var(u32),
    App(String, Vec<Type>),
    Arrow(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
}

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
    pub fn unit() -> Type {
        Type::Con("Unit".into())
    }
    pub fn pid() -> Type {
        Type::Con("Pid".into())
    }
    pub fn list(elem: Type) -> Type {
        Type::App("list".into(), vec![elem])
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

    pub fn free_vars(&self) -> Vec<u32> {
        let mut vars = Vec::new();
        self.collect_free_vars(&mut vars);
        vars
    }

    fn display_name(name: &str) -> &str {
        match name {
            "Int" => "int",
            "Float" => "float",
            "Word" => "word",
            "Bool" => "bool",
            "String" => "string",
            "Char" => "char",
            "Unit" => "unit",
            "Bytes" => "bytes",
            "Rng" => "rng",
            "Pid" => "pid",
            _ => name,
        }
    }

    fn collect_free_vars(&self, vars: &mut Vec<u32>) {
        match self {
            Type::Var(v) => {
                if !vars.contains(v) {
                    vars.push(*v);
                }
            }
            Type::Arrow(a, b) => {
                a.collect_free_vars(vars);
                b.collect_free_vars(vars);
            }
            Type::Tuple(ts) | Type::App(_, ts) => {
                for t in ts {
                    t.collect_free_vars(vars);
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
