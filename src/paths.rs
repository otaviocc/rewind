//! Where things live: the Claude Code directory, and our own config and cache.

use std::env;
use std::path::PathBuf;

use thiserror::Error;

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
        None => home()?.join(".claude"),
    };
    if dir.is_dir() { Ok(dir) } else { Err(PathError::NotADirectory(dir)) }
}

fn home() -> Result<PathBuf, PathError> {
    env::var_os("HOME").map(PathBuf::from).filter(|home| !home.as_os_str().is_empty()).ok_or(PathError::NoHome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_that_is_not_a_directory_is_an_error() {
        let err = claude_dir(Some(PathBuf::from("/nonexistent/rewind-test"))).unwrap_err();
        assert!(matches!(err, PathError::NotADirectory(_)));
    }
}
