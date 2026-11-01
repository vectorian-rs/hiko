/// `InstallHandler` encodes each clause as `u16 effect_tag, u8 flags,
/// u16 operand`. When this flag bit is set the clause is a provider: the
/// operand is the local slot of a closure taking (payload, Unit), and
/// `Perform` calls it at the perform site; its result is the result of
/// `perform`. When it is clear the clause aborts: the operand is an `i16`
/// relative offset to the clause body, and `Perform` unwinds to the handler
/// and passes the payload and a Unit continuation slot. All other flag bits
/// are reserved and must be zero.
pub const HANDLER_CLAUSE_PROVIDER: u8 = 0b0000_0001;

/// Byte length of one `InstallHandler` clause entry.
pub const HANDLER_CLAUSE_BYTES: usize = 5;

/// `MakeRecClosures` capture source: an upvalue of the current function.
pub const REC_CAPTURE_UPVALUE: u8 = 0;
/// `MakeRecClosures` capture source: a local slot of the current frame.
pub const REC_CAPTURE_LOCAL: u8 = 1;
/// `MakeRecClosures` capture source: a closure created by the same
/// instruction, by its position in the group.
pub const REC_CAPTURE_GROUP: u8 = 2;

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

    AddWord = 27,
    SubWord = 28,

    LtFloat = 29,
    GtFloat = 30,
    LeFloat = 31,
    GeFloat = 32,

    MulWord = 33,
    DivWord = 34,
    ModWord = 35,
    LtWord = 36,
    GtWord = 37,
    LeWord = 38,

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

    CallDirect = 57,
    TailCallDirect = 58,

    GeWord = 59,

    InstallHandler = 53,
    Perform = 54,
    RemoveHandler = 56,

    GetLocal0 = 60,
    GetLocal1 = 61,
    GetLocal2 = 62,
    GetLocal3 = 63,
    SetLocal0 = 64,
    SetLocal1 = 65,
    SetLocal2 = 66,
    SetLocal3 = 67,
    GetField0 = 68,
    GetField1 = 69,
    MakeCons = 70,

    /// Creates a group of closures that may capture each other, for
    /// recursive local `fun ... and ...`. Encoding: `u8 n_closures`, then per
    /// closure `u16 proto_idx, u8 n_captures` and per capture `u8 source,
    /// u16 index`, where `source` is one of the `REC_CAPTURE_*` constants.
    /// Pushes the closures in group order.
    MakeRecClosures = 71,

    /// Pops a data value and jumps unless its constructor tag is `tag`: the
    /// test at the head of a constructor pattern. Encoding: `u16 tag`, then
    /// an `i16` offset relative to the end of the instruction.
    JumpIfNotTag = 72,
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
            27 => Ok(Op::AddWord),
            28 => Ok(Op::SubWord),
            29 => Ok(Op::LtFloat),
            30 => Ok(Op::GtFloat),
            31 => Ok(Op::LeFloat),
            32 => Ok(Op::GeFloat),
            33 => Ok(Op::MulWord),
            34 => Ok(Op::DivWord),
            35 => Ok(Op::ModWord),
            36 => Ok(Op::LtWord),
            37 => Ok(Op::GtWord),
            38 => Ok(Op::LeWord),
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
            56 => Ok(Op::RemoveHandler),
            57 => Ok(Op::CallDirect),
            58 => Ok(Op::TailCallDirect),
            59 => Ok(Op::GeWord),
            60 => Ok(Op::GetLocal0),
            61 => Ok(Op::GetLocal1),
            62 => Ok(Op::GetLocal2),
            63 => Ok(Op::GetLocal3),
            64 => Ok(Op::SetLocal0),
            65 => Ok(Op::SetLocal1),
            66 => Ok(Op::SetLocal2),
            67 => Ok(Op::SetLocal3),
            68 => Ok(Op::GetField0),
            69 => Ok(Op::GetField1),
            70 => Ok(Op::MakeCons),
            71 => Ok(Op::MakeRecClosures),
            72 => Ok(Op::JumpIfNotTag),
            _ => Err(b),
        }
    }
}
