use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::ops::Deref;
use std::path::{PathBuf, Path};
use std::str::FromStr;
use std::time::Duration;

use fluvio_types::PartitionId;
use tracing::debug;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use bytesize::ByteSize;

use fluvio_smartengine::transformation::TransformationConfig;
use fluvio_compression::Compression;
use crate::metadata::Direction;

mod bytesize_serde;

const SOURCE_SUFFIX: &str = "-source";
const IMAGE_PREFFIX: &str = "infinyon/fluvio-connect";

/// Versioned connector config
/// Use this config in the places where you need to enforce the version.
/// for example on the CLI create command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "apiVersion")]
pub enum ConnectorConfig {
    // V0 is the version of the config that was used before we introduced the versioning.
    #[serde(rename = "0.0.0")]
    V0_0_0(ConnectorConfigV1),
    #[serde(rename = "0.1.0")]
    V0_1_0(ConnectorConfigV1),
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        ConnectorConfig::V0_1_0(ConnectorConfigV1::default())
    }
}

mod serde_impl {
    use serde::{Deserialize};

    use crate::config::ConnectorConfigV1;

    use super::ConnectorConfig;

    impl<'a> Deserialize<'a> for ConnectorConfig {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'a>,
        {
            #[derive(Deserialize)]
            enum Version {
                #[serde(rename = "0.0.0")]
                V0,
                #[serde(rename = "0.1.0")]
                V1,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct VersionedConfig {
                api_version: Option<Version>,
                #[serde(flatten)]
                config: serde_yaml::Value,
            }
            let versioned_config: VersionedConfig = VersionedConfig::deserialize(deserializer)?;
            let version = versioned_config.api_version.unwrap_or(Version::V0);
            match version {
                Version::V0 => ConnectorConfigV1::deserialize(versioned_config.config)
                    .map(ConnectorConfig::V0_0_0)
                    .map_err(serde::de::Error::custom),

                Version::V1 => ConnectorConfigV1::deserialize(versioned_config.config)
                    .map(ConnectorConfig::V0_1_0)
                    .map_err(serde::de::Error::custom),
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConnectorConfigV1 {
    pub meta: MetaConfig,

    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub transforms: Option<TransformationConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct MetaConfig {
    pub name: String,

    #[serde(rename = "type")]
    pub type_: String,

    pub topic: String,

    pub version: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerParameters>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerParameters>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<SecretConfig>>,
}

impl MetaConfig {
    fn secrets(&self) -> HashSet<SecretConfig> {
        HashSet::from_iter(self.secrets.clone().unwrap_or_default().into_iter())
    }

    fn direction(&self) -> Direction {
        if self.type_.ends_with(SOURCE_SUFFIX) {
            Direction::source()
        } else {
            Direction::dest()
        }
    }

    pub fn image(&self) -> String {
        format!("{}-{}:{}", IMAGE_PREFFIX, self.type_, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConsumerParameters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<PartitionId>,
    #[serde(
        with = "bytesize_serde",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub max_bytes: Option<ByteSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProducerParameters {
    #[serde(with = "humantime_serde")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linger: Option<Duration>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<Compression>,

    #[serde(
        rename = "batch-size",
        alias = "batch_size",
        with = "bytesize_serde",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub batch_size: Option<ByteSize>,
}
#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Hash)]
pub struct SecretConfig {
    /// The name of the secret. It can only contain alphanumeric ASCII characters and underscores. It cannot start with a number.
    name: SecretName,
}

impl SecretConfig {
    pub fn name(&self) -> &str {
        &self.name.inner
    }

    pub fn new(secret_name: SecretName) -> Self {
        Self { name: secret_name }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretName {
    inner: String,
}

impl SecretName {
    fn validate(&self) -> anyhow::Result<()> {
        if self.inner.chars().count() == 0 {
            return Err(anyhow::anyhow!("Secret name cannot be empty"));
        }
        if !self
            .inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(anyhow::anyhow!(
                "Secret name `{}` can only contain alphanumeric ASCII characters and underscores",
                self.inner
            ));
        }
        if self.inner.chars().next().unwrap().is_ascii_digit() {
            return Err(anyhow::anyhow!(
                "Secret name `{}` cannot start with a number",
                self.inner
            ));
        }
        Ok(())
    }
}
impl FromStr for SecretName {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let secret_name = Self {
            inner: value.into(),
        };
        secret_name.validate()?;
        Ok(secret_name)
    }
}

impl Deref for SecretName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> Deserialize<'a> for SecretName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let inner = String::deserialize(deserializer)?;
        let secret = Self { inner };
        secret.validate().map_err(serde::de::Error::custom)?;
        Ok(secret)
    }
}

impl Serialize for SecretName {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.inner)
    }
}

impl ConnectorConfigV1 {
    fn meta(&self) -> &MetaConfig {
        &self.meta
    }

    fn mut_meta(&mut self) -> &mut MetaConfig {
        &mut self.meta
    }
}

impl ConnectorConfig {
    pub fn from_file<P: Into<PathBuf>>(path: P) -> Result<Self> {
        let mut file = File::open(path.into())?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Self::config_from_str(&contents)
    }

    /// Only parses the meta section of the config
    pub fn config_from_str(config_str: &str) -> Result<Self> {
        let connector_config: Self = serde_yaml::from_str(config_str)?;
        connector_config.validate_secret_names()?;

        debug!("Using connector config {connector_config:#?}");
        Ok(connector_config)
    }

    fn validate_secret_names(&self) -> Result<()> {
        for secret in self.secrets() {
            secret.name.validate()?;
        }
        Ok(())
    }
    pub fn meta(&self) -> &MetaConfig {
        match self {
            Self::V0_1_0(config) => config.meta(),
            Self::V0_0_0(config) => config.meta(),
        }
    }

    pub fn from_value(value: serde_yaml::Value) -> Result<Self> {
        let connector_config: Self = serde_yaml::from_value(value)?;
        connector_config.validate_secret_names()?;

        debug!("Using connector config {connector_config:#?}");
        Ok(connector_config)
    }

    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        std::fs::write(path, serde_yaml::to_string(self)?)?;
        Ok(())
    }
    pub fn mut_meta(&mut self) -> &mut MetaConfig {
        match self {
            Self::V0_1_0(config) => config.mut_meta(),
            Self::V0_0_0(config) => config.mut_meta(),
        }
    }

    pub fn secrets(&self) -> HashSet<SecretConfig> {
        match self {
            Self::V0_1_0(config) => config.meta.secrets(),
            Self::V0_0_0(_) => Default::default(),
        }
    }

    pub fn transforms(&self) -> Option<&TransformationConfig> {
        match self {
            Self::V0_1_0(config) => config.transforms.as_ref(),
            Self::V0_0_0(config) => config.transforms.as_ref(),
        }
    }

    pub fn direction(&self) -> Direction {
        self.meta().direction()
    }

    pub fn image(&self) -> String {
        self.meta().image()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use fluvio_smartengine::transformation::TransformationStep;
    use pretty_assertions::assert_eq;

    #[test]
    fn full_yaml_test() {
        //given
        let expected = ConnectorConfig::V0_1_0(ConnectorConfigV1 {
            meta: MetaConfig {
                name: "my-test-mqtt".to_string(),
                type_: "mqtt".to_string(),
                topic: "my-mqtt".to_string(),
                version: "0.1.0".to_string(),
                producer: Some(ProducerParameters {
                    linger: Some(Duration::from_millis(1)),
                    compression: Some(Compression::Gzip),
                    batch_size: Some(ByteSize::mb(44)),
                }),
                consumer: Some(ConsumerParameters {
                    partition: Some(10),
                    max_bytes: Some(ByteSize::mb(1)),
                }),
                secrets: Some(vec![SecretConfig {
                    name: "secret1".parse().unwrap(),
                }]),
            },
            transforms: Some(
                TransformationStep {
                    uses: "infinyon/json-sql".to_string(),
                    with: BTreeMap::from([
                        (
                            "mapping".to_string(),
                            "{\"table\":\"topic_message\"}".into(),
                        ),
                        ("param".to_string(), "param_value".into()),
                    ]),
                }
                .into(),
            ),
        });

        //when
        let connector_cfg = ConnectorConfig::from_file("test-data/connectors/full-config.yaml")
            .expect("Failed to load test config");

        //then
        assert_eq!(connector_cfg, expected);
    }

    #[test]
    fn simple_yaml_test() {
        //given
        let expected = ConnectorConfig::V0_1_0(ConnectorConfigV1 {
            meta: MetaConfig {
                name: "my-test-mqtt".to_string(),
                type_: "mqtt".to_string(),
                topic: "my-mqtt".to_string(),
                version: "0.1.0".to_string(),
                producer: None,
                consumer: None,
                secrets: None,
            },
            transforms: None,
        });

        //when
        let connector_cfg = ConnectorConfig::from_file("test-data/connectors/simple.yaml")
            .expect("Failed to load test config");

        //then
        assert_eq!(connector_cfg, expected);
    }

    #[test]
    fn error_yaml_tests() {
        let connector_cfg = ConnectorConfig::from_file("test-data/connectors/error-linger.yaml")
            .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "invalid value: string \"1\", expected a duration",
            format!("{connector_cfg}")
        );
        let connector_cfg =
            ConnectorConfig::from_file("test-data/connectors/error-compression.yaml")
                .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "unknown variant `gzipaoeu`, expected one of `none`, `gzip`, `snappy`, `lz4`, `zstd`",
            format!("{connector_cfg}")
        );

        let connector_cfg = ConnectorConfig::from_file("test-data/connectors/error-batchsize.yaml")
            .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "invalid value: string \"1aoeu\", expected parsable string",
            format!("{connector_cfg:?}")
        );
        let connector_cfg = ConnectorConfig::from_file("test-data/connectors/error-version.yaml")
            .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!("missing field `version`", format!("{connector_cfg:?}"));

