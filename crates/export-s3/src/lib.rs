//! Amazon S3 (and S3-compatible storage such as MinIO) as an export destination, using the
//! official AWS SDK for Rust.
//!
//! Settings: `bucket` (required), `region`, `prefix` (a folder prefix put in front of every
//! export), `endpoint` (for S3-compatible services), `profile` (a profile of the AWS config
//! files), `force_path_style` (defaults to true when an endpoint is set).
//!
//! Credentials, in order: the secrets `access_key_id` / `secret_access_key` / `session_token`
//! that `app` resolved from environment variables or the config file, else the standard AWS
//! chain (environment variables, shared config and credentials files, SSO, instance roles).

use std::sync::Arc;

use application::ports::{DestinationConfig, DestinationFactory, ExportDestination, ExportError};
use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use secrecy::ExposeSecret;
use tokio::sync::OnceCell;

/// What the destination needs, kept apart from the SDK client so it can be validated and tested
/// without a network.
#[derive(Debug, Clone, PartialEq, Eq)]
struct S3Settings {
    bucket: String,
    region: Option<String>,
    prefix: String,
    endpoint: Option<String>,
    profile: Option<String>,
    force_path_style: bool,
}

fn setting(config: &DestinationConfig, key: &str) -> Option<String> {
    config
        .settings
        .get(key)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_settings(config: &DestinationConfig) -> Result<S3Settings, ExportError> {
    let bucket = setting(config, "bucket").ok_or_else(|| {
        ExportError::Invalid(format!(
            "`{}`: set `bucket` to the S3 bucket name.",
            config.name
        ))
    })?;
    if bucket.contains('/') || bucket.contains(char::is_whitespace) {
        return Err(ExportError::Invalid(format!(
            "`{}`: `bucket` is just the bucket name (no `s3://`, no slashes).",
            config.name
        )));
    }
    let endpoint = setting(config, "endpoint");
    if let Some(e) = &endpoint
        && !(e.starts_with("http://") || e.starts_with("https://"))
    {
        return Err(ExportError::Invalid(format!(
            "`{}`: `endpoint` must start with http:// or https://.",
            config.name
        )));
    }
    let force_path_style = match setting(config, "force_path_style").as_deref() {
        Some("true" | "1" | "yes") => true,
        Some("false" | "0" | "no") => false,
        Some(other) => {
            return Err(ExportError::Invalid(format!(
                "`{}`: `force_path_style` must be true or false, not `{other}`.",
                config.name
            )));
        }
        None => endpoint.is_some(),
    };
    Ok(S3Settings {
        bucket,
        region: setting(config, "region"),
        prefix: setting(config, "prefix").unwrap_or_default(),
        endpoint,
        profile: setting(config, "profile"),
        force_path_style,
    })
}

/// `prefix` + `path`, with exactly one `/` between them and none at the start.
fn object_key(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        path.trim_start_matches('/').to_owned()
    } else {
        format!("{prefix}/{}", path.trim_start_matches('/'))
    }
}

fn check_path(path: &str) -> Result<(), ExportError> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|s| s == "..");
    if bad {
        return Err(ExportError::Invalid(format!(
            "`{path}` is not a valid path inside the destination."
        )));
    }
    Ok(())
}

/// Classifies an S3 error code.
fn classify(code: Option<&str>, message: String) -> ExportError {
    match code {
        Some(
            "AccessDenied"
            | "InvalidAccessKeyId"
            | "SignatureDoesNotMatch"
            | "ExpiredToken"
            | "InvalidToken"
            | "AllAccessDisabled",
        ) => ExportError::Denied(message),
        Some("NoSuchBucket" | "PermanentRedirect" | "AuthorizationHeaderMalformed") => {
            ExportError::Invalid(message)
        }
        _ => ExportError::Other(message),
    }
}

fn map_sdk_error<E, R>(e: SdkError<E, R>) -> ExportError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
{
    let message = DisplayErrorContext(&e).to_string();
    match &e {
        SdkError::DispatchFailure(_) | SdkError::TimeoutError(_) => {
            ExportError::Unreachable(message)
        }
        _ => classify(e.code(), message),
    }
}

pub struct S3Destination {
    settings: S3Settings,
    credentials: Option<Credentials>,
    client: OnceCell<Client>,
}

