//! Writes that never leave a half-written file behind.
//!
//! `fs::write` truncates the target and then fills it, so a crash, a kill or a full disk in
//! between leaves a short file that still has the right name. For carry-over output (JSON,
//! models, recipes) that is a corrupt mod that looks fine. Here the bytes go to a sibling
//! temporary file, are flushed to disk, and only then renamed over the target: a reader sees
//! the old file or the whole new one, never part of either.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_beside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// Writes `bytes` to `path` atomically, creating its folder first.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = temp_beside(path);
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    Ok(())
}

/// Like [`write_atomic`], but first checks that `bytes` are valid JSON when `path` ends in
/// `.json` or `.mcmeta`, so carry-over never writes a JSON file that the game cannot read.
pub fn write_checked(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(ext, "json" | "mcmeta") {
        serde_json::from_slice::<serde::de::IgnoredAny>(bytes)
            .map_err(|e| format!("{}: not valid JSON, not written ({e})", path.display()))?;
    }
    write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hpp-safe-io-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_and_replaces_without_leftovers() {
        let dir = scratch("replace");
        let file = dir.join("a/b/c.json");
        write_atomic(&file, b"{\"a\":1}").unwrap();
        write_atomic(&file, b"{\"a\":2}").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"{\"a\":2}");
        let names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["c.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_json_keeps_the_old_file() {
        let dir = scratch("invalid");
        let file = dir.join("model.json");
        write_checked(&file, b"{\"ok\":true}").unwrap();
        let err = write_checked(&file, b"{\"ok\":").unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(fs::read(&file).unwrap(), b"{\"ok\":true}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_files_are_not_parsed() {
        let dir = scratch("other");
        write_checked(&dir.join("data.nbt"), b"\x0a\x00\x00").unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
