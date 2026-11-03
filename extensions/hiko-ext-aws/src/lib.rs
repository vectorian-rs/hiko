//! AWS config, S3 and SQS builtins for Hiko, as an extension.
//!
//! The run config turns the extension on and says which credentials and
//! services a script may use:
//!
//! ```toml
//! [extensions.aws]
//! sso_profiles = ["dev"]     # profiles Aws.Config.sso_profile may name
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

/// Where a config's credentials come from: an AWS SSO profile, the only
/// source the extension supports.
#[derive(Debug, Clone)]
struct Auth {
    profile: String,
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
    heap.alloc_extension_resource(CONFIG, Arc::new(Auth { profile }))
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

/// Profile keys that make credentials come from anywhere but an SSO token.
/// A profile that sets one is refused, not used.
const NOT_SSO_KEYS: &[&str] = &[
    "aws_access_key_id",
    "aws_secret_access_key",
    "aws_session_token",
    "credential_process",
    "credential_source",
    "role_arn",
    "source_profile",
    "web_identity_token_file",
];

/// What an SSO profile says, read from the AWS config files.
#[derive(Debug, PartialEq)]
struct SsoProfile {
    account_id: String,
    role_name: String,
    start_url: String,
    sso_region: String,
    /// `None` for a legacy profile that sets `sso_start_url` itself.
    session: Option<String>,
    region: String,
}

/// Read profile `name` from `profiles` and check it is a direct SSO profile:
/// `sso_account_id`, `sso_role_name` and `region`, with `sso_session` (whose
/// section gives `sso_start_url` and `sso_region`) or the legacy
/// `sso_start_url` and `sso_region`, and none of `NOT_SSO_KEYS`.
fn sso_profile_settings(
    profiles: &aws_config::profile::ProfileSet,
    name: &str,
) -> Result<SsoProfile, String> {
    let profile = profiles
        .get_profile(name)
        .ok_or_else(|| format!("AWS profile '{name}' does not exist"))?;
    if let Some(key) = NOT_SSO_KEYS.iter().find(|key| profile.get(key).is_some()) {
        return Err(format!(
            "AWS profile '{name}' sets {key}; only SSO profiles are supported \
             (sso_session or sso_start_url, with sso_account_id and sso_role_name)"
        ));
    }
    let required = |key: &str| {
        profile
            .get(key)
            .map(str::to_string)
            .ok_or_else(|| format!("AWS profile '{name}' does not set {key}"))
    };
    let (start_url, sso_region, session) = match profile.get("sso_session") {
        Some(session) => {
            let section = profiles.sso_session(session).ok_or_else(|| {
                format!("AWS profile '{name}' names sso_session '{session}', which does not exist")
            })?;
            let get = |key: &str| {
                section
                    .get(key)
                    .map(str::to_string)
                    .ok_or_else(|| format!("AWS sso_session '{session}' does not set {key}"))
            };
            (
                get("sso_start_url")?,
                get("sso_region")?,
                Some(session.to_string()),
            )
        }
        None => (required("sso_start_url")?, required("sso_region")?, None),
    };
    Ok(SsoProfile {
        account_id: required("sso_account_id")?,
        role_name: required("sso_role_name")?,
        start_url,
        sso_region,
        session,
        region: required("region")?,
    })
}

/// The SDK config for `auth`. Credentials come only from the profile's SSO
/// token, through the SSO portal, and the region only from the profile;
/// the SDK's default chains (environment variables, the profile's other
/// credential settings, web identity, ECS, the instance metadata service)
/// are never used, and a profile that is not a direct SSO profile is an
/// error.
///
/// Endpoint URLs are cleared on each service client by `no_endpoint_override`.
/// FIPS, dual-stack, retry and timeout settings still come from the
/// environment and the profile, as the SDK reads them.
async fn load_sdk_config(auth: &Auth) -> Result<aws_config::SdkConfig, String> {
    use aws_config::Region;
    use aws_config::provider_config::ProviderConfig;
    use aws_config::sso::SsoCredentialsProvider;
    let profiles = aws_config::profile::load(
        &aws_types::os_shim_internal::Fs::real(),
        &aws_types::os_shim_internal::Env::real(),
        &Default::default(),
        None,
    )
    .await
    .map_err(|e| format!("cannot read the AWS config files: {e}"))?;
    let sso = sso_profile_settings(&profiles, &auth.profile)?;
    let region = Region::new(sso.region);
    let mut credentials = SsoCredentialsProvider::builder()
        .configure(&ProviderConfig::without_region().with_region(Some(region.clone())))
        .account_id(sso.account_id)
        .role_name(sso.role_name)
        .start_url(sso.start_url)
        .region(Region::new(sso.sso_region));
    credentials.set_session_name(sso.session);
    Ok(aws_config::defaults(aws_config::BehaviorVersion::latest())
        .profile_name(&auth.profile)
        .region(region)
        .credentials_provider(credentials.build())
        .load()
        .await)
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
/// would use, without the policy check. For `tests/credentials.rs` only.
#[cfg(feature = "test-support")]
pub fn resolve_access_key_id(profile: &str) -> Result<String, String> {
    use aws_sdk_s3::config::ProvideCredentials;
    let auth = Auth {
        profile: profile.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Profiles parsed from an AWS config file at `/home/.aws/config`.
    fn profiles(config: &str) -> aws_config::profile::ProfileSet {
        use aws_types::os_shim_internal::{Env, Fs};
        block_on("profiles", async {
            aws_config::profile::load(
                &Fs::from_slice(&[("/home/.aws/config", config)]),
                &Env::from_slice(&[("HOME", "/home")]),
                &Default::default(),
                None,
            )
            .await
            .map_err(|e| e.to_string())
        })
        .unwrap()
    }

    const SSO: &str = "[profile dev]\nsso_session = corp\nsso_account_id = 111\n\
        sso_role_name = Dev\nregion = eu-west-1\n\n[sso-session corp]\n\
        sso_start_url = https://corp.awsapps.com/start\nsso_region = us-east-1\n";

    #[test]
    fn an_sso_session_profile_is_read() {
        assert_eq!(
            sso_profile_settings(&profiles(SSO), "dev"),
            Ok(SsoProfile {
                account_id: "111".into(),
                role_name: "Dev".into(),
                start_url: "https://corp.awsapps.com/start".into(),
                sso_region: "us-east-1".into(),
                session: Some("corp".into()),
                region: "eu-west-1".into(),
            })
        );
    }

    #[test]
    fn a_legacy_sso_profile_is_read() {
        let config = "[profile dev]\nsso_start_url = https://corp.awsapps.com/start\n\
            sso_region = us-east-1\nsso_account_id = 111\nsso_role_name = Dev\nregion = eu-west-1\n";
        let sso = sso_profile_settings(&profiles(config), "dev").unwrap();
        assert_eq!(sso.session, None);
        assert_eq!(sso.start_url, "https://corp.awsapps.com/start");
    }

    #[test]
    fn only_direct_sso_profiles_are_accepted() {
        for (extra, key) in [
            (
                "aws_access_key_id = AKID\naws_secret_access_key = s\n",
                "aws_access_key_id",
            ),
            ("credential_process = /bin/creds\n", "credential_process"),
            (
                "role_arn = arn:aws:iam::1:role/r\nsource_profile = base\n",
                "role_arn",
            ),
            ("credential_source = Environment\n", "credential_source"),
            ("web_identity_token_file = /t\n", "web_identity_token_file"),
        ] {
            let config = SSO.replacen(
                "region = eu-west-1\n",
                &format!("region = eu-west-1\n{extra}"),
                1,
            );
            let err = sso_profile_settings(&profiles(&config), "dev").unwrap_err();
            assert!(
                err.contains(&format!("AWS profile 'dev' sets {key}")),
                "{err}"
            );
        }
    }

    #[test]
    fn missing_settings_are_errors_naming_them() {
        let p = |config: &str| sso_profile_settings(&profiles(config), "dev").unwrap_err();
        assert_eq!(
            sso_profile_settings(&profiles(SSO), "prod").unwrap_err(),
            "AWS profile 'prod' does not exist"
        );
        assert_eq!(
            p(&SSO.replace("region = eu-west-1\n", "")),
            "AWS profile 'dev' does not set region"
        );
        assert_eq!(
            p(&SSO.replace("sso_role_name = Dev\n", "")),
            "AWS profile 'dev' does not set sso_role_name"
        );
        assert_eq!(
            p(&SSO.replace("[sso-session corp]", "[sso-session other]")),
            "AWS profile 'dev' names sso_session 'corp', which does not exist"
        );
        assert_eq!(
            p(&SSO.replace("sso_region = us-east-1\n", "")),
            "AWS sso_session 'corp' does not set sso_region"
        );
        assert_eq!(
            p("[profile dev]\nsso_account_id = 1\nsso_role_name = r\nregion = x\n"),
            "AWS profile 'dev' does not set sso_start_url"
        );
    }
}
