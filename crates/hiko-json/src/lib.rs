//! Experimental forward-only JSON reader. No Hiko VM API is registered.
//!
//! ```
//! use hiko_json::{Limits, Reader, Scratch};
//! # fn main() -> hiko_json::Result<()> {
//! let mut scratch = Scratch::default();
//! let mut reader = Reader::new(br#"{"user":{"age":37},"events":[10,20]}"#,
//!                              &mut scratch, Limits::default())?;
//! let mut root = reader.root()?.object()?;
//! {
//!     let mut field = root.find_next_field("user")?.unwrap();
//!     let mut user = field.value()?.object()?;
//!     let mut age = user.find_next_field("age")?.unwrap();
//!     assert_eq!(age.value()?.u64()?, 37);
//!     drop(age);
//!     user.finish()?;
//! }
//! root.finish()?;
//! reader.finish()?; // validates the remainder and EOF, not just selected fields
//! # Ok(()) }
//! ```
//!
//! A child exclusively borrows its parent. It cannot be retained while the
//! parent advances (this example must not compile):
//!
//! ```compile_fail
//! use hiko_json::{Limits, Reader, Scratch};
//! let mut scratch = Scratch::default();
//! let mut reader = Reader::new(b"[1,2]", &mut scratch, Limits::default()).unwrap();
//! let mut array = reader.root().unwrap().array().unwrap();
//! let mut first = array.next_value().unwrap().unwrap();
//! let second = array.next_value().unwrap();
//! first.u64().unwrap();
//! ```
//!
//! Decoded strings cannot outlive their value guard or its scratch borrow:
//!
//! ```compile_fail
//! use hiko_json::{Limits, Reader, Scratch};
//! let mut scratch = Scratch::default();
//! let mut reader = Reader::new(br#""\u0061""#, &mut scratch, Limits::default()).unwrap();
//! let mut value = reader.root().unwrap();
//! let decoded = value.string().unwrap();
//! drop(value);
//! reader.finish().unwrap();
//! assert_eq!(decoded, "a");
//! ```
#![forbid(unsafe_code)]

mod error;
mod reader;

pub use error::{Error, ErrorKind, Result};
pub use reader::{Array, Field, Kind, Limits, Object, Reader, Scratch, Stats, Value};
