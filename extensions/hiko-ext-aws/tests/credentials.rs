//! Each config uses exactly the credentials and region source it names.
//!
//! This file must hold exactly one test: it sets environment variables,
//! which is only sound while no other thread reads the environment, and
//! each file under tests/ is its own process.

use hiko_ext_aws::resolve_access_key_id;

#[test]
fn credentials_come_only_from_the_named_source() {
    let home = std::env::temp_dir().join(format!("hiko-ext-aws-home-{}", std::process::id()));
    let aws = home.join(".aws");
    std::fs::create_dir_all(&aws).unwrap();
    std::fs::write(
        aws.join("config"),
        "[profile dev]\nregion = eu-west-1\n\n[profile noregion]\n",
    )
    .unwrap();
    std::fs::write(
        aws.join("credentials"),
        "[dev]\naws_access_key_id = AKIDPROFILE\naws_secret_access_key = s\n\n\
         [noregion]\naws_access_key_id = AKIDNOREGION\naws_secret_access_key = s\n",
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

    // The profile's keys, not the environment's.
    assert_eq!(
        resolve_access_key_id(Some("dev")).as_deref(),
        Ok("AKIDPROFILE")
    );

    // A profile without a region is an error, not AWS_REGION.
    let err = resolve_access_key_id(Some("noregion")).unwrap_err();
    assert!(
        err.contains("AWS profile 'noregion' does not exist or sets no region"),
        "{err}"
    );

    // A profile that does not exist is an error, not the environment's keys.
    let err = resolve_access_key_id(Some("missing")).unwrap_err();
    assert!(
        err.contains("AWS profile 'missing' does not exist or sets no region"),
        "{err}"
    );

    // The instance profile uses the instance metadata service only.
    let err = resolve_access_key_id(None).unwrap_err();
    assert!(
        err.contains("the instance metadata service gave no region"),
        "{err}"
    );

    std::fs::remove_dir_all(&home).unwrap();
}
