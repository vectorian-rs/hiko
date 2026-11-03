//! Credentials come only from an SSO profile's token, never from the
//! environment or the profile's other credential settings.
//!
//! This file must hold exactly one test: it sets environment variables,
//! which is only sound while no other thread reads the environment, and
//! each file under tests/ is its own process.

use hiko_ext_aws::resolve_access_key_id;

#[test]
fn credentials_come_only_from_sso() {
    let home = std::env::temp_dir().join(format!("hiko-ext-aws-home-{}", std::process::id()));
    let aws = home.join(".aws");
    std::fs::create_dir_all(&aws).unwrap();
    std::fs::write(
        aws.join("config"),
        "[profile dev]\nsso_session = corp\nsso_account_id = 111\nsso_role_name = Dev\n\
         region = eu-west-1\n\n[sso-session corp]\n\
         sso_start_url = https://corp.awsapps.com/start\nsso_region = us-east-1\n\n\
         [profile keys]\nregion = eu-west-1\n",
    )
    .unwrap();
    std::fs::write(
        aws.join("credentials"),
        "[keys]\naws_access_key_id = AKIDPROFILE\naws_secret_access_key = s\n",
    )
    .unwrap();
    // SAFETY: this binary runs only this test, and nothing else has read
    // the environment yet.
    unsafe {
        // What the SDK's default chains would pick up first.
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIDENVIRONMENT");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "s");
        std::env::set_var("AWS_REGION", "us-west-2");
        // The AWS files above, and no instance metadata service.
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::remove_var("AWS_CONFIG_FILE");
        std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        std::env::remove_var("AWS_PROFILE");
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
    }

    // Static keys in the credentials file are refused, not used.
    let err = resolve_access_key_id("keys").unwrap_err();
    assert!(
        err.contains("AWS profile 'keys' sets aws_access_key_id"),
        "{err}"
    );

    // A profile that does not exist is an error, not the environment's keys.
    let err = resolve_access_key_id("missing").unwrap_err();
    assert!(
        err.contains("AWS profile 'missing' does not exist"),
        "{err}"
    );

    // A direct SSO profile goes to its SSO token, and there is none cached
    // here; the environment's keys are not used instead.
    let err = resolve_access_key_id("dev").unwrap_err();
    assert!(
        err.contains("FailedToLoadToken") && err.contains(".aws/sso/cache"),
        "{err}"
    );

    std::fs::remove_dir_all(&home).unwrap();
}