        let connector_cfg =
            ConnectorConfig::from_file("test-data/connectors/error-secret-with-spaces.yaml")
                .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "Secret name `secret name` can only contain alphanumeric ASCII characters and underscores",
            format!("{connector_cfg:?}")
        );

        let connector_cfg =
            ConnectorConfig::from_file("test-data/connectors/error-secret-starts-with-number.yaml")
                .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "Secret name `1secret` cannot start with a number",
            format!("{connector_cfg:?}")
        );

        let connector_cfg =
            ConnectorConfig::from_file("test-data/connectors/error-invalid-api-version.yaml")
                .expect_err("This yaml should error");
        #[cfg(unix)]
        assert_eq!(
            "apiVersion: unknown variant `v1`, expected `0.0.0` or `0.1.0` at line 1 column 13",
            format!("{connector_cfg:?}")
        );
    }

    #[test]
    fn deserialize_test() {
        //given
        let yaml = r#"
            apiVersion: 0.1.0
            meta:
                name: kafka-out
                topic: poc1
                type: kafka-sink
                version: latest
            "#;

        let expected = ConnectorConfig::V0_1_0(ConnectorConfigV1 {
            meta: MetaConfig {
                name: "kafka-out".to_string(),
                type_: "kafka-sink".to_string(),
                topic: "poc1".to_string(),
                version: "latest".to_string(),
                producer: None,
                consumer: None,
                secrets: None,
            },
            transforms: None,
        });

        //when
        let connector_spec: ConnectorConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize");

        //then
        assert_eq!(connector_spec, expected);
    }

