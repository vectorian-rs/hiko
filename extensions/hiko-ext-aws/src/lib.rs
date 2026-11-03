//! AWS config, S3, SQS and CloudWatch builtins for Hiko, as an extension.
//!
//! The run config turns the extension on and says which credentials and
//! services a script may use:
//!
//! ```toml
//! [extensions.aws]
//! sso_profiles = ["dev"]     # profiles Aws.Config.sso_profile may name
//! s3 = true                  # whether S3 clients may be created
//! sqs = false
//! cloudwatch = false         # whether CloudWatch clients may be created
//! ```
//!
//! Everything defaults to off. Configs and clients are host resources
//! (kinds `aws_config`, `aws_s3_client`, `aws_sqs_client`,
//! `aws_cloudwatch_client`), so
//! `max_host_resources` and `[limits.host_resources]` cap them.
//!
//! Creating a config or a client only checks the policy and records the
//! choice; it does no I/O. A listing runs as an async job on the I/O pool,
//! which loads the SDK config on first use and keeps the SDK client for
//! later calls through the same handle.

use std::path::{Path, PathBuf};
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
const CLOUDWATCH_CLIENT: &str = "aws_cloudwatch_client";

/// The extension, to link into a host and pass to
/// `RunConfig::build_vm_with_extensions`.
pub static AWS: Extension = Extension {
    name: "aws",
    types: &[CONFIG, S3_CLIENT, SQS_CLIENT, CLOUDWATCH_CLIENT],
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
        sync(
            "aws_cloudwatch_client",
            "aws_config -> aws_cloudwatch_client",
            cloudwatch_client,
        ),
        asynchronous(
            "aws_cloudwatch_describe_alarms",
            "aws_cloudwatch_client -> bool * (string * string * string * string * string) list * string",
            cloudwatch_describe_alarms,
        ),
    ],
    modules: &[
        ("Aws.Config", include_str!("Config.hml")),
        ("Aws.S3", include_str!("S3.hml")),
        ("Aws.SQS", include_str!("SQS.hml")),
        ("Aws.CloudWatch", include_str!("CloudWatch.hml")),
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
    #[serde(default)]
    cloudwatch: bool,
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

fn cloudwatch_client(args: &[Value], heap: &mut Heap) -> Result<Value, String> {
    client::<aws_sdk_cloudwatch::Client>(
        args,
        heap,
        "aws_cloudwatch_client",
        CLOUDWATCH_CLIENT,
        |p| p.cloudwatch,
    )
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
    "login_session",
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
            // One portal, named once: the session's.
            if let Some(key) = ["sso_start_url", "sso_region"]
                .into_iter()
                .find(|key| profile.get(key).is_some())
            {
                return Err(format!(
                    "AWS profile '{name}' sets both sso_session and {key}; set {key} in the sso-session section only"
                ));
            }
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

/// The user's home directory, from `HOME` (`USERPROFILE` on Windows): the
/// one input from the environment, for the AWS files and the SSO token
/// cache under it.
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find the AWS files: HOME is not set".to_string())
}

/// `~/.aws/config` and `~/.aws/credentials`, and nothing else:
/// `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` are not used.
fn aws_files(home: &Path) -> aws_runtime::env_config::file::EnvConfigFiles {
    use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};
    EnvConfigFiles::builder()
        .include_default_config_file(false)
        .include_default_credentials_file(false)
        .with_file(EnvConfigFileKind::Config, home.join(".aws").join("config"))
        .with_file(
            EnvConfigFileKind::Credentials,
            home.join(".aws").join("credentials"),
        )
        .build()
}

/// Role credentials for an SSO profile: the cached SSO token from
/// `~/.aws/sso/cache`, exchanged with the SSO portal's GetRoleCredentials.
/// The SSO client is built from the profile's `sso_region` alone, so no
/// environment variable or profile setting can move the portal endpoint
/// (the SDK's own SSO provider takes `AWS_ENDPOINT_URL_SSO` from the
/// environment). The token is never refreshed here: an expired one is an
/// error asking for `aws sso login`.
#[derive(Debug)]
struct SsoRole {
    profile: String,
    sso: SsoProfile,
    home: PathBuf,
    /// The SSO portal's endpoint: `None`, AWS's own for the region, except
    /// in tests, which point it at a local server.
    endpoint: Option<String>,
}

