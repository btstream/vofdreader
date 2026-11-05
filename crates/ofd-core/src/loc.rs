use crate::error::{OfdError, Result};

/// Normalize an in-package path (`ST_Loc`) to an entry name.
///
/// - always uses `/`
/// - removes `.` and empty segments
/// - resolves `..` without allowing escape above the package root
pub fn normalize_loc(path: &str) -> Result<String> {
    let path = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(OfdError::InvalidPath(path.to_string()));
                }
            }
            other => parts.push(other),
        }
    }

    if parts.is_empty() {
        return Err(OfdError::InvalidPath(path.to_string()));
    }

    Ok(parts.join("/"))
}

/// Resolve a path relative to the directory containing `base_file`.
/// If `loc` starts with `/`, it is resolved from the package root.
pub fn resolve_loc(base_file: &str, loc: &str) -> Result<String> {
    if let Some(root_relative) = loc.strip_prefix('/') {
        return normalize_loc(root_relative);
    }

    let dir = base_file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    if dir.is_empty() {
        normalize_loc(loc)
    } else {
        normalize_loc(&format!("{dir}/{loc}"))
    }
}

/// Join a file name to an already resolved directory.
pub fn join_dir(base_dir: &str, loc: &str) -> Result<String> {
    if loc.starts_with('/') {
        if loc.trim_matches('/').is_empty() {
            return Ok(String::new());
        }
        return normalize_loc(loc);
    }
    if base_dir.is_empty() {
        normalize_loc(loc)
    } else {
        normalize_loc(&format!("{base_dir}/{loc}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_to_document() {
        assert_eq!(
            resolve_loc("Doc_0/Document.xml", "Pages/Page_0/Content.xml").unwrap(),
            "Doc_0/Pages/Page_0/Content.xml"
        );
        assert_eq!(
            resolve_loc("OFD.xml", "Doc_0/Document.xml").unwrap(),
            "Doc_0/Document.xml"
        );
        assert_eq!(
            resolve_loc("Doc_0/DocumentRes.xml", "Res").unwrap(),
            "Doc_0/Res"
        );
        assert_eq!(
            join_dir("Doc_0/Res", "image_0.png").unwrap(),
            "Doc_0/Res/image_0.png"
        );
    }

    #[test]
    fn root_base_loc_resolves_to_package_root() {
        assert_eq!(join_dir("Doc_0", "/").unwrap(), "");
        assert_eq!(join_dir("", "/").unwrap(), "");
        assert_eq!(join_dir("", "/Doc_0/Res/2.gif").unwrap(), "Doc_0/Res/2.gif");
    }

    #[test]
    fn normalizes_dots() {
        assert_eq!(normalize_loc("./a//b/./c").unwrap(), "a/b/c");
        assert_eq!(normalize_loc("a/b/../c").unwrap(), "a/c");
        assert_eq!(normalize_loc("/a/b").unwrap(), "a/b");
    }

    #[test]
    fn rejects_escape() {
        assert!(normalize_loc("../OFD.xml").is_err());
        assert!(normalize_loc("a/../../OFD.xml").is_err());
    }
}
