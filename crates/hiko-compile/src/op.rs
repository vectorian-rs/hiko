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

    /// Compare and branch: pop `b`, then `a`, and jump unless the comparison
    /// holds. Encoding: an `i16` offset relative to the end of the
    /// instruction. `JumpIfNotEq` and `JumpIfEq` use structural equality,
    /// like `Eq`; the others compare ints, like `LtInt` and friends.
    JumpIfNotEq = 73,
    JumpIfEq = 74,
    JumpIfNotLtInt = 75,
    JumpIfNotGtInt = 76,
    JumpIfNotLeInt = 77,
    JumpIfNotGeInt = 78,
}

impl Op {
    /// The opcode with byte value `b`, or `None` if no opcode has it.
    const fn decode(b: u8) -> Option<Op> {
        match b {
            0 => Some(Op::Const),
            1 => Some(Op::Unit),
            2 => Some(Op::True),
            3 => Some(Op::False),
            4 => Some(Op::GetLocal),
            5 => Some(Op::SetLocal),
            6 => Some(Op::GetUpvalue),
            7 => Some(Op::GetGlobal),
            8 => Some(Op::SetGlobal),
            9 => Some(Op::Pop),
            10 => Some(Op::AddInt),
            11 => Some(Op::SubInt),
            12 => Some(Op::MulInt),
            13 => Some(Op::DivInt),
            14 => Some(Op::ModInt),
            15 => Some(Op::Neg),
            16 => Some(Op::AddFloat),
            17 => Some(Op::SubFloat),
            18 => Some(Op::MulFloat),
            19 => Some(Op::DivFloat),
            20 => Some(Op::NegFloat),
            21 => Some(Op::Eq),
            22 => Some(Op::Ne),
            23 => Some(Op::LtInt),
            24 => Some(Op::GtInt),
            25 => Some(Op::LeInt),
            26 => Some(Op::GeInt),
            27 => Some(Op::AddWord),
            28 => Some(Op::SubWord),
            29 => Some(Op::LtFloat),
            30 => Some(Op::GtFloat),
            31 => Some(Op::LeFloat),
            32 => Some(Op::GeFloat),
            33 => Some(Op::MulWord),
            34 => Some(Op::DivWord),
            35 => Some(Op::ModWord),
            36 => Some(Op::LtWord),
            37 => Some(Op::GtWord),
            38 => Some(Op::LeWord),
            39 => Some(Op::ConcatString),
            40 => Some(Op::Not),
            41 => Some(Op::MakeTuple),
            42 => Some(Op::GetField),
            43 => Some(Op::MakeData),
            45 => Some(Op::Jump),
            46 => Some(Op::JumpIfFalse),
            47 => Some(Op::MakeClosure),
            48 => Some(Op::Call),
            49 => Some(Op::TailCall),
            50 => Some(Op::Return),
            51 => Some(Op::Panic),
            52 => Some(Op::Halt),
            53 => Some(Op::InstallHandler),
            54 => Some(Op::Perform),
            56 => Some(Op::RemoveHandler),
            57 => Some(Op::CallDirect),
            58 => Some(Op::TailCallDirect),
            59 => Some(Op::GeWord),
            60 => Some(Op::GetLocal0),
            61 => Some(Op::GetLocal1),
            62 => Some(Op::GetLocal2),
            63 => Some(Op::GetLocal3),
            64 => Some(Op::SetLocal0),
            65 => Some(Op::SetLocal1),
            66 => Some(Op::SetLocal2),
            67 => Some(Op::SetLocal3),
            68 => Some(Op::GetField0),
            69 => Some(Op::GetField1),
            70 => Some(Op::MakeCons),
            71 => Some(Op::MakeRecClosures),
            72 => Some(Op::JumpIfNotTag),
            73 => Some(Op::JumpIfNotEq),
            74 => Some(Op::JumpIfEq),
            75 => Some(Op::JumpIfNotLtInt),
            76 => Some(Op::JumpIfNotGtInt),
            77 => Some(Op::JumpIfNotLeInt),
            78 => Some(Op::JumpIfNotGeInt),
            _ => None,
        }
    }

    /// `decode` for every byte value, so decoding an instruction is one
    /// table load instead of a `match` with an error path.
    const DECODE: [Option<Op>; 256] = {
        let mut table = [None; 256];
        let mut b = 0;
        while b < 256 {
            table[b] = Op::decode(b as u8);
            b += 1;
        }
        table
    };
}

impl TryFrom<u8> for Op {
    type Error = u8;

    fn try_from(b: u8) -> Result<Op, u8> {
        Op::DECODE[b as usize].ok_or(b)
    }
}
