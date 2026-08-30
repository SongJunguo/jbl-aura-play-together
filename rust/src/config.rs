use std::collections::HashMap;
use std::env::{self, VarError};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

use crate::error::JblError;
use crate::model::DeviceIdentity;
use crate::private_file::{read_private_file, read_private_file_if_present, PrivateFileKind};

const MAX_CONFIG_BYTES: u64 = 65_536;
const DEFAULT_GENA_CALLBACK_PORT: u16 = 8_098;
const DEFAULT_BROADCAST_CONFIRMATION: BroadcastConfirmation = BroadcastConfirmation::Ack;
const ALLOWED_KEYS: &[&str] = &[
    "JBL_IP",
    "JBL_LOCAL_API_CERT",
    "JBL_LOCAL_API_KEY",
    "JBL_LOCAL_API_TLS_SHA256",
    "JBL_LOCAL_API_TIMEOUT",
    "JBL_GENA_CALLBACK_PORT",
    "JBL_BROADCAST_CONFIRMATION",
    "JBL_BT_MAC",
    "JBL_EXPECTED_MODEL",
    "AURA_BT_MAC",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastConfirmation {
    Ack,
    Gena,
}

#[derive(Clone)]
pub struct RuntimeConfig {
    pub address: String,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub tls_sha256: String,
    pub jbl_identity: DeviceIdentity,
    pub expected_model: String,
    pub aura_identity: DeviceIdentity,
    pub timeout: Duration,
    pub gena_callback_port: u16,
    pub broadcast_confirmation: BroadcastConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigSource {
    Required(PathBuf),
    OptionalDefault(PathBuf),
    None,
}

impl ConfigSource {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Required(path) | Self::OptionalDefault(path) => Some(path),
            Self::None => None,
        }
    }
}

fn automatic_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(root) = env::var_os("APPDATA").filter(|root| !root.is_empty()) {
        return Some(
            PathBuf::from(root)
                .join("jbl-aura-link-rust")
                .join("devices.env"),
        );
    }
    if let Some(root) = env::var_os("XDG_CONFIG_HOME").filter(|root| !root.is_empty()) {
        return Some(
            PathBuf::from(root)
                .join("jbl-aura-link-rust")
                .join("devices.env"),
        );
    }
    env::var_os("HOME")
        .filter(|root| !root.is_empty())
        .map(|root| {
            PathBuf::from(root)
                .join(".config")
                .join("jbl-aura-link-rust")
                .join("devices.env")
        })
}

fn select_config_source(
    explicit_path: Option<PathBuf>,
    environment_path: Option<PathBuf>,
    automatic_path: Option<PathBuf>,
) -> ConfigSource {
    if let Some(path) = explicit_path {
        ConfigSource::Required(path)
    } else if let Some(path) = environment_path {
        ConfigSource::Required(path)
    } else if let Some(path) = automatic_path {
        ConfigSource::OptionalDefault(path)
    } else {
        ConfigSource::None
    }
}

fn parse_config_text(text: &str) -> Result<HashMap<String, String>, JblError> {
    let mut values = HashMap::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(JblError::InvalidConfig);
        };
        let key = raw_key.trim();
        if !ALLOWED_KEYS.contains(&key) {
            continue;
        }
        let mut value = raw_value.trim();
        if value.len() >= 2 {
            let first = value.as_bytes()[0];
            let last = value.as_bytes()[value.len() - 1];
            if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
                value = &value[1..value.len() - 1];
            }
        }
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(JblError::InvalidConfig);
        }
    }
    Ok(values)
}

fn resolve_path(value: &str, config_path: Option<&Path>) -> Result<PathBuf, JblError> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Ok(path);
    }
    if let Some(parent) = config_path.and_then(Path::parent) {
        return Ok(parent.join(path));
    }
    env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|_| JblError::InvalidConfig)
}

fn required(values: &HashMap<String, String>, key: &'static str) -> Result<String, JblError> {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or(JblError::MissingSetting(key))
}

fn environment_overrides() -> Result<HashMap<String, String>, JblError> {
    let mut values = HashMap::new();
    for key in ALLOWED_KEYS {
        match env::var(key) {
            Ok(value) => {
                values.insert((*key).to_string(), value);
            }
            Err(VarError::NotPresent) => {}
            Err(VarError::NotUnicode(_)) => return Err(JblError::InvalidConfig),
        }
    }
    Ok(values)
}

