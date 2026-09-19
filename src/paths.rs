//! Where things live: the Claude Code directory, and our own config and cache.

use std::env;
use std::path::PathBuf;

use thiserror::Error;

const APP: &str = "rewind";

#[derive(Debug, Error)]
pub enum PathError {
    #[error("cannot find a home directory; set HOME or pass --claude-dir")]
    NoHome,
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
}

pub fn claude_dir(override_dir: Option<PathBuf>) -> Result<PathBuf, PathError> {
    let dir = match override_dir {
        Some(dir) => dir,
        None => home().ok_or(PathError::NoHome)?.join(".claude"),
    };
    if dir.is_dir() { Ok(dir) } else { Err(PathError::NotADirectory(dir)) }
}

pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(var("XDG_CONFIG_HOME"), home(), var("APPDATA"))
}

fn config_dir_from(xdg: Option<PathBuf>, home: Option<PathBuf>, appdata: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(xdg) = present(xdg) {
        return Some(xdg.join(APP));
    }
    if let Some(home) = present(home) {
        return Some(home.join(".config").join(APP));
    }
    present(appdata).map(|appdata| appdata.join(APP))
}

pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(var("XDG_CACHE_HOME"), home())
}

fn cache_dir_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(xdg) = present(xdg) {
        return Some(xdg.join(APP));
    }
    present(home).map(|home| home.join(".cache").join(APP))
}

fn present(dir: Option<PathBuf>) -> Option<PathBuf> {
    dir.filter(|dir| !dir.as_os_str().is_empty())
}

fn var(name: &str) -> Option<PathBuf> {
    present(env::var_os(name).map(PathBuf::from))
}

fn home() -> Option<PathBuf> {
    var("HOME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_that_is_not_a_directory_is_an_error() {
        let err = claude_dir(Some(PathBuf::from("/nonexistent/rewind-test"))).unwrap_err();
        assert!(matches!(err, PathError::NotADirectory(_)));
    }

    #[test]
    fn the_config_directory_prefers_xdg_config_home() {
        let dir = config_dir_from(
            Some(PathBuf::from("/xdg")),
            Some(PathBuf::from("/home/someone")),
            Some(PathBuf::from(r"C:\Users\someone\AppData")),
        );
        assert_eq!(dir, Some(PathBuf::from("/xdg/rewind")));
    }

    #[test]
    fn the_config_directory_is_dot_config_under_home_and_never_library() {
        let dir = config_dir_from(None, Some(PathBuf::from("/Users/someone")), None).expect("a home directory is enough");
        assert_eq!(dir, PathBuf::from("/Users/someone/.config/rewind"));
        assert!(!dir.to_string_lossy().contains("Library"), "{dir:?} went to ~/Library");
    }

    #[test]
    fn appdata_is_the_config_directory_only_when_there_is_no_home() {
        let appdata = PathBuf::from(r"C:\Users\someone\AppData");
        assert_eq!(config_dir_from(None, None, Some(appdata.clone())), Some(appdata.join(APP)));
    }

    #[test]
    fn there_is_no_config_directory_when_the_environment_says_nothing() {
        assert_eq!(config_dir_from(None, None, None), None);
    }

    #[test]
    fn an_empty_variable_is_skipped_rather_than_joined_onto() {
        let dir = config_dir_from(Some(PathBuf::from("")), Some(PathBuf::from("/home/someone")), None);
        assert_eq!(dir, Some(PathBuf::from("/home/someone/.config/rewind")), "an empty XDG_CONFIG_HOME was treated as set");
        assert_eq!(config_dir_from(Some(PathBuf::from("")), Some(PathBuf::from("")), Some(PathBuf::from(""))), None);
    }

    #[test]
    fn the_cache_directory_prefers_xdg_cache_home() {
        let dir = cache_dir_from(Some(PathBuf::from("/xdg-cache")), Some(PathBuf::from("/home/someone")));
        assert_eq!(dir, Some(PathBuf::from("/xdg-cache/rewind")));
    }

    #[test]
    fn the_cache_directory_is_dot_cache_under_home() {
        let dir = cache_dir_from(None, Some(PathBuf::from("/Users/someone"))).expect("a home directory is enough");
        assert_eq!(dir, PathBuf::from("/Users/someone/.cache/rewind"));
    }

    #[test]
    fn there_is_no_cache_directory_when_the_environment_says_nothing() {
        assert_eq!(cache_dir_from(None, None), None);
    }

    #[test]
    fn an_empty_cache_variable_is_skipped_rather_than_joined_onto() {
        let dir = cache_dir_from(Some(PathBuf::from("")), Some(PathBuf::from("/home/someone")));
        assert_eq!(dir, Some(PathBuf::from("/home/someone/.cache/rewind")), "an empty XDG_CACHE_HOME was treated as set");
        assert_eq!(cache_dir_from(Some(PathBuf::from("")), Some(PathBuf::from(""))), None);
    }
}