    #[test]
    fn deserialize_test_untagged() {
        //given
        let yaml = r#"
            meta:
                name: kafka-out
                topic: poc1
                type: kafka-sink
                version: latest
            "#;

        let expected = ConnectorConfig::V0_0_0(ConnectorConfigV1 {
            meta: MetaConfig {
                name: "kafka-out".to_string(),
                type_: "kafka-sink".to_string(),
                topic: "poc1".to_string(),
                version: "latest".to_string(),
                producer: None,
                consumer: None,
                secrets: None,
            },
            transforms: None,
        });

        //when
        let connector_spec: ConnectorConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize");

        //then
        assert_eq!(connector_spec, expected);
    }

    #[test]
    fn deserialize_with_integer_batch_size() {
        //given
        let yaml = r#"
        apiVersion: 0.1.0
        meta:
          version: 0.1.0
          name: my-test-mqtt
          type: mqtt-source
          topic: my-mqtt
          consumer:
            max_bytes: 1400
          producer:
            batch_size: 1600
        "#;

        let expected = ConnectorConfig::V0_1_0(ConnectorConfigV1 {
            meta: MetaConfig {
                name: "my-test-mqtt".to_string(),
                type_: "mqtt-source".to_string(),
                topic: "my-mqtt".to_string(),
                version: "0.1.0".to_string(),
                producer: Some(ProducerParameters {
                    linger: None,
                    compression: None,
                    batch_size: Some(ByteSize::b(1600)),
                }),
                consumer: Some(ConsumerParameters {
                    max_bytes: Some(ByteSize::b(1400)),
                    partition: None,
                }),
                secrets: None,
            },
            transforms: None,
        });

        //when
        let connector_spec: ConnectorConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize");

        //then
        assert_eq!(connector_spec, expected);
    }

