#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Const = 0,
    Unit = 1,
    True = 2,
    False = 3,

    GetLocal = 4,
    SetLocal = 5,
    GetUpvalue = 6,
    GetGlobal = 7,
    SetGlobal = 8,

    Pop = 9,

    AddInt = 10,
    SubInt = 11,
    MulInt = 12,
    DivInt = 13,
    ModInt = 14,
    Neg = 15,

    AddFloat = 16,
    SubFloat = 17,
    MulFloat = 18,
    DivFloat = 19,
    NegFloat = 20,

    Eq = 21,
    Ne = 22,
    LtInt = 23,
    GtInt = 24,
    LeInt = 25,
    GeInt = 26,

    EqFloat = 27,
    NeFloat = 28,
    LtFloat = 29,
    GtFloat = 30,
    LeFloat = 31,
    GeFloat = 32,

    EqBool = 33,
    NeBool = 34,
    EqChar = 35,
    NeChar = 36,
    EqString = 37,
    NeString = 38,

    ConcatString = 39,
    Not = 40,

    MakeTuple = 41,
    GetField = 42,

    MakeData = 43,
    GetTag = 44,

    Jump = 45,
    JumpIfFalse = 46,

    MakeClosure = 47,
    Call = 48,
    TailCall = 49,
    Return = 50,

    Panic = 51,

    Halt = 52,

    InstallHandler = 53,
    Perform = 54,
    Resume = 55,
    RemoveHandler = 56,
}

impl TryFrom<u8> for Op {
    type Error = u8;

    fn try_from(b: u8) -> Result<Op, u8> {
        match b {
            0 => Ok(Op::Const),
            1 => Ok(Op::Unit),
            2 => Ok(Op::True),
            3 => Ok(Op::False),
            4 => Ok(Op::GetLocal),
            5 => Ok(Op::SetLocal),
            6 => Ok(Op::GetUpvalue),
            7 => Ok(Op::GetGlobal),
            8 => Ok(Op::SetGlobal),
            9 => Ok(Op::Pop),
            10 => Ok(Op::AddInt),
            11 => Ok(Op::SubInt),
            12 => Ok(Op::MulInt),
            13 => Ok(Op::DivInt),
            14 => Ok(Op::ModInt),
            15 => Ok(Op::Neg),
            16 => Ok(Op::AddFloat),
            17 => Ok(Op::SubFloat),
            18 => Ok(Op::MulFloat),
            19 => Ok(Op::DivFloat),
            20 => Ok(Op::NegFloat),
            21 => Ok(Op::Eq),
            22 => Ok(Op::Ne),
            23 => Ok(Op::LtInt),
            24 => Ok(Op::GtInt),
            25 => Ok(Op::LeInt),
            26 => Ok(Op::GeInt),
            27 => Ok(Op::EqFloat),
            28 => Ok(Op::NeFloat),
            29 => Ok(Op::LtFloat),
            30 => Ok(Op::GtFloat),
            31 => Ok(Op::LeFloat),
            32 => Ok(Op::GeFloat),
            33 => Ok(Op::EqBool),
            34 => Ok(Op::NeBool),
            35 => Ok(Op::EqChar),
            36 => Ok(Op::NeChar),
            37 => Ok(Op::EqString),
            38 => Ok(Op::NeString),
            39 => Ok(Op::ConcatString),
            40 => Ok(Op::Not),
            41 => Ok(Op::MakeTuple),
            42 => Ok(Op::GetField),
            43 => Ok(Op::MakeData),
            44 => Ok(Op::GetTag),
            45 => Ok(Op::Jump),
            46 => Ok(Op::JumpIfFalse),
            47 => Ok(Op::MakeClosure),
            48 => Ok(Op::Call),
            49 => Ok(Op::TailCall),
            50 => Ok(Op::Return),
            51 => Ok(Op::Panic),
            52 => Ok(Op::Halt),
            53 => Ok(Op::InstallHandler),
            54 => Ok(Op::Perform),
            55 => Ok(Op::Resume),
            56 => Ok(Op::RemoveHandler),
            _ => Err(b),
        }
    }
}
