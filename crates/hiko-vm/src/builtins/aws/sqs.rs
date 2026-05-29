use super::{BuiltinFn, Heap, Value};
use crate::value::{AwsSqsClientHandle, HostResource};
use std::sync::Arc;

pub(super) fn entries() -> Vec<(&'static str, BuiltinFn)> {
    [
        ("aws_sqs_client", client as BuiltinFn),
        ("aws_sqs_list_queues", list_queues as BuiltinFn),
    ]
    .to_vec()
}

pub(super) fn client(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let config_value = args
        .first()
        .copied()
        .ok_or_else(|| "aws_sqs_client: expected aws_config".to_string())?;
    let config = heap.aws_config_handle_from_value(config_value, "aws_sqs_client")?;
    let client = if config.sdk_config.region().is_some() {
        aws_sdk_sqs::Client::new(&config.sdk_config)
    } else {
        let conf = aws_sdk_sqs::config::Builder::from(config.sdk_config.as_ref())
            .region(aws_sdk_sqs::config::Region::new("us-east-1"))
            .build();
        aws_sdk_sqs::Client::from_conf(conf)
    };
    heap.alloc_host_resource(HostResource::AwsSqsClient(AwsSqsClientHandle {
        client: Arc::new(client),
    }))
    .map_err(|e| e.to_string())
}

pub(super) fn list_queues(_args: &[Value], _heap: &mut Heap) -> Result<Value, String> {
    Err("aws_sqs_list_queues: requires async I/O runtime".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{AwsConfigAuthMethod, AwsConfigHandle, HostHandleKind};

    #[test]
    fn client_creates_host_handle_from_config() {
        let mut heap = Heap::new();
        let cfg = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }))
            .unwrap();

        let sqs = client(&[cfg], &mut heap).unwrap();
        let id = heap
            .host_handle_from_value(sqs, HostHandleKind::AwsSqsClient, "aws_sqs_client", "test")
            .unwrap();
        assert!(
            heap.get_host_resource(id, HostHandleKind::AwsSqsClient, "aws_sqs_client", "test")
                .is_ok()
        );
    }

    #[test]
    fn client_rejects_wrong_host_handle_kind_cleanly() {
        let mut heap = Heap::new();
        let cfg = heap
            .alloc_host_resource(HostResource::AwsConfig(AwsConfigHandle {
                auth: AwsConfigAuthMethod::InstanceProfile,
                sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
            }))
            .unwrap();
        let sqs = client(&[cfg], &mut heap).unwrap();

        let err = client(&[sqs], &mut heap).unwrap_err();
        assert!(err.contains("expected aws_config"), "{err}");
    }
}