impl SsoRole {
    fn cached_token(&self) -> Result<String, String> {
        use sha1::{Digest, Sha1};
        let login = format!("run 'aws sso login --profile {}'", self.profile);
        let id = self.sso.session.as_deref().unwrap_or(&self.sso.start_url);
        let hash: String = Sha1::digest(id.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self
            .home
            .join(".aws")
            .join("sso")
            .join("cache")
            .join(format!("{hash}.json"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "no SSO token for AWS profile '{}' ({}: {e}); {login}",
                self.profile,
                path.display()
            )
        })?;
        #[derive(Deserialize)]
        struct Cached {
            #[serde(rename = "accessToken")]
            access_token: String,
            #[serde(rename = "expiresAt")]
            expires_at: String,
        }
        let cached: Cached = serde_json::from_str(&text)
            .map_err(|e| format!("invalid SSO token cache {}: {e}", path.display()))?;
        let expires = aws_sdk_s3::primitives::DateTime::from_str(
            &cached.expires_at,
            aws_sdk_s3::primitives::DateTimeFormat::DateTime,
        )
        .map_err(|e| format!("invalid expiresAt in {}: {e}", path.display()))?;
        if std::time::SystemTime::try_from(expires)
            .is_ok_and(|expires| expires <= std::time::SystemTime::now())
        {
            return Err(format!(
                "the SSO token for AWS profile '{}' expired at {}; {login}",
                self.profile, cached.expires_at
            ));
        }
        Ok(cached.access_token)
    }

    async fn credentials(
        &self,
    ) -> Result<
        aws_credential_types::Credentials,
        aws_credential_types::provider::error::CredentialsError,
    > {
        use aws_credential_types::provider::error::CredentialsError;
        let token = self.cached_token().map_err(CredentialsError::not_loaded)?;
        let mut config = aws_sdk_sso::Config::builder()
            .behavior_version(aws_sdk_sso::config::BehaviorVersion::latest())
            .region(aws_sdk_sso::config::Region::new(
                self.sso.sso_region.clone(),
            ));
        if let Some(endpoint) = &self.endpoint {
            config = config.endpoint_url(endpoint);
        }
        let output = aws_sdk_sso::Client::from_conf(config.build())
            .get_role_credentials()
            .account_id(&self.sso.account_id)
            .role_name(&self.sso.role_name)
            .access_token(token)
            .send()
            .await
            .map_err(|e| {
                CredentialsError::provider_error(format_aws_error(
                    &format!("SSO GetRoleCredentials for AWS profile '{}'", self.profile),
                    &e,
                ))
            })?;
        let missing = |field: &str| {
            CredentialsError::provider_error(format!(
                "SSO GetRoleCredentials for AWS profile '{}' returned no {field}",
                self.profile
            ))
        };
        let role = output
            .role_credentials()
            .ok_or_else(|| missing("credentials"))?;
        let expiration = u64::try_from(role.expiration()).map_err(|_| missing("expiration"))?;
        Ok(aws_credential_types::Credentials::new(
            role.access_key_id().ok_or_else(|| missing("access key"))?,
            role.secret_access_key()
                .ok_or_else(|| missing("secret key"))?,
            role.session_token().map(str::to_string),
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(expiration)),
            "hiko-ext-aws-sso",
        ))
    }
}

impl aws_credential_types::provider::ProvideCredentials for SsoRole {
    fn provide_credentials<'a>(
        &'a self,
    ) -> aws_credential_types::provider::future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        aws_credential_types::provider::future::ProvideCredentials::new(self.credentials())
    }
}

