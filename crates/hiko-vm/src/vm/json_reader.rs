//! VM-only dispatch: policy/availability is selected by the registered table;
//! roots are still on the interpreter stack at the preflight boundary.
use super::*;
use crate::builtins::{json_work::*, support::host_work};
use crate::heap::json_reader::{Creation, Output};
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
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Null => "null",
        Kind::Boolean => "boolean",
        Kind::Number => "number",
        Kind::String => "string",
        Kind::Array => "array",
        Kind::Object => "object",
    }
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
            .map_err(|e| e.message)
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
            let work = host_work(
                JSON_PARSE_HOST_WORK,
                JSON_PARSE_PER_KIB_HOST_WORK,
                input.len(),
            );
            let creation = Creation::new(input)?;
            self.prepare_json_reader_call(1, creation.bytes, true)?;
            return self.heap.create_json_reader(creation, work);
        }
        // Curried public name->reader maps to the private (name,reader) pair.
        let (reader, name) = if op == ReaderOp::FindNextField {
            let pair = match arg {
                Value::Heap(r) => match self.heap.get(r).map_err(str::to_owned)? {
                    HeapObject::Tuple(t) if t.len() == 2 => Some((t[1], t[0])),
                    _ => None,
                },
                _ => None,
            };
            let (reader, name) = pair.ok_or("expected (String, JSON reader)")?;
            (reader, Some(name))
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
        // No heap borrow survives parsing/GC. Arguments remain VM roots, keeping
        // this handle's payload charge/slot alive while its owner is local.
        let mut owners = self.heap.take_json_reader(reader)?;
        let result = (|| {
            self.gc_failed().map_err(|e| e.message)?;
            self.heap.charge_host_work(JSON_PARSE_HOST_WORK)?;
            let name = name.map(|value| self.reader_string(value)).transpose()?;
            let r = &mut owners[0];
            let unit = |res: hiko_json::Result<()>| res.map(|()| Output::Scalar(Value::Unit));
            let output = match op {
                ReaderOp::Kind => r.kind().map(|kind| Output::Text(kind_name(kind))),
                ReaderOp::BeginObject => unit(r.begin_object()),
                ReaderOp::BeginArray => unit(r.begin_array()),
                ReaderOp::NextField => r.next_field().map(Output::Field),
                ReaderOp::FindNextField => {
                    let name = name.as_deref().ok_or("missing JSON reader field name")?;
                    r.find_next_field(name)
                        .map(|found| Output::Scalar(Value::Bool(found)))
                }
                ReaderOp::NextItem => r.next_item().map(|b| Output::Scalar(Value::Bool(b))),
                ReaderOp::ReadNull => unit(r.read_null()),
                ReaderOp::ReadBool => r.read_bool().map(|b| Output::Scalar(Value::Bool(b))),
                ReaderOp::ReadInt => r.read_int().map(|n| Output::Scalar(Value::Int(n))),
                ReaderOp::ReadWord => r.read_word().map(|n| Output::Scalar(Value::Word(n))),
                ReaderOp::ReadFloat => r.read_float().map(|x| Output::Scalar(Value::Float(x))),
                ReaderOp::ReadString => r.read_string().map(Output::Text),
                ReaderOp::ReadNumberText => r.read_number_text().map(Output::Text),
                ReaderOp::Skip => unit(r.skip()),
                ReaderOp::FinishObject => unit(r.finish_object()),
                ReaderOp::FinishArray => unit(r.finish_array()),
                ReaderOp::Finish => unit(r.finish()),
                ReaderOp::OfString | ReaderOp::Close => {
                    return Err("invalid JSON reader dispatch".into());
                }
            }
            .map_err(|e| e.to_string())?;
            // Text already lives in accounted owner scratch/input (or static
            // kind names). Do not copy it or reserve the unread input. GC cannot
            // invalidate this local owner or the rooted handle's ledger slot.
            let (objects, bytes) = output.layout()?;
            self.prepare_json_reader_call(objects, bytes, false)?;
            let admission = self.heap.reserve_json_output(objects, bytes)?;
            self.heap.alloc_json_output(output, admission)
        })();
        let cleanup =
            self.heap
                .end_json_reader(reader, owners, result.is_ok() && op != ReaderOp::Finish);
        // Always clean up, but never replace the operation's original error.
        result.and_then(|value| cleanup.map(|()| value))
    }
}

#[cfg(test)]
mod tests;
