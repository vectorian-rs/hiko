//! VM-only dispatch: policy/availability is selected by the registered table;
//! roots are still on the interpreter stack at the preflight boundary.
use super::*;
use crate::builtins::{json_work::*, support::host_work};
use crate::heap::json_reader::Output;
use crate::value::BuiltinFn;
use hiko_json::Kind;

macro_rules! operations {
    ($($op:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum ReaderOp { $($op),* }
        impl ReaderOp {
            pub(crate) fn of(name: &str) -> Option<Self> { match name { $($name => Some(Self::$op),)* _ => None } }
            fn name(self) -> &'static str { match self { $(Self::$op => $name,)* } }
        }
        pub(crate) fn entries() -> &'static [(&'static str, BuiltinFn)] { &[$(($name, placeholder as BuiltinFn)),*] }
    }
}
operations! {
    OfString => "json_reader_of_string", Kind => "json_reader_kind",
    BeginObject => "json_reader_begin_object", BeginArray => "json_reader_begin_array",
    NextField => "json_reader_next_field", FindNextField => "json_reader_find_next_field",
    NextItem => "json_reader_next_item", ReadNull => "json_reader_read_null",
    ReadBool => "json_reader_read_bool", ReadString => "json_reader_read_string",
    ReadNumberText => "json_reader_read_number_text", ReadInt => "json_reader_read_int",
    ReadWord => "json_reader_read_word", ReadFloat => "json_reader_read_float",
    Skip => "json_reader_skip", FinishObject => "json_reader_finish_object",
    FinishArray => "json_reader_finish_array", Finish => "json_reader_finish",
    Close => "json_reader_close",
}
fn placeholder(_: &[Value], _: &mut Heap) -> Result<Value, String> {
    Err("JSON reader requires VM dispatch".into())
}

