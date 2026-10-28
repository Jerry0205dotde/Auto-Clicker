use crate::model::{
    ClickType, MAX_INTERVAL_MS, MAX_REPEAT_COUNT, MIN_INTERVAL_MS, MouseButton, PositionMode,
    RepeatMode,
};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

const APP_DIR: &str = "klickmeister";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub interval_ms: u64,
    pub mouse_button: MouseButton,
    pub click_type: ClickType,
    pub repeat_mode: RepeatMode,
    pub repeat_count: u64,
    pub position_mode: PositionMode,
    pub fixed_x: u32,
    pub fixed_y: u32,
    pub hotkey: String,
    pub monitor_identity: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            interval_ms: 100,
            mouse_button: MouseButton::Left,
            click_type: ClickType::Single,
            repeat_mode: RepeatMode::UntilStopped,
            repeat_count: 100,
            position_mode: PositionMode::CurrentCursor,
            fixed_x: 0,
            fixed_y: 0,
            hotkey: "Pause".to_owned(),
            monitor_identity: String::new(),
        }
    }
}

impl AppConfig {
    /// Limits hand-edited values to the ranges the UI can display and start with.
    fn clamp_to_valid_ranges(mut self) -> Self {
        self.interval_ms = self.interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        self.repeat_count = self.repeat_count.clamp(1, MAX_REPEAT_COUNT);
        self
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "Der Speicherort für die Einstellungen fehlt. HOME oder XDG_CONFIG_HOME muss ein absoluter Pfad sein."
    )]
    MissingHome,
    #[error("Einstellungen konnten nicht geladen werden: {0}")]
    Read(#[source] io::Error),
    #[error("Die Einstellungsdatei ist ungültig: {0}")]
    Parse(#[source] toml::de::Error),
    #[error("Einstellungen konnten nicht gespeichert werden: {0}")]
    Write(#[source] io::Error),
    #[error("Einstellungen konnten nicht für das Speichern vorbereitet werden: {0}")]
    Serialize(#[source] toml::ser::Error),
}

pub fn config_path() -> Result<PathBuf, ConfigError> {
    config_path_from_env(
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

fn config_path_from_env(
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, ConfigError> {
    if let Some(base) = xdg_config_home.filter(|value| value.is_absolute()) {
        return Ok(base.join(APP_DIR).join(CONFIG_FILE));
    }
    home.filter(|value| value.is_absolute())
        .map(|home| home.join(".config").join(APP_DIR).join(CONFIG_FILE))
        .ok_or(ConfigError::MissingHome)
}

pub fn load() -> Result<AppConfig, ConfigError> {
    let path = config_path()?;
    load_from(&path)
}

pub fn load_from(path: &Path) -> Result<AppConfig, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => toml::from_str::<AppConfig>(&contents)
            .map(AppConfig::clamp_to_valid_ranges)
            .map_err(ConfigError::Parse),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(error) => Err(ConfigError::Read(error)),
    }
}

pub fn save(config: &AppConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    save_to(&path, config)
}

pub fn save_to(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    let resolved_path = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => Some(fs::canonicalize(path).map_err(|error| {
            ConfigError::Write(io::Error::new(
                error.kind(),
                format!(
                    "Das Linkziel der Einstellungsdatei {} konnte nicht aufgelöst werden: {error}",
                    path.display()
                ),
            ))
        })?),
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(ConfigError::Write(error)),
    };
    let path = resolved_path.as_deref().unwrap_or(path);
    let parent = path.parent().ok_or_else(|| {
        ConfigError::Write(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Der Speicherort hat kein übergeordnetes Verzeichnis.",
        ))
    })?;
    fs::create_dir_all(parent).map_err(ConfigError::Write)?;

    let serialized = toml::to_string_pretty(config).map_err(ConfigError::Serialize)?;
    let temp_path = parent.join(format!(".{CONFIG_FILE}.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let result = (|| -> io::Result<()> {
        let mut file = options.open(&temp_path)?;
        file.write_all(serialized.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(ConfigError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn test_path(name: &str) -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "klickmeister-test-{}-{id}-{name}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn missing_config_uses_defaults() {
        let path = test_path("missing");
        assert_eq!(load_from(&path).ok(), Some(AppConfig::default()));
        assert_eq!(AppConfig::default().hotkey, "Pause");
    }

    #[test]
    fn config_round_trip() {
        let path = test_path("roundtrip");
        let config = AppConfig {
            interval_ms: 250,
            mouse_button: MouseButton::Right,
            click_type: ClickType::Double,
            repeat_mode: RepeatMode::Count,
            repeat_count: 42,
            position_mode: PositionMode::Fixed,
            fixed_x: 640,
            fixed_y: 480,
            hotkey: "F8".to_owned(),
            monitor_identity: "monitor-serial-123".to_owned(),
        };
        assert!(save_to(&path, &config).is_ok());
        assert_eq!(load_from(&path).ok(), Some(config));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_path_requires_absolute_base_directories() {
        for (xdg, home, expected) in [
            (
                Some("/xdg"),
                Some("/home/user"),
                Some("/xdg/klickmeister/config.toml"),
            ),
            (
                Some("relative"),
                Some("/home/user"),
                Some("/home/user/.config/klickmeister/config.toml"),
            ),
            (
                Some(""),
                Some("/home/user"),
                Some("/home/user/.config/klickmeister/config.toml"),
            ),
            (
                None,
                Some("/home/user"),
                Some("/home/user/.config/klickmeister/config.toml"),
            ),
            (
                Some("/xdg"),
                Some("relative"),
                Some("/xdg/klickmeister/config.toml"),
            ),
            (Some("relative"), Some("relative"), None),
            (None, Some("relative"), None),
            (None, None, None),
        ] {
            let result = config_path_from_env(xdg.map(PathBuf::from), home.map(PathBuf::from));
            match expected {
                Some(path) => {
                    assert_eq!(result.ok(), Some(PathBuf::from(path)), "{xdg:?}, {home:?}")
                }
                None => assert!(
                    matches!(result, Err(ConfigError::MissingHome)),
                    "{xdg:?}, {home:?}"
                ),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_symlink_and_atomically_replaces_its_target() {
        use std::os::unix::fs::symlink;

        for relative in [false, true] {
            let directory = test_path("symlink");
            let path = directory.join("config/config.toml");
            let target = directory.join("dotfiles/settings.toml");
            let previous = directory.join("previous.toml");
            assert!(fs::create_dir_all(directory.join("config")).is_ok());
            assert!(fs::create_dir_all(directory.join("dotfiles")).is_ok());
            let original = "interval_ms = 100\n";
            assert!(fs::write(&target, original).is_ok());
            assert!(fs::hard_link(&target, &previous).is_ok());
            let link_target = if relative {
                PathBuf::from("../dotfiles/settings.toml")
            } else {
                target.clone()
            };
            assert!(symlink(&link_target, &path).is_ok());
            let config = AppConfig {
                interval_ms: 250,
                ..AppConfig::default()
            };

            let result = save_to(&path, &config);
            let preserved_link = fs::read_link(&path).ok();
            let loaded_link = load_from(&path).ok();
            let loaded_target = load_from(&target).ok();
            let previous_contents = fs::read_to_string(&previous).ok();
            assert!(fs::remove_dir_all(directory).is_ok());

            assert!(result.is_ok(), "{result:?}");
            assert_eq!(preserved_link, Some(link_target));
            assert_eq!(loaded_link, Some(config.clone()));
            assert_eq!(loaded_target, Some(config));
            assert_eq!(previous_contents.as_deref(), Some(original));
        }
    }

    #[cfg(unix)]
    #[test]
    fn save_reports_a_broken_symlink_without_replacing_it() {
        use std::os::unix::fs::symlink;

        let directory = test_path("broken-symlink");
        assert!(fs::create_dir(&directory).is_ok());
        let path = directory.join("config.toml");
        let target = directory.join("missing.toml");
        assert!(symlink(&target, &path).is_ok());

        let result = save_to(&path, &AppConfig::default());
        let preserved_link = fs::read_link(&path).ok();
        let target_exists = target.exists();
        assert!(fs::remove_dir_all(directory).is_ok());

        assert!(matches!(&result, Err(ConfigError::Write(_))));
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("Linkziel"))
        );
        assert_eq!(preserved_link, Some(target));
        assert!(!target_exists);
    }

    #[test]
    fn legacy_config_has_no_implicit_monitor_identity() {
        let config = toml::from_str::<AppConfig>(
            "position_mode = \"fixed\"\nfixed_x = 640\nfixed_y = 480\n",
        );
        assert!(config.is_ok_and(|config| config.monitor_identity.is_empty()));
    }

    #[test]
    fn clamps_out_of_range_values_on_load() {
        let too_small = test_path("too-small");
        assert!(fs::write(&too_small, "interval_ms = 5\nrepeat_count = 0\n").is_ok());
        let config = load_from(&too_small).ok();
        assert_eq!(
            config.as_ref().map(|config| config.interval_ms),
            Some(MIN_INTERVAL_MS)
        );
        assert_eq!(config.map(|config| config.repeat_count), Some(1));
        let _ = fs::remove_file(too_small);

        let too_large = test_path("too-large");
        let contents = format!(
            "interval_ms = {}\nrepeat_count = {}\n",
            MAX_INTERVAL_MS + 1,
            MAX_REPEAT_COUNT + 1
        );
        assert!(fs::write(&too_large, contents).is_ok());
        let config = load_from(&too_large).ok();
        assert_eq!(
            config.as_ref().map(|config| config.interval_ms),
            Some(MAX_INTERVAL_MS)
        );
        assert_eq!(
            config.map(|config| config.repeat_count),
            Some(MAX_REPEAT_COUNT)
        );
        let _ = fs::remove_file(too_large);

        let at_bounds = test_path("at-bounds");
        for (interval_ms, repeat_count) in
            [(MIN_INTERVAL_MS, 1), (MAX_INTERVAL_MS, MAX_REPEAT_COUNT)]
        {
            let contents = format!("interval_ms = {interval_ms}\nrepeat_count = {repeat_count}\n");
            assert!(fs::write(&at_bounds, contents).is_ok());
            let config = load_from(&at_bounds).ok();
            assert_eq!(
                config.map(|config| (config.interval_ms, config.repeat_count)),
                Some((interval_ms, repeat_count))
            );
        }
        let _ = fs::remove_file(at_bounds);
    }

    #[test]
    fn rejects_unknown_or_wrongly_typed_fields() {
        let unknown = toml::from_str::<AppConfig>("interval_ms = 100\nsecret = true\n");
        assert!(unknown.is_err());
        let wrong_type = toml::from_str::<AppConfig>("interval_ms = \"fast\"\n");
        assert!(wrong_type.is_err());
    }
}