impl S3Destination {
    async fn client(&self) -> Result<&Client, ExportError> {
        self.client
            .get_or_try_init(|| async {
                // TLS with rustls and the ring provider; certificates come from the platform.
                let http = aws_smithy_http_client::Builder::new()
                    .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                        aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
                    ))
                    .build_https();
                let mut loader =
                    aws_config::defaults(aws_config::BehaviorVersion::latest()).http_client(http);
                if let Some(region) = &self.settings.region {
                    loader = loader.region(Region::new(region.clone()));
                }
                if let Some(profile) = &self.settings.profile {
                    loader = loader.profile_name(profile);
                }
                if let Some(credentials) = &self.credentials {
                    loader = loader.credentials_provider(credentials.clone());
                }
                let shared = loader.load().await;
                let mut builder = aws_sdk_s3::config::Builder::from(&shared)
                    .force_path_style(self.settings.force_path_style);
                if let Some(endpoint) = &self.settings.endpoint {
                    builder = builder.endpoint_url(endpoint);
                }
                if shared.region().is_none() {
                    // S3-compatible services rarely care; the SDK insists on one.
                    builder = builder.region(Region::new("us-east-1"));
                }
                Ok(Client::from_conf(builder.build()))
            })
            .await
    }
}

#[async_trait]
impl ExportDestination for S3Destination {
    fn kind(&self) -> &'static str {
        "s3"
    }

    fn describe(&self) -> String {
        let prefix = self.settings.prefix.trim_matches('/');
        if prefix.is_empty() {
            format!("s3://{}/", self.settings.bucket)
        } else {
            format!("s3://{}/{prefix}/", self.settings.bucket)
        }
    }

    async fn test(&self) -> Result<(), ExportError> {
        let client = self.client().await?;
        // Write and remove a small probe object: this checks the permissions that exports need.
        let key = object_key(
            &self.settings.prefix,
            &format!(".fathom-write-test-{}", std::process::id()),
        );
        client
            .put_object()
            .bucket(&self.settings.bucket)
            .key(&key)
            .body(ByteStream::from_static(b"fathom"))
            .send()
            .await
            .map_err(map_sdk_error)?;
        // Cleaning up may be denied for write-only credentials; the probe is a few bytes.
        let _ = client
            .delete_object()
            .bucket(&self.settings.bucket)
            .key(&key)
            .send()
            .await;
        Ok(())
    }

    async fn put(&self, path: &str, data: Vec<u8>) -> Result<String, ExportError> {
        check_path(path)?;
        let client = self.client().await?;
        let key = object_key(&self.settings.prefix, path);
        let location = format!("s3://{}/{key}", self.settings.bucket);

        // Never overwrite: look first (a clear message), and make the write itself conditional
        // so a concurrent writer cannot be overwritten either.
        match client
            .head_object()
            .bucket(&self.settings.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => return Err(ExportError::AlreadyExists(location)),
            Err(e) => {
                let missing = e
                    .as_service_error()
                    .is_some_and(aws_sdk_s3::operation::head_object::HeadObjectError::is_not_found)
                    || e.raw_response().is_some_and(|r| r.status().as_u16() == 404);
                if !missing {
                    return Err(map_sdk_error(e));
                }
            }
        }
        client
            .put_object()
            .bucket(&self.settings.bucket)
            .key(&key)
            .if_none_match("*")
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|e| {
                if e.raw_response().is_some_and(|r| r.status().as_u16() == 412)
                    || e.code() == Some("PreconditionFailed")
                {
                    ExportError::AlreadyExists(location.clone())
                } else {
                    map_sdk_error(e)
                }
            })?;
        Ok(location)
    }
}

pub struct S3Factory;

