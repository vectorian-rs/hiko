//! Builtin registration and runtime-bound builtin dispatch.

use super::*;
use crate::value::BuiltinFn;

impl VM {
    pub(super) fn register_builtins(&mut self) {
        for (name, func) in crate::builtins::builtin_entries() {
            self.register_builtin(name, func);
        }
    }

    pub(super) fn global_slot(&mut self, name: String) -> usize {
        if let Some(&slot) = self.global_names.get(&name) {
            slot
        } else {
            let slot = self.globals.len();
            self.globals.push(Value::Unit);
            self.global_names.insert(name, slot);
            slot
        }
    }

    /// Register a single builtin function by name.
    pub fn register_builtin(&mut self, name: impl Into<Arc<str>>, func: BuiltinFn) {
        let name: Arc<str> = name.into();
        let idx = self.builtins.len() as u16;
        self.builtins.push(BuiltinEntry {
            name: name.clone(),
            func,
        });
        let slot = self.global_slot(name.to_string());
        self.globals[slot] = Value::Builtin(idx);
        match name.as_ref() {
            "print" => self.print_builtin_id = Some(idx),
            "println" => self.println_builtin_id = Some(idx),
            "exec" => self.exec_builtin_id = Some(idx),
            "spawn" => self.spawn_builtin_id = Some(idx),
            "await_process" => self.await_builtin_id = Some(idx),
            "await_process_result" => self.await_result_builtin_id = Some(idx),
            "cancel" => self.cancel_builtin_id = Some(idx),
            "wait_any" => self.wait_any_builtin_id = Some(idx),
            "sleep" => self.sleep_builtin_id = Some(idx),
            "http_get" => self.http_get_builtin_id = Some(idx),
            "http" => self.http_builtin_id = Some(idx),
            "http_json" => self.http_json_builtin_id = Some(idx),
            "http_msgpack" => self.http_msgpack_builtin_id = Some(idx),
            "http_bytes" => self.http_bytes_builtin_id = Some(idx),
            "read_file" => self.read_file_builtin_id = Some(idx),
            _ => {}
        }
    }

    /// Register a builtin with an owned name string.
    pub fn register_builtin_owned(&mut self, name: String, func: BuiltinFn) {
        self.register_builtin(name, func);
    }

    /// Common path for builtins that suspend the current process and hand
    /// control back to the runtime.
    pub(super) fn suspend_for_runtime_request(
        &mut self,
        request: RuntimeRequest,
        callee_pos: usize,
    ) -> Result<(), RuntimeError> {
        self.pending_runtime_request = Some(request);
        self.stack.truncate(callee_pos);
        self.push(Value::Unit)?;
        Err(RuntimeError {
            message: "runtime request".into(),
        })
    }