    #[test]
    fn test_deserialize_transform() {
        //given

        //when
        let connector_spec: ConnectorConfig =
            ConnectorConfig::from_file("test-data/connectors/with_transform.yaml")
                .expect("Failed to deserialize");

        //then
        assert!(connector_spec.transforms().is_some());
        assert_eq!(
            connector_spec.transforms().unwrap().transforms[0]
                .uses
                .as_str(),
            "infinyon/sql"
        );
        assert_eq!(connector_spec.transforms().unwrap().transforms[0].with,
                       BTreeMap::from([("mapping".to_string(), "{\"map-columns\":{\"device_id\":{\"json-key\":\"device.device_id\",\"value\":{\"default\":0,\"required\":true,\"type\":\"int\"}},\"record\":{\"json-key\":\"$\",\"value\":{\"required\":true,\"type\":\"jsonb\"}}},\"table\":\"topic_message\"}".into())]));
    }

    #[test]
    fn test_deserialize_secret_name() {
        let secret_name: SecretName = serde_yaml::from_str("secret_name").unwrap();
        assert_eq!("secret_name", &*secret_name);

        assert!(
            serde_yaml::from_str::<SecretName>("secret name").is_err(),
            "string with space is not a valid secret name"
        );
    }

    #[test]
    fn test_serialize_secret_config() {
        let secrets = ["secret_name", "secret_name2"];
        let secret_configs = secrets
            .iter()
            .map(|s| SecretConfig::new(s.parse().unwrap()))
            .collect::<Vec<_>>();

        let serialized = serde_yaml::to_string(&secret_configs).expect("failed to serialize");
        assert_eq!(
            "- name: secret_name
- name: secret_name2
",
            serialized
        );
    }

    #[test]
    fn test_parse_secret_name() {
        let secret_name: SecretName = "secret_name".parse().unwrap();
        assert_eq!("secret_name", &*secret_name);

        assert!(
            "secret name".parse::<SecretName>().is_err(),
            "secret name should fail if has space"
        );

        assert!(
            "1secretname".parse::<SecretName>().is_err(),
            "secret name should fail if starts with number"
        );
        assert!(
            "secret-name".parse::<SecretName>().is_err(),
            "secret name should fail if has dash"
        );
    }

    #[test]
    fn test_serialize_secret_name() {
        let secret_name: SecretName = "secret_name".parse().unwrap();
        assert_eq!("secret_name", &*secret_name);

        let secret_name: SecretName = serde_yaml::from_str("secret_name").unwrap();
        assert_eq!("secret_name", &*secret_name);
    }

    #[test]
    fn test_deserialize_transform_many() {
        //given

        //when
        let connector_spec: ConnectorConfig =
            ConnectorConfig::from_file("test-data/connectors/with_transform_many.yaml")
                .expect("Failed to deserialize");

        //then
        assert!(connector_spec.transforms().is_some());
        let transform = &connector_spec.transforms().unwrap().transforms;
        assert_eq!(transform.len(), 3);
        assert_eq!(transform[0].uses.as_str(), "infinyon/json-sql");
        assert_eq!(
            transform[0].with,
            BTreeMap::from([(
                "mapping".to_string(),
                "{\"table\":\"topic_message\"}".into()
            )])
        );
        assert_eq!(transform[1].uses.as_str(), "infinyon/avro-sql");
        assert_eq!(transform[1].with, BTreeMap::default());
        assert_eq!(transform[2].uses.as_str(), "infinyon/regex-filter");
        assert_eq!(
            transform[2].with,
            BTreeMap::from([("regex".to_string(), "\\w".into())])
        );
    }

    #[test]
    fn sample_yaml_test_files() {
        let testfiles = vec!["tests/sample-http.yaml", "tests/sample-mqtt.yaml"];

        for tfile in testfiles {
            let res = ConnectorConfig::from_file(tfile);
            assert!(res.is_ok(), "failed to load {tfile}");
            let connector_cfg = res.unwrap();
            println!("{tfile}: {connector_cfg:?}");
        }
    }
}
