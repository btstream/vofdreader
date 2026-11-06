use std::path::{Path, PathBuf};

const RECENT_FILE: &str = "recent.txt";
const CONFIG_DIR: &str = "vofdreader";

/// Moves `path` to the front, removes duplicates, and caps the list.
pub fn push_recent(paths: &mut Vec<PathBuf>, path: &Path, limit: usize) {
    paths.retain(|existing| existing != path);
    paths.insert(0, path.to_path_buf());
    paths.truncate(limit);
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(CONFIG_DIR).join(RECENT_FILE))
}

/// Loads recent paths, newest first. Missing files are skipped silently.
pub fn load_recent(limit: usize) -> Vec<PathBuf> {
    let mut paths = recent_file()
        .map(|file| load_from(&file, limit))
        .unwrap_or_default();
    paths.retain(|path| path.exists());
    paths
}

fn load_from(file: &Path, limit: usize) -> Vec<PathBuf> {
    let Ok(contents) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        paths.push(PathBuf::from(line));
    }
    paths.truncate(limit);
    paths
}

/// Best-effort persistence; recent files must never block opening a document.
pub fn save_recent(paths: &[PathBuf], limit: usize) {
    if let Some(file) = recent_file() {
        save_to(&file, paths, limit);
    }
}

fn save_to(file: &Path, paths: &[PathBuf], limit: usize) {
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut contents = String::new();
    for path in paths.iter().take(limit) {
        if let Some(text) = path.to_str()
            && !text.contains('\n')
            && !text.contains('\r')
        {
            contents.push_str(text);
            contents.push('\n');
        }
    }
    let _ = std::fs::write(file, contents);
}

#[cfg(test)]
mod tests;
