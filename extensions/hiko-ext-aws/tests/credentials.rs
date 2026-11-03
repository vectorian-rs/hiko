//! Each config uses exactly the credentials source it names. One test in
//! its own binary: it sets environment variables, which no other thread
//! may read meanwhile.

#[test]
fn credentials_come_only_from_the_named_source() {
    let home = std::env::temp_dir().join(format!("hiko-ext-aws-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this binary runs only this test, on one thread, and sets the
    // variables before anything reads them.
    unsafe {
        // Keys the default chain would pick up first.
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
        // No real AWS files, and no instance metadata service to reach.
        std::env::set_var("HOME", &home);
        std::env::remove_var("AWS_CONFIG_FILE");
        std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        std::env::remove_var("AWS_PROFILE");
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
    }

    // A profile that does not exist is an error, not the environment's keys.
    hiko_ext_aws::resolve_credentials(Some("dev")).expect_err("no profile 'dev'");

    // The instance profile uses the metadata service only.
    hiko_ext_aws::resolve_credentials(None).expect_err("the metadata service is disabled");
}
