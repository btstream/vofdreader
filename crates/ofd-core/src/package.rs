use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use zip::ZipArchive;

use crate::error::{OfdError, Result};
use crate::loc::normalize_loc;

/// Safety limits for untrusted packages.
const MAX_ENTRIES: usize = 10_000;
const MAX_ENTRY_SIZE: u64 = 200 * 1024 * 1024;
const MAX_TOTAL_SIZE: u64 = 1024 * 1024 * 1024;
/// Templates and annotation XML are read again for every page. Cache small
/// entries only; large images and fonts must not inflate process memory.
const ENTRY_CACHE_BYTES: usize = 16 * 1024 * 1024;
const ENTRY_CACHE_MAX_ENTRY: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct EntryCache {
    entries: HashMap<String, Arc<Vec<u8>>>,
    order: VecDeque<String>,
    bytes: usize,
}

/// A read-only view over an OFD ZIP container.
///
/// The archive itself is not kept open, so `Package` is `Send + Sync` and can
/// be shared with background workers. Each `read` opens the file again; this is
/// deliberately simple for v0.1 and will be replaced by a pooled reader when
/// page rendering is added.
pub struct Package {
    path: PathBuf,
    data: Option<Arc<[u8]>>,
    /// Exact entry name -> entry name (kept for future case diagnostics).
    exact: HashMap<String, String>,
    /// Lowercase entry name -> entry name, for tolerant lookup.
    lower: HashMap<String, String>,
    entries: Vec<String>,
    cache: Mutex<EntryCache>,
}

impl Package {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        Self::index(path, None, ZipArchive::new(file)?)
    }

    pub(crate) fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let data: Arc<[u8]> = data.into();
        let archive = ZipArchive::new(Cursor::new(data.clone()))?;
        Self::index(PathBuf::from("embedded-seal.ofd"), Some(data), archive)
    }

    fn index<R: Read + Seek>(
        path: PathBuf,
        data: Option<Arc<[u8]>>,
        mut archive: ZipArchive<R>,
    ) -> Result<Self> {
        if archive.len() > MAX_ENTRIES {
            return Err(OfdError::LimitExceeded("ZIP 条目数量"));
        }

        let mut exact = HashMap::new();
        let mut lower = HashMap::new();
        let mut entries = Vec::new();
        let mut total: u64 = 0;

        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }

            let raw_name = entry.name()?.trim_start_matches('/').replace('\\', "/");
            if raw_name.is_empty() {
                continue;
            }
            let Some(name) = normalize_loc(&raw_name).ok() else {
                continue;
            };

            if entry.size() > MAX_ENTRY_SIZE {
                return Err(OfdError::LimitExceeded("单个条目大小"));
            }
            total = total.saturating_add(entry.size());
            if total > MAX_TOTAL_SIZE {
                return Err(OfdError::LimitExceeded("解压后总大小"));
            }

            entries.push(name.clone());
            exact.entry(name.clone()).or_insert_with(|| name.clone());
            lower.entry(name.to_lowercase()).or_insert(name);
        }

        Ok(Self {
            path,
            data,
            exact,
            lower,
            entries,
            cache: Mutex::new(EntryCache::default()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn contains(&self, entry: &str) -> bool {
        let Ok(normalized) = normalize_loc(entry) else {
            return false;
        };
        self.exact.contains_key(&normalized)
    }

    /// Resolve an entry name with an exact match first, then a case-insensitive
    /// fallback. Real-world OFD packages occasionally use inconsistent casing.
    pub fn resolve(&self, entry: &str) -> Result<String> {
        let normalized = normalize_loc(entry)?;
        if let Some(found) = self.exact.get(&normalized) {
            return Ok(found.clone());
        }
        if let Some(found) = self.lower.get(&normalized.to_lowercase()) {
            return Ok(found.clone());
        }
        Err(OfdError::EntryNotFound(normalized))
    }

    /// Find the package entry point. The standard requires `OFD.xml` at the
    /// root, but some producers wrap the package in a directory.
    pub fn root_entry(&self) -> Result<String> {
        if self.contains("OFD.xml") {
            return Ok("OFD.xml".to_string());
        }

        let mut candidates: Vec<&String> = self
            .entries
            .iter()
            .filter(|name| {
                name.rsplit('/')
                    .next()
                    .is_some_and(|file| file.eq_ignore_ascii_case("OFD.xml"))
            })
            .collect();
        candidates.sort_by_key(|name| name.matches('/').count());

        candidates
            .first()
            .map(|name| (*name).clone())
            .ok_or_else(|| OfdError::EntryNotFound("OFD.xml".to_string()))
    }

    /// Reads a small entry through the shared cache. XML files are parsed once
    /// per page today, so templates and annotations would otherwise be
    /// decompressed again for every page in the document.
    pub fn read_shared(&self, entry: &str) -> Result<Arc<Vec<u8>>> {
        let resolved = self.resolve(entry)?;
        {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(data) = cache.entries.get(&resolved).cloned() {
                cache.order.retain(|name| name != &resolved);
                cache.order.push_back(resolved.clone());
                return Ok(data);
            }
        }

        let data = Arc::new(self.read_uncached(&resolved)?);
        if data.len() <= ENTRY_CACHE_MAX_ENTRY {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !cache.entries.contains_key(&resolved) {
                cache.bytes += data.len();
                cache.entries.insert(resolved.clone(), data.clone());
                cache.order.push_back(resolved);
                while cache.bytes > ENTRY_CACHE_BYTES {
                    let Some(oldest) = cache.order.pop_front() else {
                        break;
                    };
                    if let Some(removed) = cache.entries.remove(&oldest) {
                        cache.bytes = cache.bytes.saturating_sub(removed.len());
                    }
                }
            }
        }
        Ok(data)
    }

    pub fn read(&self, entry: &str) -> Result<Vec<u8>> {
        let resolved = self.resolve(entry)?;
        self.read_uncached(&resolved)
    }

    fn read_uncached(&self, resolved: &str) -> Result<Vec<u8>> {
        if let Some(data) = &self.data {
            return Self::read_archive(ZipArchive::new(Cursor::new(data.clone()))?, resolved);
        }
        let file = File::open(&self.path)?;
        Self::read_archive(ZipArchive::new(file)?, resolved)
    }

    fn read_archive<R: Read + Seek>(mut archive: ZipArchive<R>, resolved: &str) -> Result<Vec<u8>> {
        let mut archive_entry = archive
            .by_name(resolved)
            .map_err(|_| OfdError::EntryNotFound(resolved.to_string()))?;

        if archive_entry.size() > MAX_ENTRY_SIZE {
            return Err(OfdError::LimitExceeded("单个条目大小"));
        }

        let mut data = Vec::with_capacity(archive_entry.size().min(8 * 1024 * 1024) as usize);
        let mut limited = archive_entry.by_ref().take(MAX_ENTRY_SIZE + 1);
        limited.read_to_end(&mut data)?;

        if data.len() as u64 > MAX_ENTRY_SIZE {
            return Err(OfdError::LimitExceeded("单个条目大小"));
        }

        Ok(data)
    }
}
