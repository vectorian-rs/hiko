//! AWS config, S3 and SQS builtins for Hiko, as an extension.
//!
//! The run config turns the extension on and says which credentials and
//! services a script may use:
//!
//! ```toml
//! [extensions.aws]
//! sso_profiles = ["dev"]     # profiles Aws.Config.sso_profile may name
//! instance_profile = false   # whether Aws.Config.instance_profile works
//! s3 = true                  # whether S3 clients may be created
//! sqs = false
//! ```
//!
//! Everything defaults to off. Configs and clients are host resources
//! (kinds `aws_config`, `aws_s3_client`, `aws_sqs_client`), so
//! `max_host_resources` and `[limits.host_resources]` cap them.
//!
//! Creating a config or a client only checks the policy and records the
//! choice; it does no I/O. A listing runs as an async job on the I/O pool,
//! which loads the SDK config on first use and keeps the SDK client for
//! later calls through the same handle.

use std::sync::{Arc, OnceLock};

use hiko_builtin_meta::BuiltinTypeSignature;
use hiko_vm::extension::{
    AsyncBuiltinFn, AsyncJob, Extension, ExtensionBuiltin, ExtensionFn, ExtensionState,
};
use hiko_vm::heap::Heap;
use hiko_vm::sendable::SendableValue;
use hiko_vm::value::{BuiltinFn, HeapObject, Value};
use serde::Deserialize;

const CONFIG: &str = "aws_config";
const S3_CLIENT: &str = "aws_s3_client";
const SQS_CLIENT: &str = "aws_sqs_client";

/// The extension, to link into a host and pass to
/// `RunConfig::build_vm_with_extensions`.
pub static AWS: Extension = Extension {
    name: "aws",
    types: &[CONFIG, S3_CLIENT, SQS_CLIENT],
    builtins: &[
        sync(
            "aws_config_sso_profile",
            "string -> aws_config",
            sso_profile,
        ),
        sync(
            "aws_config_instance_profile",
            "unit -> aws_config",
            instance_profile,
        ),
        sync("aws_s3_client", "aws_config -> aws_s3_client", s3_client),
        asynchronous(
            "aws_s3_list_buckets",
            "aws_s3_client -> bool * (string * string * string) list * string",
            s3_list_buckets,
        ),
        sync("aws_sqs_client", "aws_config -> aws_sqs_client", sqs_client),
        asynchronous(
            "aws_sqs_list_queues",
            "aws_sqs_client -> bool * string list * string",
            sqs_list_queues,
        ),
    ],
    modules: &[
        ("Aws.Config", include_str!("Config.hml")),
        ("Aws.S3", include_str!("S3.hml")),
        ("Aws.SQS", include_str!("SQS.hml")),
    ],
    configure,
};

const fn sync(name: &'static str, ty: &'static str, func: BuiltinFn) -> ExtensionBuiltin {
    ExtensionBuiltin {
        name,
        signature: BuiltinTypeSignature { vars: &[], ty },
        func: ExtensionFn::Sync(func),
    }
}

const fn asynchronous(
    name: &'static str,
    ty: &'static str,
    func: AsyncBuiltinFn,
) -> ExtensionBuiltin {
    ExtensionBuiltin {
        name,
        signature: BuiltinTypeSignature { vars: &[], ty },
        func: ExtensionFn::Async(func),
    }
}

/// `[extensions.aws]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    #[serde(default)]
    sso_profiles: Vec<String>,
    #[serde(default)]
    instance_profile: bool,
    #[serde(default)]
    s3: bool,
    #[serde(default)]
    sqs: bool,
}

fn configure(table: &toml::Value) -> Result<ExtensionState, String> {
    let policy: Policy = table.clone().try_into().map_err(|e| e.to_string())?;
    Ok(Arc::new(policy))
}

fn policy(heap: &Heap, builtin: &str) -> Result<Arc<Policy>, String> {
    heap.extension_state::<Policy>("aws")
        .ok_or_else(|| format!("{builtin}: the aws extension is not configured"))
}

/// Where a config's credentials come from.
#[derive(Debug, Clone)]
enum Auth {
    SsoProfile(String),
    InstanceProfile,
}

/// A service client: the auth it was made from, and the SDK client, built
/// on the first call.
struct Client<C> {
    auth: Auth,
    sdk: OnceLock<C>,
}

fn sso_profile(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "aws_config_sso_profile";
    let profile = match args[0] {
        Value::Heap(r) => match heap.get(r).map_err(|e| e.to_string())? {
            HeapObject::String(s) => s.to_string(),
            _ => return Err(format!("{name}: expected a string")),
        },
        _ => return Err(format!("{name}: expected a string")),
    };
    let policy = policy(heap, name)?;
    if !policy.sso_profiles.contains(&profile) {
        return Err(format!(
            "{name}: profile '{profile}' is not in sso_profiles {:?}",
            policy.sso_profiles
        ));
    }
    heap.alloc_extension_resource(CONFIG, Arc::new(Auth::SsoProfile(profile)))
}

