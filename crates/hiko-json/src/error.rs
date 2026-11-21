use std::fmt;

/// Errors carry absolute input byte offsets and never embed input contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax(jiter::JiterErrorType),
    InvalidUtf8,
    InputLimit,
    ScratchLimit,
    WorkLimit,
    DepthLimit,
    Allocation,
    WrongKind,
    NumberRange,
    IntegerTokenRequired,
    NonFinite,
    Underflow,
    Consumed,
    UnconsumedChild,
    Poisoned,
    InternalInvariant,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON reader {:?} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
