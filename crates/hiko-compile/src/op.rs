#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Const,
    Unit,
    True,
    False,

    GetLocal,
    SetLocal,
    GetUpvalue,
    GetGlobal,
    SetGlobal,

    Pop,

    AddInt,
    SubInt,
    MulInt,
    DivInt,
    ModInt,
    NegInt,

    AddFloat,
    SubFloat,
    MulFloat,
    DivFloat,
    NegFloat,

    EqInt,
    NeInt,
    LtInt,
    GtInt,
    LeInt,
    GeInt,

    EqFloat,
    NeFloat,
    LtFloat,
    GtFloat,
    LeFloat,
    GeFloat,

    EqBool,
    NeBool,
    EqChar,
    NeChar,
    EqString,
    NeString,

    ConcatString,
    Not,

    MakeTuple,
    GetField,

    MakeData,
    GetTag,

    Jump,
    JumpIfFalse,

    MakeClosure,
    Call,
    Return,

    Halt,
}

impl Op {
    pub fn from_byte(b: u8) -> Option<Op> {
        if b <= Op::Halt as u8 {
            Some(unsafe { std::mem::transmute::<u8, Op>(b) })
        } else {
            None
        }
    }
}
