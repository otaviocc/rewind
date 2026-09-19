//! Writing a cache file so a reader never sees a partial one: tmp, `sync_all`, rename. No new
//! dependency — plain `std::fs`.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const TMP_EXTENSION: &str = "tmp";

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

pub fn clean_stray_tmp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some(TMP_EXTENSION) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    path.extension().map_or_else(
        || path.with_extension(TMP_EXTENSION),
        |extension| {
            let mut name = extension.to_os_string();
            name.push(".");
            name.push(TMP_EXTENSION);
            path.with_extension(name)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn a_write_creates_the_final_file_with_the_given_bytes() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("shard.bin");

        write(&path, b"hello").expect("a successful write");

        assert_eq!(fs::read(&path).expect("a readable file"), b"hello");
    }

    #[test]
    fn a_successful_write_leaves_no_tmp_file_behind() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("shard.bin");

        write(&path, b"hello").expect("a successful write");

        assert!(!tmp_path(&path).exists(), "the tmp file must be gone after a successful rename");
    }

    #[test]
    fn a_write_replaces_an_existing_file_atomically() {
        let dir = TempDir::new().expect("a temporary directory");
        let path = dir.path().join("shard.bin");

        write(&path, b"first").expect("a first write");
        write(&path, b"second").expect("a second write");

        assert_eq!(fs::read(&path).expect("a readable file"), b"second");
    }

    #[test]
    fn a_stray_tmp_file_is_removed_and_everything_else_is_left_alone() {
        let dir = TempDir::new().expect("a temporary directory");
        fs::write(dir.path().join("shard.bin.tmp"), b"stale").expect("a stray tmp file");
        fs::write(dir.path().join("shard.bin"), b"current").expect("a current file");
        fs::write(dir.path().join("meta.json"), b"{}").expect("a non-tmp file");

        clean_stray_tmp_files(dir.path());

        assert!(!dir.path().join("shard.bin.tmp").exists());
        assert!(dir.path().join("shard.bin").exists());
        assert!(dir.path().join("meta.json").exists());
    }

    #[test]
    fn cleaning_a_missing_directory_does_not_panic() {
        clean_stray_tmp_files(Path::new("/nonexistent-rewind-atomic-test"));
    }
}