/// The SDK config for `auth`. Credentials come only from the profile's SSO
/// token, through `SsoRole`, and the region only from the profile. The
/// SDK's default chains (environment variables, the profile's other
/// credential settings, web identity, ECS, the instance metadata service)
/// are never used, and a profile that is not a direct SSO profile is an
/// error.
///
/// Endpoint URLs are cleared on each service client by `no_endpoint_override`.
/// FIPS, dual-stack, retry and timeout settings still come from the
/// environment and the profile, as the SDK reads them.
async fn load_sdk_config(auth: &Auth) -> Result<aws_config::SdkConfig, String> {
    let home = home_dir()?;
    let files = aws_files(&home);
    let profiles = aws_config::profile::load(
        &aws_types::os_shim_internal::Fs::real(),
        &aws_types::os_shim_internal::Env::real(),
        &files,
        None,
    )
    .await
    .map_err(|e| format!("cannot read the AWS config files: {e}"))?;
    let sso = sso_profile_settings(&profiles, &auth.profile)?;
    let region = aws_config::Region::new(sso.region.clone());
    let role = SsoRole {
        profile: auth.profile.clone(),
        sso,
        home,
        endpoint: None,
    };
    Ok(aws_config::defaults(aws_config::BehaviorVersion::latest())
        .profile_files(files)
        .profile_name(&auth.profile)
        .region(region)
        .credentials_provider(role)
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

/// Every metric and composite alarm, all pages, as alarm rows.
fn cloudwatch_describe_alarms(args: &[Value], heap: &mut Heap) -> Result<AsyncJob, String> {
    use aws_sdk_cloudwatch::types::AlarmType;
    let name = "aws_cloudwatch_describe_alarms";
    let client = heap.extension_resource::<Client<aws_sdk_cloudwatch::Client>>(
        args[0],
        CLOUDWATCH_CLIENT,
        name,
    )?;
    Ok(Box::new(move || {
        let result = block_on(name, async {
            if client.sdk.get().is_none() {
                let config = load_sdk_config(&client.auth).await?;
                let conf =
                    no_endpoint_override!(aws_sdk_cloudwatch::config::Builder::from(&config));
                let _ = client.sdk.set(aws_sdk_cloudwatch::Client::from_conf(conf));
            }
            let pages = client
                .sdk
                .get()
                .expect("set above")
                .describe_alarms()
                .alarm_types(AlarmType::MetricAlarm)
                .alarm_types(AlarmType::CompositeAlarm)
                .into_paginator()
                .send()
                .try_collect()
                .await
                .map_err(|e| format_aws_error(name, &e))?;
            let mut alarms = Vec::new();
            for page in &pages {
                alarms.extend(page.metric_alarms().iter().map(metric_alarm_row));
                alarms.extend(page.composite_alarms().iter().map(composite_alarm_row));
            }
            Ok(SendableValue::List(alarms))
        });
        Ok(outcome(result, SendableValue::List(Vec::new())))
    }))
}

/// `(name, state, source, updated, reason)`, as `Aws.CloudWatch.alarm`
/// documents it.
fn alarm_row(
    name: Option<&str>,
    state: Option<&aws_sdk_cloudwatch::types::StateValue>,
    source: &str,
    updated: Option<&aws_sdk_cloudwatch::primitives::DateTime>,
    reason: Option<&str>,
) -> SendableValue {
    SendableValue::Tuple(vec![
        string(name.unwrap_or("")),
        string(state.map(|s| s.as_str()).unwrap_or("")),
        string(source),
        // Whole seconds: CloudWatch's fractions vary in length.
        string(
            &updated
                .map(|d| aws_sdk_cloudwatch::primitives::DateTime::from_secs(d.secs()).to_string())
                .unwrap_or_default(),
        ),
        string(reason.unwrap_or("")),
    ])
}

/// A metric alarm's source is `Namespace/MetricName`, or `metric math`
/// for an alarm on an expression.
fn metric_alarm_row(alarm: &aws_sdk_cloudwatch::types::MetricAlarm) -> SendableValue {
    let source = match (alarm.namespace(), alarm.metric_name()) {
        (Some(namespace), Some(metric)) => format!("{namespace}/{metric}"),
        _ => "metric math".to_string(),
    };
    alarm_row(
        alarm.alarm_name(),
        alarm.state_value(),
        &source,
        alarm.state_updated_timestamp(),
        alarm.state_reason(),
    )
}

fn composite_alarm_row(alarm: &aws_sdk_cloudwatch::types::CompositeAlarm) -> SendableValue {
    alarm_row(
        alarm.alarm_name(),
        alarm.state_value(),
        "composite",
        alarm.state_updated_timestamp(),
        alarm.state_reason(),
    )
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

    #[test]
    fn a_session_profile_names_its_portal_once() {
        let config = SSO.replacen(
            "sso_session = corp\n",
            "sso_session = corp\nsso_start_url = https://other.awsapps.com/start\n",
            1,
        );
        assert_eq!(
            sso_profile_settings(&profiles(&config), "dev").unwrap_err(),
            "AWS profile 'dev' sets both sso_session and sso_start_url; set sso_start_url in the sso-session section only"
        );
        let config = SSO.replacen(
            "sso_session = corp\n",
            "sso_session = corp\nlogin_session = x\n",
            1,
        );
        let err = sso_profile_settings(&profiles(&config), "dev").unwrap_err();
        assert!(
            err.contains("AWS profile 'dev' sets login_session"),
            "{err}"
        );
    }

    /// An `SsoRole` for the profile in `SSO`, with its home in a new
    /// temporary directory holding `token` as the cached SSO token, if any.
    fn role(name: &str, token: Option<&str>, endpoint: Option<String>) -> SsoRole {
        use sha1::{Digest, Sha1};
        let home = std::env::temp_dir().join(format!("hiko-ext-aws-{name}-{}", std::process::id()));
        let cache = home.join(".aws").join("sso").join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let hash: String = Sha1::digest(b"corp")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let file = cache.join(format!("{hash}.json"));
        match token {
            Some(token) => std::fs::write(&file, token).unwrap(),
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
        SsoRole {
            profile: "dev".into(),
            sso: sso_profile_settings(&profiles(SSO), "dev").unwrap(),
            home,
            endpoint,
        }
    }

    fn credentials(role: &SsoRole) -> Result<aws_credential_types::Credentials, String> {
        block_on("credentials", async {
            role.credentials().await.map_err(|e| format!("{e:?}"))
        })
    }

    #[test]
    fn without_a_cached_token_the_error_says_to_log_in() {
        let err = credentials(&role("no-token", None, None)).unwrap_err();
        assert!(
            err.contains("no SSO token for AWS profile 'dev'")
                && err.contains("aws sso login --profile dev"),
            "{err}"
        );
    }

    #[test]
    fn an_expired_token_is_an_error_not_refreshed() {
        let token = r#"{"accessToken": "T", "expiresAt": "2020-01-01T00:00:00Z"}"#;
        let err = credentials(&role("expired", Some(token), None)).unwrap_err();
        assert!(
            err.contains("expired at 2020-01-01T00:00:00Z")
                && err.contains("aws sso login --profile dev"),
            "{err}"
        );
    }

    #[test]
    fn the_cached_token_is_exchanged_with_the_sso_portal() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                let line = line.unwrap();
                if line.is_empty() {
                    break;
                }
                request.push(line.to_ascii_lowercase());
            }
            let body = r#"{"roleCredentials":{"accessKeyId":"AKIDSSO","secretAccessKey":"s","sessionToken":"t","expiration":4102444800000}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        let token = r#"{"accessToken": "TOKEN", "expiresAt": "2099-01-01T00:00:00Z"}"#;
        let creds = credentials(&role("portal", Some(token), Some(endpoint))).unwrap();
        assert_eq!(creds.access_key_id(), "AKIDSSO");
        let request = server.join().unwrap();
        assert!(
            request[0].starts_with("get /federation/credentials?")
                && request[0].contains("account_id=111")
                && request[0].contains("role_name=dev"),
            "{request:?}"
        );
        assert!(
            request.iter().any(|h| h == "x-amz-sso_bearer_token: token"),
            "{request:?}"
        );
    }

    use aws_sdk_cloudwatch::primitives::DateTime;
    use aws_sdk_cloudwatch::types::{CompositeAlarm, MetricAlarm, StateValue};

    fn row(value: SendableValue) -> Vec<String> {
        let SendableValue::Tuple(fields) = &value else {
            panic!("an alarm row is a tuple: {value:?}");
        };
        fields
            .iter()
            .map(|field| match field {
                SendableValue::String(s) => s.to_string(),
                other => panic!("an alarm field is a string: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_metric_alarm_names_its_metric() {
        let alarm = MetricAlarm::builder()
            .alarm_name("api-5xx")
            .state_value(StateValue::Alarm)
            .namespace("AWS/ApplicationELB")
            .metric_name("HTTPCode_Target_5XX_Count")
            .state_updated_timestamp(DateTime::from_millis(1_793_700_000_875))
            .state_reason("Threshold Crossed: 1 datapoint [12.0] was greater than 5.0")
            .build();
        assert_eq!(
            row(metric_alarm_row(&alarm)),
            [
                "api-5xx",
                "ALARM",
                "AWS/ApplicationELB/HTTPCode_Target_5XX_Count",
                "2026-11-03T10:00:00Z",
                "Threshold Crossed: 1 datapoint [12.0] was greater than 5.0",
            ]
        );
    }

    #[test]
    fn a_metric_math_alarm_and_a_composite_alarm_say_so() {
        let math = MetricAlarm::builder()
            .alarm_name("error-rate")
            .state_value(StateValue::InsufficientData)
            .build();
        assert_eq!(
            row(metric_alarm_row(&math)),
            ["error-rate", "INSUFFICIENT_DATA", "metric math", "", ""]
        );
        let composite = CompositeAlarm::builder()
            .alarm_name("service-down")
            .state_value(StateValue::Ok)
            .build();
        assert_eq!(
            row(composite_alarm_row(&composite)),
            ["service-down", "OK", "composite", "", ""]
        );
    }
}