impl VM {
    fn reader_string(&self, value: Value) -> Result<Arc<str>, String> {
        if let Value::Heap(r) = value
            && let HeapObject::String(s) = self.heap.get(r).map_err(str::to_owned)?
        {
            return Ok(s.clone());
        }
        Err("expected String".into())
    }
    fn prepare_json_reader_call(
        &mut self,
        objects: usize,
        bytes: usize,
        creation: bool,
    ) -> Result<(), String> {
        let pressure = creation && self.heap.json_reader_resource_pressure();
        self.allocation_safepoint_for(objects, bytes, pressure)
            .map_err(|e| e.message)?;
        if creation {
            self.heap.ensure_json_reader_resource()?;
        }
        Ok(())
    }
    pub(super) fn call_json_reader(
        &mut self,
        op: ReaderOp,
        arg: Value,
    ) -> Result<Value, RuntimeError> {
        self.json_reader_operation(op, arg)
            .map_err(|e| RuntimeError {
                message: format!("{}: {e}", op.name()),
            })
    }
    fn json_reader_operation(&mut self, op: ReaderOp, arg: Value) -> Result<Value, String> {
        if op == ReaderOp::OfString {
            let input = self.reader_string(arg)?;
            if input.len() > hiko_json::Limits::default().max_input_bytes {
                return Err(hiko_json::Error {
                    kind: hiko_json::ErrorKind::InputLimit,
                    offset: 0,
                }
                .to_string());
            }
            let bytes = Heap::json_reader_creation_bytes(input.len())?;
            self.prepare_json_reader_call(1, bytes, true)?;
            self.heap.charge_host_work(host_work(
                JSON_PARSE_HOST_WORK,
                JSON_PARSE_PER_KIB_HOST_WORK,
                input.len(),
            ))?;
            return self.heap.new_json_reader(input);
        }
        // Curried public name->reader maps to the private (name,reader) pair.
        let (reader, name) = if op == ReaderOp::FindNextField {
            let Value::Heap(r) = arg else {
                return Err("expected (String, JSON reader)".into());
            };
            let HeapObject::Tuple(pair) = self.heap.get(r).map_err(str::to_owned)? else {
                return Err("expected (String, JSON reader)".into());
            };
            if pair.len() != 2 {
                return Err("expected (String, JSON reader)".into());
            }
            (pair[1], Some(pair[0]))
        } else {
            (arg, None)
        };
        if op == ReaderOp::Close {
            let (_, id) = self.heap.json_handle(reader)?; // Closed handles are valid for close only.
            if id.0 != 0 {
                self.heap.json_reader(reader)?;
            }
            let charged = self.heap.charge_host_work(JSON_PARSE_HOST_WORK);
            self.heap.close_json_reader(reader)?;
            charged?;
            return Ok(Value::Unit);
        }
        let input_bytes = self.heap.json_reader(reader)?.input_bytes();
        // Once live-owner dispatch succeeds, ANY failure releases the payload.
        let result = (|| {
            let (objects, bytes) = match op {
                ReaderOp::Kind => Heap::json_output_bound(7, false)?,
                ReaderOp::ReadString | ReaderOp::ReadNumberText => {
                    Heap::json_output_bound(input_bytes, false)?
                }
                ReaderOp::NextField => Heap::json_output_bound(input_bytes, true)?,
                _ => (0, 0),
            };
            self.prepare_json_reader_call(objects, bytes, false)?;
            self.heap.charge_host_work(JSON_PARSE_HOST_WORK)?;
            let name = name.map(|value| self.reader_string(value)).transpose()?;
            let admission = self.heap.reserve_json_output(objects, bytes)?;
            self.heap.with_json_reader(reader, |r, heap| {
                let output = match op {
                    ReaderOp::Kind => Output::Text(match r.kind().map_err(|e| e.to_string())? {
                        Kind::Null => "null",
                        Kind::Boolean => "boolean",
                        Kind::Number => "number",
                        Kind::String => "string",
                        Kind::Array => "array",
                        Kind::Object => "object",
                    }),
                    ReaderOp::BeginObject => {
                        r.begin_object().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::BeginArray => {
                        r.begin_array().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::NextField => {
                        Output::Field(r.next_field().map_err(|e| e.to_string())?)
                    }
                    ReaderOp::FindNextField => {
                        let name = name.as_deref().ok_or("missing JSON reader field name")?;
                        Output::Scalar(Value::Bool(
                            r.find_next_field(name).map_err(|e| e.to_string())?,
                        ))
                    }
                    ReaderOp::NextItem => {
                        Output::Scalar(Value::Bool(r.next_item().map_err(|e| e.to_string())?))
                    }
                    ReaderOp::ReadNull => {
                        r.read_null().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::ReadBool => {
                        Output::Scalar(Value::Bool(r.read_bool().map_err(|e| e.to_string())?))
                    }
                    ReaderOp::ReadInt => {
                        Output::Scalar(Value::Int(r.read_int().map_err(|e| e.to_string())?))
                    }
                    ReaderOp::ReadWord => {
                        Output::Scalar(Value::Word(r.read_word().map_err(|e| e.to_string())?))
                    }
                    ReaderOp::ReadFloat => {
                        Output::Scalar(Value::Float(r.read_float().map_err(|e| e.to_string())?))
                    }
                    ReaderOp::ReadString => {
                        Output::Text(r.read_string().map_err(|e| e.to_string())?)
                    }
                    ReaderOp::ReadNumberText => {
                        Output::Text(r.read_number_text().map_err(|e| e.to_string())?)
                    }
                    ReaderOp::Skip => {
                        r.skip().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::FinishObject => {
                        r.finish_object().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::FinishArray => {
                        r.finish_array().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::Finish => {
                        r.finish().map_err(|e| e.to_string())?;
                        Output::Scalar(Value::Unit)
                    }
                    ReaderOp::OfString | ReaderOp::Close => {
                        return Err("invalid JSON reader dispatch".into());
                    }
                };
                heap.alloc_json_output(output, admission)
            })
        })();
        if result.is_err() || op == ReaderOp::Finish {
            self.heap.close_json_reader(reader)?;
        }
        result
    }
}

#[cfg(test)]
mod tests;
