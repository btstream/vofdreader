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
mod tests {
    use super::*;

    #[test]
    fn round_trips_recent_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("recent.txt");
        let paths = vec![PathBuf::from("/a.ofd"), PathBuf::from("/b.ofd")];
        save_to(&file, &paths, 10);
        assert_eq!(load_from(&file, 10), paths);
    }

    #[test]
    fn moves_existing_path_to_front() {
        let mut paths = vec![
            PathBuf::from("/a.ofd"),
            PathBuf::from("/b.ofd"),
            PathBuf::from("/c.ofd"),
        ];
        push_recent(&mut paths, Path::new("/b.ofd"), 10);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/b.ofd"),
                PathBuf::from("/a.ofd"),
                PathBuf::from("/c.ofd")
            ]
        );
    }

    #[test]
    fn caps_list_and_keeps_newest() {
        let mut paths = vec![];
        for index in 0..5 {
            push_recent(&mut paths, Path::new(&format!("/{index}.ofd")), 3);
        }
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], PathBuf::from("/4.ofd"));
        assert_eq!(paths[2], PathBuf::from("/2.ofd"));
    }
}