fn instance_profile(_args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    let name = "aws_config_instance_profile";
    if !policy(heap, name)?.instance_profile {
        return Err(format!("{name}: instance_profile is not enabled"));
    }
    heap.alloc_extension_resource(CONFIG, Arc::new(Auth::InstanceProfile))
}

/// A client handle of `kind` for the config in `args[0]`, if `enabled`.
fn client<C: Send + Sync + 'static>(
    args: &[Value],
    heap: &mut Heap,
    name: &str,
    kind: &'static str,
    enabled: impl Fn(&Policy) -> bool,
) -> Result<Value, String> {
    if !enabled(&*policy(heap, name)?) {
        return Err(format!("{name}: the service is not enabled"));
    }
    let auth = heap.extension_resource::<Auth>(args[0], CONFIG, name)?;
    heap.alloc_extension_resource(
        kind,
        Arc::new(Client::<C> {
            auth: (*auth).clone(),
            sdk: OnceLock::new(),
        }),
    )
}

fn s3_client(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    client::<aws_sdk_s3::Client>(args, heap, "aws_s3_client", S3_CLIENT, |p| p.s3)
}

fn sqs_client(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    client::<aws_sdk_sqs::Client>(args, heap, "aws_sqs_client", SQS_CLIENT, |p| p.sqs)
}

/// Run `f` on a current-thread Tokio runtime, on the I/O pool thread.
fn block_on<T>(
    name: &str,
    f: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("{name}: tokio runtime: {e}"))?
        .block_on(f)
}

/// The instance metadata service. Fixed, so `AWS_EC2_METADATA_SERVICE_ENDPOINT`
/// and the profile setting that moves it are not used.
const IMDS_ENDPOINT: &str = "http://169.254.169.254";

/// The SDK config for `auth`. Credentials and region each come from exactly
/// one source, never from the SDK's default chains (which try environment
/// variables, web identity, ECS and the instance metadata service in turn):
///
/// - `SsoProfile`: the named profile in the AWS config and credentials
///   files. A profile that does not exist or sets no region is an error.
///   What the profile itself says (SSO, `role_arn` with `source_profile` or
///   `credential_source`, `credential_process`) applies; an assumed role's
///   STS call uses the profile's region.
/// - `InstanceProfile`: the instance metadata service at `IMDS_ENDPOINT`.
///
/// Endpoint URLs are cleared on each service client by `no_endpoint_override`.
/// FIPS, dual-stack, retry and timeout settings still come from the
/// environment and the profile, as the SDK reads them.
async fn load_sdk_config(auth: &Auth) -> Result<aws_config::SdkConfig, String> {
    use aws_config::imds::{
        self, credentials::ImdsCredentialsProvider, region::ImdsRegionProvider,
    };
    use aws_config::meta::region::ProvideRegion;
    use aws_config::profile::{ProfileFileCredentialsProvider, ProfileFileRegionProvider};
    use aws_config::provider_config::ProviderConfig;
    let base = ProviderConfig::without_region();
    let loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    match auth {
        Auth::SsoProfile(profile) => {
            let region = ProfileFileRegionProvider::builder()
                .configure(&base)
                .profile_name(profile)
                .build()
                .region()
                .await
                .ok_or_else(|| {
                    format!("AWS profile '{profile}' does not exist or sets no region")
                })?;
            let credentials = ProfileFileCredentialsProvider::builder()
                .configure(&base.with_region(Some(region.clone())))
                .profile_name(profile)
                .build();
            Ok(loader
                .profile_name(profile)
                .region(region)
                .credentials_provider(credentials)
                .load()
                .await)
        }
        Auth::InstanceProfile => {
            let client = imds::Client::builder()
                .endpoint(IMDS_ENDPOINT)
                .map_err(|e| format!("instance metadata endpoint: {e}"))?
                .build();
            let region = ImdsRegionProvider::builder()
                .configure(&base)
                .imds_client(client.clone())
                .build()
                .region()
                .await
                .ok_or("the instance metadata service gave no region")?;
            let credentials = ImdsCredentialsProvider::builder()
                .configure(&base.with_region(Some(region.clone())))
                .imds_client(client)
                .build();
            Ok(loader
                .region(region)
                .credentials_provider(credentials)
                .load()
                .await)
        }
    }
}

/// Clear the endpoint URL a service config takes from `AWS_ENDPOINT_URL`,
/// `AWS_ENDPOINT_URL_<SERVICE>` or the profile: requests go to the
/// service's own endpoint for the region.
macro_rules! no_endpoint_override {
    ($builder:expr) => {{
        let mut builder = $builder;
        builder.set_endpoint_url(None);
        builder.build()
    }};
}