fn config_values(source: &ConfigSource) -> Result<HashMap<String, String>, JblError> {
    let payload = match source {
        ConfigSource::Required(path) => Some(read_private_file(
            path,
            MAX_CONFIG_BYTES,
            PrivateFileKind::Config,
        )?),
        ConfigSource::OptionalDefault(path) => {
            read_private_file_if_present(path, MAX_CONFIG_BYTES, PrivateFileKind::Config)?
        }
        ConfigSource::None => None,
    };
    let Some(payload) = payload else {
        return Ok(HashMap::new());
    };
    let text = String::from_utf8(payload).map_err(|_| JblError::InvalidConfig)?;
    parse_config_text(&text)
}

impl RuntimeConfig {
    pub fn load(explicit_path: Option<PathBuf>) -> Result<Self, JblError> {
        let source = select_config_source(
            explicit_path,
            env::var_os("JBL_AURA_CONFIG").map(PathBuf::from),
            automatic_config_path(),
        );
        Self::load_from_source(source, &environment_overrides()?)
    }

    fn load_from_source(
        source: ConfigSource,
        overrides: &HashMap<String, String>,
    ) -> Result<Self, JblError> {
        let mut values = config_values(&source)?;
        values.extend(overrides.clone());

        let certificate_value = required(&values, "JBL_LOCAL_API_CERT")?;
        let private_key_value = required(&values, "JBL_LOCAL_API_KEY")?;
        let timeout_seconds = values
            .get("JBL_LOCAL_API_TIMEOUT")
            .map(String::as_str)
            .unwrap_or("5")
            .parse::<u64>()
            .ok()
            .filter(|value| (1..=30).contains(value))
            .ok_or(JblError::InvalidTimeout)?;
        let gena_callback_port = match values.get("JBL_GENA_CALLBACK_PORT") {
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|value| (1_024..=u16::MAX).contains(value))
                .ok_or(JblError::InvalidConfig)?,
            None => DEFAULT_GENA_CALLBACK_PORT,
        };
        let broadcast_confirmation =
            match values.get("JBL_BROADCAST_CONFIRMATION").map(String::as_str) {
                Some("ack") => BroadcastConfirmation::Ack,
                Some("gena") => BroadcastConfirmation::Gena,
                Some(_) => return Err(JblError::InvalidConfig),
                None => DEFAULT_BROADCAST_CONFIRMATION,
            };
        let jbl_identity = DeviceIdentity::parse(&required(&values, "JBL_BT_MAC")?)
            .ok_or(JblError::InvalidConfig)?;
        let aura_identity = DeviceIdentity::parse(&required(&values, "AURA_BT_MAC")?)
            .ok_or(JblError::InvalidConfig)?;
        if jbl_identity == aura_identity {
            return Err(JblError::InvalidConfig);
        }

        Ok(Self {
            address: required(&values, "JBL_IP")?,
            certificate: resolve_path(&certificate_value, source.path())?,
            private_key: resolve_path(&private_key_value, source.path())?,
            tls_sha256: required(&values, "JBL_LOCAL_API_TLS_SHA256")?,
            jbl_identity,
            expected_model: values
                .get("JBL_EXPECTED_MODEL")
                .cloned()
                .unwrap_or_else(|| "JBL Authentics 300".to_string()),
            aura_identity,
            timeout: Duration::from_secs(timeout_seconds),
            gena_callback_port,
            broadcast_confirmation,
        })
    }

    /// Atomically installs the independent Rust service configuration from a
    /// complete set of process-environment values. Values never appear on the
    /// command line or in output, and the destination is fixed to the private
    /// automatic Rust configuration path.
    #[cfg(unix)]
    pub fn install_private_default_from_environment() -> Result<(), JblError> {
        let overrides = environment_overrides()?;
        for key in [
            "JBL_IP",
            "JBL_LOCAL_API_CERT",
            "JBL_LOCAL_API_KEY",
            "JBL_LOCAL_API_TLS_SHA256",
            "JBL_BT_MAC",
            "AURA_BT_MAC",
        ] {
            required(&overrides, key)?;
        }
        let config = Self::load_from_source(ConfigSource::None, &overrides)?;
        let path = automatic_config_path().ok_or(JblError::ConfigUnavailable)?;
        install_private_config(&path, &config)
    }
}

