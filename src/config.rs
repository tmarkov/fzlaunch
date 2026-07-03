use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::Value;

const DEFAULT_CONFIG_TEXT: &str = r#"[sources.path]
enabled = true
direct_action = "{}"
preview_command = "man {}"

[sources.filesystem]
enabled = true
direct_action = "xdg-open {}"
directory_preview_command = "ls {}"
text_file_preview_command = "cat {}"
"#;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub sources: SourceConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceConfig {
    pub path: PathSourceConfig,
    pub filesystem: FilesystemSourceConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSourceConfig {
    pub enabled: bool,
    pub direct_action: Value,
    pub preview_command: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemSourceConfig {
    pub enabled: bool,
    pub direct_action: Value,
    pub directory_preview_command: Value,
    pub text_file_preview_command: Value,
}

#[derive(Debug)]
pub enum ConfigError {
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };

        load_or_create_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let file = toml::from_str::<ConfigFile>(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

        Ok(Self::from(file))
    }
}

fn load_or_create_from_path(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        write_default_config(path)?;
        return Ok(Config::default());
    }

    Config::load_from_path(path)
}

fn write_default_config(path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    fs::write(path, DEFAULT_CONFIG_TEXT).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

impl Default for PathSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            direct_action: Value::raw("{}"),
            preview_command: Value::raw("man {}"),
        }
    }
}

impl Default for FilesystemSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            direct_action: Value::raw("xdg-open {}"),
            directory_preview_command: Value::raw("ls {}"),
            text_file_preview_command: Value::raw("cat {}"),
        }
    }
}

impl From<ConfigFile> for Config {
    fn from(file: ConfigFile) -> Self {
        let defaults = Self::default();
        let Some(sources) = file.sources else {
            return defaults;
        };

        Self {
            sources: SourceConfig {
                path: sources
                    .path
                    .map(PathSourceConfig::from)
                    .unwrap_or(defaults.sources.path),
                filesystem: sources
                    .filesystem
                    .map(FilesystemSourceConfig::from)
                    .unwrap_or(defaults.sources.filesystem),
            },
        }
    }
}

impl From<PathSourceConfigFile> for PathSourceConfig {
    fn from(file: PathSourceConfigFile) -> Self {
        let defaults = Self::default();

        Self {
            enabled: file.enabled.unwrap_or(defaults.enabled),
            direct_action: file
                .direct_action
                .map(Value::raw)
                .unwrap_or(defaults.direct_action),
            preview_command: file
                .preview_command
                .map(Value::raw)
                .unwrap_or(defaults.preview_command),
        }
    }
}