impl DestinationFactory for S3Factory {
    fn kind(&self) -> &'static str {
        "s3"
    }

    fn create(
        &self,
        config: &DestinationConfig,
    ) -> Result<Arc<dyn ExportDestination>, ExportError> {
        let settings = parse_settings(config)?;
        let secret = |key: &str| {
            config
                .secrets
                .get(key)
                .map(|s| s.expose_secret().to_owned())
        };
        let credentials = match (secret("access_key_id"), secret("secret_access_key")) {
            (Some(id), Some(key)) => Some(Credentials::new(
                id,
                key,
                secret("session_token"),
                None,
                "fathom-config",
            )),
            (None, None) => None,
            _ => {
                return Err(ExportError::Invalid(format!(
                    "`{}`: set both `access_key_id` and `secret_access_key`, or neither to use the AWS credential chain.",
                    config.name
                )));
            }
        };
        Ok(Arc::new(S3Destination {
            settings,
            credentials,
            client: OnceCell::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use secrecy::SecretString;

    use super::*;

    fn config(settings: &[(&str, &str)], secrets: &[(&str, &str)]) -> DestinationConfig {
        DestinationConfig {
            name: "Analytics archive".into(),
            kind: "s3".into(),
            settings: settings
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            secrets: secrets
                .iter()
                .map(|(k, v)| ((*k).to_owned(), SecretString::from((*v).to_owned())))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn a_bucket_is_required_and_must_be_just_a_name() {
        assert!(parse_settings(&config(&[], &[])).is_err());
        for bad in ["s3://b", "a/b", "my bucket"] {
            assert!(
                parse_settings(&config(&[("bucket", bad)], &[])).is_err(),
                "{bad}"
            );
        }
        let s =
            parse_settings(&config(&[("bucket", "acme"), ("prefix", "exports/")], &[])).unwrap();
        assert_eq!(s.bucket, "acme");
        assert_eq!(s.prefix, "exports/");
        assert!(!s.force_path_style, "virtual-hosted style against AWS");
    }

    #[test]
    fn a_custom_endpoint_switches_to_path_style_unless_told_otherwise() {
        let s = parse_settings(&config(
            &[("bucket", "b"), ("endpoint", "http://localhost:9000")],
            &[],
        ))
        .unwrap();
        assert!(s.force_path_style);
        let s = parse_settings(&config(
            &[
                ("bucket", "b"),
                ("endpoint", "https://s3.example.com"),
                ("force_path_style", "false"),
            ],
            &[],
        ))
        .unwrap();
        assert!(!s.force_path_style);
        assert!(
            parse_settings(&config(
                &[("bucket", "b"), ("endpoint", "localhost:9000")],
                &[]
            ))
            .is_err()
        );
        assert!(
            parse_settings(&config(
                &[("bucket", "b"), ("force_path_style", "maybe")],
                &[]
            ))
            .is_err()
        );
    }

    #[test]
    fn keys_join_prefix_and_path_with_one_slash() {
        assert_eq!(object_key("", "a/b.csv"), "a/b.csv");
        assert_eq!(object_key("exports", "a.csv"), "exports/a.csv");
        assert_eq!(object_key("/exports/", "a.csv"), "exports/a.csv");
    }

    #[test]
    fn paths_that_leave_the_prefix_are_refused() {
        for bad in ["", "/a", "../a", "a/../b", "a\0"] {
            assert!(check_path(bad).is_err(), "{bad:?}");
        }
        assert!(check_path("a/b.csv").is_ok());
    }

    #[test]
    fn s3_error_codes_are_classified() {
        assert!(matches!(
            classify(Some("AccessDenied"), "m".into()),
            ExportError::Denied(_)
        ));
        assert!(matches!(
            classify(Some("InvalidAccessKeyId"), "m".into()),
            ExportError::Denied(_)
        ));
        assert!(matches!(
            classify(Some("NoSuchBucket"), "m".into()),
            ExportError::Invalid(_)
        ));
        assert!(matches!(
            classify(Some("SlowDown"), "m".into()),
            ExportError::Other(_)
        ));
        assert!(matches!(classify(None, "m".into()), ExportError::Other(_)));
    }

    #[test]
    fn credentials_come_in_pairs_or_not_at_all() {
        let base = [("bucket", "b")];
        assert!(
            S3Factory.create(&config(&base, &[])).is_ok(),
            "falls back to the AWS chain"
        );
        assert!(
            S3Factory
                .create(&config(
                    &base,
                    &[("access_key_id", "AKIA"), ("secret_access_key", "s")]
                ))
                .is_ok()
        );
        let lone = S3Factory
            .create(&config(&base, &[("access_key_id", "AKIA")]))
            .err()
            .unwrap();
        assert!(lone.to_string().contains("both"), "{lone}");
    }

    #[test]
    fn describe_shows_the_bucket_and_prefix_but_no_secrets() {
        let d = S3Factory
            .create(&config(
                &[("bucket", "acme-analytics"), ("prefix", "/exports/")],
                &[
                    ("access_key_id", "AKIASECRET"),
                    ("secret_access_key", "topsecret"),
                ],
            ))
            .unwrap();
        assert_eq!(d.describe(), "s3://acme-analytics/exports/");
        assert!(
            !format!("{:?}", config(&[], &[("secret_access_key", "topsecret")]))
                .contains("topsecret")
        );
    }
}