    pub(super) fn call_builtin(
        &mut self,
        builtin_id: u16,
        callee_pos: usize,
        arity: usize,
    ) -> Result<(), RuntimeError> {
        let first_arg = self.stack[callee_pos + 1];

        if self.spawn_builtin_id == Some(builtin_id) {
            let closure_val = self.stack[callee_pos + 1];
            match closure_val {
                Value::Heap(r) => match self.heap.get(r) {
                    Ok(HeapObject::Closure {
                        proto_idx,
                        captures,
                    }) => {
                        let mut serialized = Vec::new();
                        for &v in captures.iter() {
                            serialized.push(crate::sendable::serialize(v, &self.heap).map_err(
                                |e| RuntimeError {
                                    message: format!("spawn: {e}"),
                                },
                            )?);
                        }
                        return self.suspend_for_runtime_request(
                            RuntimeRequest::Spawn {
                                proto_idx: *proto_idx,
                                captures: serialized,
                            },
                            callee_pos,
                        );
                    }
                    _ => {
                        return Err(RuntimeError {
                            message: "spawn: expected a function".into(),
                        });
                    }
                },
                _ => {
                    return Err(RuntimeError {
                        message: "spawn: expected a function".into(),
                    });
                }
            }
        }

        if self.await_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    return self
                        .suspend_for_runtime_request(RuntimeRequest::Await(pid), callee_pos);
                }
                _ => {
                    return Err(RuntimeError {
                        message: "await_process: expected Pid".into(),
                    });
                }
            }
        }

        if self.await_result_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    return self
                        .suspend_for_runtime_request(RuntimeRequest::AwaitResult(pid), callee_pos);
                }
                _ => {
                    return Err(RuntimeError {
                        message: "await_process_result: expected Pid".into(),
                    });
                }
            }
        }

        if self.cancel_builtin_id == Some(builtin_id) {
            let pid_val = self.stack[callee_pos + 1];
            match pid_val {
                Value::Pid(pid) => {
                    return self
                        .suspend_for_runtime_request(RuntimeRequest::Cancel(pid), callee_pos);
                }
                _ => {
                    return Err(RuntimeError {
                        message: "cancel: expected Pid".into(),
                    });
                }
            }
        }

        if self.wait_any_builtin_id == Some(builtin_id) {
            let pids = crate::builtins::extract_pid_list_arg(
                &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                &self.heap,
                "wait_any",
            )
            .map_err(|message| RuntimeError { message })?;
            return self.suspend_for_runtime_request(RuntimeRequest::WaitAny(pids), callee_pos);
        }

        if self.async_io {
            let io_request = if self.sleep_builtin_id == Some(builtin_id) {
                let ms = match self.stack[callee_pos + 1] {
                    Value::Int(ms) if ms >= 0 => ms as u64,
                    _ => {
                        return Err(RuntimeError {
                            message: "sleep: expected non-negative Int (milliseconds)".into(),
                        });
                    }
                };
                Some(crate::io_backend::IoRequest::Sleep(
                    std::time::Duration::from_millis(ms),
                ))
            } else if self.http_get_builtin_id == Some(builtin_id) {
                let url = crate::builtins::extract_string_arg(
                    &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                    &self.heap,
                    "http_get",
                )
                .map_err(|msg| RuntimeError { message: msg })?;
                self.heap
                    .check_http_host_for("http_get", &url)
                    .map_err(|e| RuntimeError {
                        message: format!("http_get: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::HttpGet { url })
            } else if let Some(format) = self.match_http_builtin(builtin_id) {
                let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
                let (method, url, headers, body) =
                    crate::builtins::extract_http_args(args, &self.heap, "http")
                        .map_err(|msg| RuntimeError { message: msg })?;
                let builtin_name = self.builtins[builtin_id as usize].name.as_ref();
                self.heap
                    .check_http_host_for(builtin_name, &url)
                    .map_err(|e| RuntimeError {
                        message: format!("{builtin_name}: {e}"),
                    })?;
                self.heap
                    .charge_io_bytes(body.len() as u64)
                    .map_err(|e| RuntimeError {
                        message: format!("{builtin_name}: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::Http {
                    method,
                    url,
                    headers,
                    body,
                    format,
                })
            } else if self.read_file_builtin_id == Some(builtin_id) {
                let path = crate::builtins::extract_string_arg(
                    &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                    &self.heap,
                    "read_file",
                )
                .map_err(|msg| RuntimeError { message: msg })?;
                let checked = self
                    .heap
                    .check_fs_path_for("read_file", &path)
                    .map_err(|e| RuntimeError {
                        message: format!("read_file: {e}"),
                    })?;
                Some(crate::io_backend::IoRequest::ReadFile {
                    path: checked.to_string_lossy().to_string(),
                })
            } else {
                None
            };
            if let Some(request) = io_request {
                return self.suspend_for_runtime_request(RuntimeRequest::Io(request), callee_pos);
            }
        }

        if self.exec_builtin_id == Some(builtin_id) {
            let exec_arg = self.stack[callee_pos + 1];
            let prepared = self.prepare_exec(exec_arg)?;
            let result = self
                .run_exec(prepared)
                .map_err(|msg| RuntimeError { message: msg })?;
            self.stack.truncate(callee_pos);
            self.push(result)?;
            return Ok(());
        }

        let func = self.builtins[builtin_id as usize].func;
        let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
        let result = func(args, &mut self.heap).map_err(|msg| RuntimeError { message: msg })?;
        self.stack.truncate(callee_pos);
        let is_print = self.print_builtin_id == Some(builtin_id);
        let is_println = self.println_builtin_id == Some(builtin_id);
        if is_print || is_println {
            let displayed = if is_println {
                format!("{}\n", self.display_value(&first_arg))
            } else {
                self.display_value(&first_arg)
            };
            self.heap
                .charge_io_bytes(displayed.len() as u64)
                .map_err(|e| RuntimeError {
                    message: format!("stdout: {e}"),
                })?;
            if let Some(output) = &mut self.output {
                output.push(displayed.clone());
            }
            if let Some(sink) = &self.output_sink {
                sink.write(&displayed).map_err(|e| RuntimeError {
                    message: format!("stdout: {e}"),
                })?;
            }
            self.push(Value::Unit)?;
        } else {
            self.push(result)?;
        }
        Ok(())
    }

    fn match_http_builtin(&self, builtin_id: u16) -> Option<crate::io_backend::HttpResponseFormat> {
        use crate::io_backend::HttpResponseFormat;
        if self.http_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Text)
        } else if self.http_json_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Json)
        } else if self.http_msgpack_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Msgpack)
        } else if self.http_bytes_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Bytes)
        } else {
            None
        }
    }
}