impl From<FilesystemSourceConfigFile> for FilesystemSourceConfig {
    fn from(file: FilesystemSourceConfigFile) -> Self {
        let defaults = Self::default();

        Self {
            enabled: file.enabled.unwrap_or(defaults.enabled),
            direct_action: file
                .direct_action
                .map(Value::raw)
                .unwrap_or(defaults.direct_action),
            directory_preview_command: file
                .directory_preview_command
                .map(Value::raw)
                .unwrap_or(defaults.directory_preview_command),
            text_file_preview_command: file
                .text_file_preview_command
                .map(Value::raw)
                .unwrap_or(defaults.text_file_preview_command),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, source } => {
                write!(
                    formatter,
                    "failed to create config directory {}: {source}",
                    path.display()
                )
            }
            Self::Read { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(formatter, "failed to parse {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(formatter, "failed to write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDir { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    sources: Option<SourceConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
struct SourceConfigFile {
    path: Option<PathSourceConfigFile>,
    filesystem: Option<FilesystemSourceConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
struct PathSourceConfigFile {
    enabled: Option<bool>,
    direct_action: Option<String>,
    preview_command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FilesystemSourceConfigFile {
    enabled: Option<bool>,
    direct_action: Option<String>,
    directory_preview_command: Option<String>,
    text_file_preview_command: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    config_path_from_env(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn config_path_from_env(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let config_home = xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(PathBuf::from).map(|path| path.join(".config")))?;

    Some(config_home.join("fzlaunch").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn default_config_preserves_current_source_settings() {
        assert_eq!(
            Config::default(),
            Config {
                sources: SourceConfig {
                    path: PathSourceConfig {
                        enabled: true,
                        direct_action: Value::raw("{}"),
                        preview_command: Value::raw("man {}"),
                    },
                    filesystem: FilesystemSourceConfig {
                        enabled: true,
                        direct_action: Value::raw("xdg-open {}"),
                        directory_preview_command: Value::raw("ls {}"),
                        text_file_preview_command: Value::raw("cat {}"),
                    },
                },
            }
        );
    }

    #[test]
    fn config_path_uses_xdg_config_home() {
        assert_eq!(
            config_path_from_env(
                Some(OsStr::new("/tmp/config")),
                Some(OsStr::new("/home/me"))
            ),
            Some(PathBuf::from("/tmp/config/fzlaunch/config.toml"))
        );
    }

    #[test]
    fn config_path_falls_back_to_home_config() {
        assert_eq!(
            config_path_from_env(None, Some(OsStr::new("/home/me"))),
            Some(PathBuf::from("/home/me/.config/fzlaunch/config.toml"))
        );
    }

    #[test]
    fn config_path_ignores_relative_xdg_config_home() {
        assert_eq!(
            config_path_from_env(Some(OsStr::new("relative")), Some(OsStr::new("/home/me"))),
            Some(PathBuf::from("/home/me/.config/fzlaunch/config.toml"))
        );
    }

    #[test]
    fn config_file_overrides_source_settings() {
        let dir = TempDir::new("config-source-settings");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            r#"
[sources.path]
enabled = false
direct_action = "run-command {}"
preview_command = "help-command {}"

[sources.filesystem]
enabled = false
direct_action = "open-path {}"
directory_preview_command = "list-path {}"
text_file_preview_command = "show-text {}"
"#,
        )
        .expect("test config should be written");

        assert_eq!(
            Config::load_from_path(&path).expect("test config should parse"),
            Config {
                sources: SourceConfig {
                    path: PathSourceConfig {
                        enabled: false,
                        direct_action: Value::raw("run-command {}"),
                        preview_command: Value::raw("help-command {}"),
                    },
                    filesystem: FilesystemSourceConfig {
                        enabled: false,
                        direct_action: Value::raw("open-path {}"),
                        directory_preview_command: Value::raw("list-path {}"),
                        text_file_preview_command: Value::raw("show-text {}"),
                    },
                },
            }
        );
    }

    #[test]
    fn missing_config_fields_keep_defaults() {
        let dir = TempDir::new("config-partial-settings");
        let path = dir.join("config.toml");
        fs::write(
            &path,
            r#"
[sources.path]
preview_command = "help {}"
"#,
        )
        .expect("test config should be written");

        assert_eq!(
            Config::load_from_path(&path).expect("test config should parse"),
            Config {
                sources: SourceConfig {
                    path: PathSourceConfig {
                        preview_command: Value::raw("help {}"),
                        ..PathSourceConfig::default()
                    },
                    filesystem: FilesystemSourceConfig::default(),
                },
            }
        );
    }

    #[test]
    fn missing_config_file_is_created_with_defaults() {
        let dir = TempDir::new("config-create-default");
        let path = dir.join("fzlaunch").join("config.toml");

        assert_eq!(
            load_or_create_from_path(&path).expect("missing config should be created"),
            Config::default()
        );
        assert_eq!(
            fs::read_to_string(&path).expect("default config should be written"),
            DEFAULT_CONFIG_TEXT
        );
        assert_eq!(
            Config::load_from_path(&path).expect("written config should parse"),
            Config::default()
        );
    }
}