#[cfg(unix)]
fn install_private_config(path: &Path, config: &RuntimeConfig) -> Result<(), JblError> {
    let parent = path.parent().ok_or(JblError::InvalidConfig)?;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).recursive(true);
    builder
        .create(parent)
        .map_err(|_| JblError::ConfigUnavailable)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| JblError::ConfigUnavailable)?;
    if !parent_metadata.file_type().is_dir()
        || parent_metadata.uid() != unsafe { libc::geteuid() }
        || parent_metadata.mode() & 0o777 != 0o700
    {
        return Err(JblError::ConfigPermissions);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o777 == 0o600
                && metadata.nlink() == 1 => {}
        Ok(_) => return Err(JblError::ConfigPermissions),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(JblError::ConfigUnavailable),
    }

    let certificate = config
        .certificate
        .to_str()
        .filter(|value| safe_config_value(value))
        .ok_or(JblError::InvalidConfig)?;
    let private_key = config
        .private_key
        .to_str()
        .filter(|value| safe_config_value(value))
        .ok_or(JblError::InvalidConfig)?;
    for value in [
        config.address.as_str(),
        config.tls_sha256.as_str(),
        config.expected_model.as_str(),
    ] {
        if !safe_config_value(value) {
            return Err(JblError::InvalidConfig);
        }
    }
    let payload = format!(
        "JBL_IP={}\nJBL_EXPECTED_MODEL={}\nJBL_BT_MAC={}\nAURA_BT_MAC={}\nJBL_LOCAL_API_CERT={}\nJBL_LOCAL_API_KEY={}\nJBL_LOCAL_API_TLS_SHA256={}\nJBL_LOCAL_API_TIMEOUT={}\nJBL_GENA_CALLBACK_PORT={}\nJBL_BROADCAST_CONFIRMATION={}\n",
        config.address,
        config.expected_model,
        config.jbl_identity.compact_config_value(),
        config.aura_identity.compact_config_value(),
        certificate,
        private_key,
        config.tls_sha256,
        config.timeout.as_secs(),
        config.gena_callback_port,
        match config.broadcast_confirmation {
            BroadcastConfirmation::Ack => "ack",
            BroadcastConfirmation::Gena => "gena",
        },
    );

    let mut random = [0_u8; 8];
    openssl::rand::rand_bytes(&mut random).map_err(|_| JblError::ConfigUnavailable)?;
    let mut suffix = String::with_capacity(16);
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut suffix, "{byte:02x}").map_err(|_| JblError::ConfigUnavailable)?;
    }
    let temporary = parent.join(format!(".devices.{suffix}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(|_| JblError::ConfigUnavailable)?;
    let result = (|| {
        file.write_all(payload.as_bytes())
            .map_err(|_| JblError::ConfigUnavailable)?;
        file.sync_all().map_err(|_| JblError::ConfigUnavailable)?;
        drop(file);
        fs::rename(&temporary, path).map_err(|_| JblError::ConfigUnavailable)?;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| JblError::ConfigUnavailable)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn safe_config_value(value: &str) -> bool {
    !(value.is_empty()
        || value.contains(['\0', '\n', '\r'])
        || value.starts_with(['\'', '"'])
        || value.ends_with(['\'', '"']))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[cfg(unix)]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    fn temporary_directory() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jbl-aura-config-{}-{unique}", std::process::id()));
        fs::create_dir(&path).expect("temporary directory should be created");
        path
    }

    fn complete_overrides() -> HashMap<String, String> {
        HashMap::from([
            ("JBL_IP".to_string(), "192.0.2.10".to_string()),
            (
                "JBL_LOCAL_API_CERT".to_string(),
                "/private/client-cert.pem".to_string(),
            ),
            (
                "JBL_LOCAL_API_KEY".to_string(),
                "/private/client-key.pem".to_string(),
            ),
            (
                "JBL_LOCAL_API_TLS_SHA256".to_string(),
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
            ("JBL_BT_MAC".to_string(), "02:00:00:00:00:01".to_string()),
            ("AURA_BT_MAC".to_string(), "020000000002".to_string()),
        ])
    }

    #[cfg(unix)]
    #[test]
    fn private_installer_writes_an_atomic_owner_only_round_trip() {
        let root = temporary_directory();
        let path = root.join("jbl-aura-link-rust").join("devices.env");
        let mut overrides = complete_overrides();
        overrides.insert("JBL_GENA_CALLBACK_PORT".to_string(), "48098".to_string());
        let config = RuntimeConfig::load_from_source(ConfigSource::None, &overrides)
            .expect("fixture config");
        install_private_config(&path, &config).expect("private install");
        let metadata = fs::symlink_metadata(&path).expect("metadata");
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::symlink_metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let loaded = RuntimeConfig::load_from_source(ConfigSource::Required(path), &HashMap::new())
            .expect("round trip");
        assert_eq!(loaded.address, config.address);
        assert!(loaded.jbl_identity == config.jbl_identity);
        assert!(loaded.aura_identity == config.aura_identity);
        assert_eq!(loaded.gena_callback_port, config.gena_callback_port);
        assert_eq!(loaded.broadcast_confirmation, config.broadcast_confirmation);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn private_installer_refuses_broad_or_symlinked_destination() {
        for symlinked in [false, true] {
            let root = temporary_directory();
            let parent = root.join("jbl-aura-link-rust");
            fs::create_dir(&parent).expect("parent");
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("mode");
            let path = parent.join("devices.env");
            if symlinked {
                symlink("target", &path).expect("symlink");
            } else {
                fs::write(&path, b"placeholder\n").expect("file");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
            }
            let config = RuntimeConfig::load_from_source(ConfigSource::None, &complete_overrides())
                .expect("fixture config");
            assert_eq!(
                install_private_config(&path, &config),
                Err(JblError::ConfigPermissions)
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn parses_allowlisted_values_without_shell_evaluation() {
        let values = parse_config_text(
            "JBL_IP=192.0.2.10\n\
             UNKNOWN=$(unsafe)\n\
             JBL_DEVICE_NAME='attacker-controlled-name'\n\
             JBL_BT_MAC='02:00:00:00:00:01'\n\
             AURA_BT_MAC=\"020000000002\"\n",
        )
        .expect("synthetic config should parse");
        assert_eq!(values.get("JBL_IP").map(String::as_str), Some("192.0.2.10"));
        assert_eq!(
            values.get("JBL_BT_MAC").map(String::as_str),
            Some("02:00:00:00:00:01")
        );
        assert!(!values.contains_key("UNKNOWN"));
        assert!(!values.contains_key("JBL_DEVICE_NAME"));
    }

    #[test]
    fn device_identity_normalizes_supported_private_config_forms() {
        assert_eq!(std::mem::size_of::<DeviceIdentity>(), 6);
        let colon =
            DeviceIdentity::parse("02:00:00:00:00:01").expect("colon form should be accepted");
        let hyphen_value = "02:00:00:00:00:01".replace(':', "-");
        let hyphen = DeviceIdentity::parse(&hyphen_value).expect("hyphen form should be accepted");
        let compact =
            DeviceIdentity::parse("020000000001").expect("compact form should be accepted");
        assert!(colon == hyphen && hyphen == compact);
    }

    #[test]
    fn device_identity_rejects_malformed_multicast_and_zero_values() {
        let mut multicast = "02:00:00:00:00:01".to_string();
        multicast.replace_range(..2, "03");
        let zero = "00".repeat(6);
        for value in [
            "".to_string(),
            "02000000001".to_string(),
            "02:00-00:00:00:01".to_string(),
            "02:00:00:00:00:0g".to_string(),
            multicast,
            zero,
        ] {
            assert!(DeviceIdentity::parse(&value).is_none());
        }
    }

    #[test]
    fn runtime_config_rejects_identical_private_identities() {
        let mut overrides = complete_overrides();
        overrides.insert("AURA_BT_MAC".to_string(), "020000000001".to_string());
        assert!(matches!(
            RuntimeConfig::load_from_source(ConfigSource::None, &overrides),
            Err(JblError::InvalidConfig)
        ));
    }

    #[test]
    fn runtime_config_requires_both_private_identities() {
        for key in ["JBL_BT_MAC", "AURA_BT_MAC"] {
            let mut overrides = complete_overrides();
            overrides.remove(key);
            assert_eq!(
                RuntimeConfig::load_from_source(ConfigSource::None, &overrides).err(),
                Some(JblError::MissingSetting(key))
            );
        }
    }

    #[test]
    fn runtime_config_defaults_and_validates_gena_callback_port() {
        let config = RuntimeConfig::load_from_source(ConfigSource::None, &complete_overrides())
            .expect("fixture config");
        assert_eq!(config.gena_callback_port, DEFAULT_GENA_CALLBACK_PORT);

        for port in [1_024_u16, 8_098, u16::MAX] {
            let mut overrides = complete_overrides();
            overrides.insert("JBL_GENA_CALLBACK_PORT".to_string(), port.to_string());
            let config = RuntimeConfig::load_from_source(ConfigSource::None, &overrides)
                .expect("in-range callback port should be accepted");
            assert_eq!(config.gena_callback_port, port);
        }

        for value in ["", "1023", "65536", "not-a-port"] {
            let mut overrides = complete_overrides();
            overrides.insert("JBL_GENA_CALLBACK_PORT".to_string(), value.to_string());
            assert_eq!(
                RuntimeConfig::load_from_source(ConfigSource::None, &overrides).err(),
                Some(JblError::InvalidConfig)
            );
        }
    }

    #[test]
    fn runtime_config_defaults_and_validates_broadcast_confirmation() {
        let config = RuntimeConfig::load_from_source(ConfigSource::None, &complete_overrides())
            .expect("fixture config");
        assert_eq!(config.broadcast_confirmation, BroadcastConfirmation::Ack);

        for (value, expected) in [
            ("ack", BroadcastConfirmation::Ack),
            ("gena", BroadcastConfirmation::Gena),
        ] {
            let mut overrides = complete_overrides();
            overrides.insert("JBL_BROADCAST_CONFIRMATION".to_string(), value.to_string());
            let config = RuntimeConfig::load_from_source(ConfigSource::None, &overrides)
                .expect("allowlisted confirmation mode");
            assert_eq!(config.broadcast_confirmation, expected);
        }

        for value in ["", "ACK", "strict", "true"] {
            let mut overrides = complete_overrides();
            overrides.insert("JBL_BROADCAST_CONFIRMATION".to_string(), value.to_string());
            assert_eq!(
                RuntimeConfig::load_from_source(ConfigSource::None, &overrides).err(),
                Some(JblError::InvalidConfig)
            );
        }
    }

    #[test]
    fn legacy_friendly_names_cannot_influence_runtime_identity() {
        let mut overrides = complete_overrides();
        overrides.insert(
            "JBL_DEVICE_NAME".to_string(),
            "attacker-controlled-name".to_string(),
        );
        overrides.insert(
            "AURA_DEVICE_NAME".to_string(),
            "attacker-controlled-name".to_string(),
        );
        let config = RuntimeConfig::load_from_source(ConfigSource::None, &overrides)
            .expect("ignored friendly names must not affect valid identities");
        assert!(config.jbl_identity != config.aura_identity);
    }

    #[test]
    fn rejects_duplicate_allowlisted_key() {
        assert_eq!(
            parse_config_text("JBL_IP=192.0.2.10\nJBL_IP=192.0.2.11\n").unwrap_err(),
            JblError::InvalidConfig
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_missing_config_is_required_even_with_complete_overrides() {
        let directory = temporary_directory();
        let missing = directory.join("missing.env");
        let source = select_config_source(
            Some(missing),
            None,
            Some(directory.join("ignored-default.env")),
        );
        assert!(matches!(source, ConfigSource::Required(_)));
        assert_eq!(
            RuntimeConfig::load_from_source(source, &complete_overrides()).err(),
            Some(JblError::ConfigUnavailable)
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn environment_config_path_is_required_when_selected() {
        let directory = temporary_directory();
        let missing = directory.join("missing-override.env");
        let source = select_config_source(
            None,
            Some(missing),
            Some(directory.join("ignored-default.env")),
        );
        assert!(matches!(source, ConfigSource::Required(_)));
        assert_eq!(
            RuntimeConfig::load_from_source(source, &complete_overrides()).err(),
            Some(JblError::ConfigUnavailable)
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn missing_automatic_default_allows_environment_only_configuration() {
        let directory = temporary_directory();
        let source = ConfigSource::OptionalDefault(directory.join("missing-default.env"));
        let config = RuntimeConfig::load_from_source(source, &complete_overrides())
            .expect("pure ENOENT default may fall back to complete environment");
        assert_eq!(config.address, "192.0.2.10");
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_default_symlink_is_rejected_instead_of_treated_as_missing() {
        let directory = temporary_directory();
        let path = directory.join("dangling.env");
        symlink(directory.join("absent-target.env"), &path)
            .expect("dangling symlink should be created");
        assert_eq!(
            RuntimeConfig::load_from_source(
                ConfigSource::OptionalDefault(path),
                &complete_overrides()
            )
            .err(),
            Some(JblError::InvalidConfig)
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_config_under_symlinked_parent_is_rejected() {
        let directory = temporary_directory();
        let real_parent = directory.join("real");
        let linked_parent = directory.join("linked");
        fs::create_dir(&real_parent).expect("real parent should be created");
        let config_path = real_parent.join("devices.env");
        fs::write(&config_path, b"# private fixture\n").expect("fixture should be written");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("fixture mode should be set");
        symlink(&real_parent, &linked_parent).expect("parent symlink should be created");

        assert_eq!(
            RuntimeConfig::load_from_source(
                ConfigSource::OptionalDefault(linked_parent.join("devices.env")),
                &complete_overrides()
            )
            .err(),
            Some(JblError::InvalidConfig)
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }
}
