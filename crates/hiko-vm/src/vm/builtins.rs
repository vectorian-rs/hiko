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
        let table = Arc::make_mut(&mut self.builtins);
        let idx = table.entries.len() as u16;
        table.entries.push(BuiltinEntry {
            name: name.clone(),
            func,
        });
        table.values.push(Value::Builtin(idx));
        let ids = &mut table.ids;
        match name.as_ref() {
            "print" => ids.print_builtin_id = Some(idx),
            "println" => ids.println_builtin_id = Some(idx),
            "exec" => ids.exec_builtin_id = Some(idx),
            "spawn" => ids.spawn_builtin_id = Some(idx),
            "await_process" => ids.await_builtin_id = Some(idx),
            "await_process_result" => ids.await_result_builtin_id = Some(idx),
            "cancel" => ids.cancel_builtin_id = Some(idx),
            "wait_any" => ids.wait_any_builtin_id = Some(idx),
            "sleep" => ids.sleep_builtin_id = Some(idx),
            "http_get" => ids.http_get_builtin_id = Some(idx),
            "http" => ids.http_builtin_id = Some(idx),
            "http_json" => ids.http_json_builtin_id = Some(idx),
            "http_msgpack" => ids.http_msgpack_builtin_id = Some(idx),
            "http_bytes" => ids.http_bytes_builtin_id = Some(idx),
            "read_file" => ids.read_file_builtin_id = Some(idx),
            #[cfg(feature = "builtin-aws-config")]
            "aws_config_sso_profile" => ids.aws_config_sso_profile_builtin_id = Some(idx),
            #[cfg(feature = "builtin-aws-config")]
            "aws_config_instance_profile" => ids.aws_config_instance_profile_builtin_id = Some(idx),
            #[cfg(feature = "builtin-aws-s3")]
            "aws_s3_list_buckets" => ids.aws_s3_list_buckets_builtin_id = Some(idx),
            #[cfg(feature = "builtin-aws-sqs")]
            "aws_sqs_list_queues" => ids.aws_sqs_list_queues_builtin_id = Some(idx),
            #[cfg(feature = "builtin-github")]
            "github_issue_create" => ids.github_issue_create_builtin_id = Some(idx),
            #[cfg(feature = "builtin-github")]
            "github_issue_view" => ids.github_issue_view_builtin_id = Some(idx),
            #[cfg(feature = "builtin-github")]
            "github_issue_update" => ids.github_issue_update_builtin_id = Some(idx),
            _ => {}
        }
        table.by_name.insert(name.clone(), idx);
        // A global already resolved to an earlier builtin of this name now
        // refers to the new one.
        if let Some(&slot) = self.global_names.get(name.as_ref())
            && matches!(self.globals[slot], Value::Builtin(_))
        {
            self.globals[slot] = Value::Builtin(idx);
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
        // Every builtin takes one (possibly tuple) argument; the compiler
        // always emits `Call 1`, but bytecode may use another arity.
        if arity != 1 {
            return Err(RuntimeError {
                message: format!("builtin expects 1 arg, got {arity}"),
            });
        }
        let first_arg = self.stack[callee_pos + 1];

        if self.builtins.ids.spawn_builtin_id == Some(builtin_id) {
            let closure_val = self.stack[callee_pos + 1];
            match closure_val {
                Value::Heap(r) => match self.heap.get(r) {
                    Ok(HeapObject::Closure {
                        proto_idx,
                        captures,
                    }) => {
                        let proto_idx = *proto_idx;
                        let captures = captures.clone();
                        self.refresh_spawn_snapshot()?;
                        let mut serialized = Vec::new();
                        for &v in captures.iter() {
                            self.heap
                                .charge_host_work(crate::sendable::SENDABLE_BOUNDARY_BASE_HOST_WORK)
                                .map_err(|e| RuntimeError {
                                    message: format!("spawn: {e}"),
                                })?;
                            // A value already sent to an earlier child is
                            // reused, and its serialization is not charged again.
                            let (sendable, fresh) =
                                crate::sendable::serialize_shared(v, &mut self.heap).map_err(
                                    |e| RuntimeError {
                                        message: format!("spawn: {e}"),
                                    },
                                )?;
                            if fresh {
                                self.heap
                                    .charge_host_work(crate::sendable::host_work_for_sendable(
                                        &sendable,
                                    ))
                                    .map_err(|e| RuntimeError {
                                        message: format!("spawn: {e}"),
                                    })?;
                            }
                            serialized.push(sendable);
                        }
                        return self.suspend_for_runtime_request(
                            RuntimeRequest::Spawn {
                                proto_idx,
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

        if self.builtins.ids.await_builtin_id == Some(builtin_id) {
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

        if self.builtins.ids.await_result_builtin_id == Some(builtin_id) {
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

        if self.builtins.ids.cancel_builtin_id == Some(builtin_id) {
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

        if self.builtins.ids.wait_any_builtin_id == Some(builtin_id) {
            let pids = crate::builtins::extract_pid_list_arg(
                &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                &self.heap,
                "wait_any",
            )
            .map_err(|message| RuntimeError { message })?;
            return self.suspend_for_runtime_request(RuntimeRequest::WaitAny(pids), callee_pos);
        }

        if self.async_io {
            let io_request = if self.builtins.ids.sleep_builtin_id == Some(builtin_id) {
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
            } else if self.builtins.ids.http_get_builtin_id == Some(builtin_id) {
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
                Some(crate::io_backend::IoRequest::HttpGet {
                    url,
                    max_response: self.heap.http_response_limit(),
                    timeouts: self.heap.io_limits().http_timeouts(),
                })
            } else if let Some(format) = self.match_http_builtin(builtin_id) {
                let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
                let (method, url, headers, body) =
                    crate::builtins::extract_http_args(args, &self.heap, "http")
                        .map_err(|msg| RuntimeError { message: msg })?;
                let builtin_name = self.builtins.entries[builtin_id as usize].name.as_ref();
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
                    max_response: self.heap.http_response_limit(),
                    timeouts: self.heap.io_limits().http_timeouts(),
                })
            } else if self.builtins.ids.read_file_builtin_id == Some(builtin_id) {
                let path = crate::builtins::extract_string_arg(
                    &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                    &self.heap,
                    "read_file",
                )
                .map_err(|msg| RuntimeError { message: msg })?;
                #[cfg(feature = "builtin-filesystem")]
                {
                    if self.heap.has_cap_fs_policy() {
                        let candidates =
                            self.heap
                                .cap_candidates_for("read_file", &path)
                                .map_err(|e| RuntimeError {
                                    message: format!("read_file: {e}"),
                                })?;
                        Some(crate::io_backend::IoRequest::CapReadFile {
                            candidates,
                            path,
                            max_bytes: self.heap.file_read_limit(),
                        })
                    } else {
                        Some(crate::io_backend::IoRequest::ReadFile {
                            path,
                            max_bytes: self.heap.file_read_limit(),
                        })
                    }
                }
                #[cfg(not(feature = "builtin-filesystem"))]
                {
                    let _ = path;
                    None
                }
            } else {
                #[cfg(feature = "builtin-aws-config")]
                {
                    if self.builtins.ids.aws_config_sso_profile_builtin_id == Some(builtin_id) {
                        let profile = crate::builtins::extract_string_arg(
                            &self.stack[callee_pos + 1..callee_pos + 1 + arity],
                            &self.heap,
                            "aws_config_sso_profile",
                        )
                        .map_err(|msg| RuntimeError { message: msg })?;
                        self.heap
                            .check_aws_sso_profile(&profile)
                            .map_err(|e| RuntimeError {
                                message: format!("aws_config_sso_profile: {e}"),
                            })?;
                        Some(crate::io_backend::IoRequest::AwsConfigSsoProfile { profile })
                    } else if self.builtins.ids.aws_config_instance_profile_builtin_id
                        == Some(builtin_id)
                    {
                        self.heap
                            .check_aws_instance_profile()
                            .map_err(|e| RuntimeError {
                                message: format!("aws_config_instance_profile: {e}"),
                            })?;
                        Some(crate::io_backend::IoRequest::AwsConfigInstanceProfile)
                    } else {
                        #[cfg(feature = "builtin-aws-s3")]
                        {
                            if self.builtins.ids.aws_s3_list_buckets_builtin_id == Some(builtin_id)
                            {
                                let client = self.stack[callee_pos + 1];
                                let handle = self
                                    .heap
                                    .aws_s3_client_handle_from_value(client, "aws_s3_list_buckets")
                                    .map_err(|message| RuntimeError { message })?;
                                Some(crate::io_backend::IoRequest::AwsS3ListBuckets {
                                    client: handle.client.clone(),
                                })
                            } else {
                                #[cfg(feature = "builtin-aws-sqs")]
                                {
                                    if self.builtins.ids.aws_sqs_list_queues_builtin_id
                                        == Some(builtin_id)
                                    {
                                        let client = self.stack[callee_pos + 1];
                                        let handle = self
                                            .heap
                                            .aws_sqs_client_handle_from_value(
                                                client,
                                                "aws_sqs_list_queues",
                                            )
                                            .map_err(|message| RuntimeError { message })?;
                                        Some(crate::io_backend::IoRequest::AwsSqsListQueues {
                                            client: handle.client.clone(),
                                        })
                                    } else {
                                        None
                                    }
                                }
                                #[cfg(not(feature = "builtin-aws-sqs"))]
                                {
                                    None
                                }
                            }
                        }
                        #[cfg(all(not(feature = "builtin-aws-s3"), feature = "builtin-aws-sqs"))]
                        {
                            if self.builtins.ids.aws_sqs_list_queues_builtin_id == Some(builtin_id)
                            {
                                let client = self.stack[callee_pos + 1];
                                let handle = self
                                    .heap
                                    .aws_sqs_client_handle_from_value(client, "aws_sqs_list_queues")
                                    .map_err(|message| RuntimeError { message })?;
                                Some(crate::io_backend::IoRequest::AwsSqsListQueues {
                                    client: handle.client.clone(),
                                })
                            } else {
                                None
                            }
                        }
                        #[cfg(all(
                            not(feature = "builtin-aws-s3"),
                            not(feature = "builtin-aws-sqs")
                        ))]
                        {
                            None
                        }
                    }
                }
                #[cfg(not(feature = "builtin-aws-config"))]
                {
                    None
                }
            };
            if let Some(request) = io_request {
                return self.suspend_for_runtime_request(RuntimeRequest::Io(request), callee_pos);
            }
        }

        if self.builtins.ids.exec_builtin_id == Some(builtin_id) {
            let exec_arg = self.stack[callee_pos + 1];
            let prepared = self.prepare_exec(exec_arg)?;
            let result = self
                .run_exec(prepared)
                .map_err(|msg| RuntimeError { message: msg })?;
            self.stack.truncate(callee_pos);
            self.push(result)?;
            return Ok(());
        }

        let func = self.builtins.entries[builtin_id as usize].func;
        let args = &self.stack[callee_pos + 1..callee_pos + 1 + arity];
        let result = func(args, &mut self.heap).map_err(|msg| RuntimeError { message: msg })?;
        self.stack.truncate(callee_pos);
        let is_print = self.builtins.ids.print_builtin_id == Some(builtin_id);
        let is_println = self.builtins.ids.println_builtin_id == Some(builtin_id);
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
        if self.builtins.ids.http_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Text)
        } else if self.builtins.ids.http_json_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Json)
        } else if self.builtins.ids.http_msgpack_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Msgpack)
        } else if self.builtins.ids.http_bytes_builtin_id == Some(builtin_id) {
            Some(HttpResponseFormat::Bytes)
        } else {
            None
        }
    }
}