/// Resolve the access key ID a client made from `sso_profile profile`
/// (`Some`) or `instance_profile ()` (`None`) would use, without the
/// policy check. For `tests/credentials.rs` only.
#[cfg(feature = "test-support")]
pub fn resolve_access_key_id(profile: Option<&str>) -> Result<String, String> {
    use aws_sdk_s3::config::ProvideCredentials;
    let auth = match profile {
        Some(profile) => Auth::SsoProfile(profile.to_string()),
        None => Auth::InstanceProfile,
    };
    block_on("resolve_access_key_id", async {
        let config = load_sdk_config(&auth).await?;
        let provider = config
            .credentials_provider()
            .ok_or("no credentials provider")?;
        provider
            .provide_credentials()
            .await
            .map(|c| c.access_key_id().to_string())
            .map_err(|e| format!("{e:?}"))
    })
}

/// `(ok, payload, error)`, with `empty` as the payload on failure.
fn outcome(result: Result<SendableValue, String>, empty: SendableValue) -> SendableValue {
    let (ok, payload, error) = match result {
        Ok(payload) => (true, payload, String::new()),
        Err(error) => (false, empty, error),
    };
    SendableValue::Tuple(vec![
        SendableValue::Bool(ok),
        payload,
        SendableValue::String(error.into()),
    ])
}

fn s3_list_buckets(args: &[Value], heap: &mut Heap) -> Result<AsyncJob, String> {
    let name = "aws_s3_list_buckets";
    let client = heap.extension_resource::<Client<aws_sdk_s3::Client>>(args[0], S3_CLIENT, name)?;
    Ok(Box::new(move || {
        let result = block_on(name, async {
            if client.sdk.get().is_none() {
                let config = load_sdk_config(&client.auth).await?;
                let conf = no_endpoint_override!(aws_sdk_s3::config::Builder::from(&config));
                let _ = client.sdk.set(aws_sdk_s3::Client::from_conf(conf));
            }
            let output = client
                .sdk
                .get()
                .expect("set above")
                .list_buckets()
                .send()
                .await;
            let output = output.map_err(|e| format_aws_error(name, &e))?;
            let buckets = output
                .buckets()
                .iter()
                .map(|bucket| {
                    SendableValue::Tuple(vec![
                        string(bucket.name().unwrap_or("")),
                        string(
                            &bucket
                                .creation_date()
                                .map(|d| d.to_string())
                                .unwrap_or_default(),
                        ),
                        string(bucket.bucket_arn().unwrap_or("")),
                    ])
                })
                .collect();
            Ok(SendableValue::List(buckets))
        });
        Ok(outcome(result, SendableValue::List(Vec::new())))
    }))
}

fn sqs_list_queues(args: &[Value], heap: &mut Heap) -> Result<AsyncJob, String> {
    let name = "aws_sqs_list_queues";
    let client =
        heap.extension_resource::<Client<aws_sdk_sqs::Client>>(args[0], SQS_CLIENT, name)?;
    Ok(Box::new(move || {
        let result = block_on(name, async {
            if client.sdk.get().is_none() {
                let config = load_sdk_config(&client.auth).await?;
                let conf = no_endpoint_override!(aws_sdk_sqs::config::Builder::from(&config));
                let _ = client.sdk.set(aws_sdk_sqs::Client::from_conf(conf));
            }
            let output = client
                .sdk
                .get()
                .expect("set above")
                .list_queues()
                .send()
                .await;
            let output = output.map_err(|e| format_aws_error(name, &e))?;
            Ok(SendableValue::List(
                output.queue_urls().iter().map(|url| string(url)).collect(),
            ))
        });
        Ok(outcome(result, SendableValue::List(Vec::new())))
    }))
}

fn string(s: &str) -> SendableValue {
    SendableValue::String(s.into())
}

/// An SDK error with its distinct causes, up to 16 deep.
fn format_aws_error(context: &str, err: &dyn std::error::Error) -> String {
    const MAX_SOURCES: usize = 16;
    let mut message = format!("{context}: {err}");
    let mut seen = vec![err.to_string()];
    let mut source = err.source();
    let mut causes = Vec::new();
    let mut depth = 0;
    while let Some(cause) = source {
        if depth == MAX_SOURCES {
            causes.push("...".to_string());
            break;
        }
        let text = cause.to_string();
        if !text.is_empty() && !seen.contains(&text) {
            seen.push(text.clone());
            causes.push(text);
        }
        source = cause.source();
        depth += 1;
    }
    if !causes.is_empty() {
        message.push_str("; caused by: ");
        message.push_str(&causes.join("; "));
    }
    message
}
