//! Ownership of the document currently on screen.
//!
//! Inactive tabs keep only their `DocumentTab` snapshot, so replacing this
//! value is the single place where document identity, page metadata and the
//! per-document raster products are dropped together. Background results
//! compare `generation` and are ignored once it has moved on.

use crate::*;

pub(crate) struct ActiveDocument {
    pub(crate) doc: Option<Arc<OfdDocument>>,
    pub(crate) renderer: Option<Arc<PageRenderer>>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) file_name: String,
    pub(crate) file_size: u64,
    /// Bumped on every open and close; stale async results compare it before
    /// touching any state.
    pub(crate) generation: u64,
    /// Page sizes start from `DocumentMeta::page_area` and are filled in the
    /// background; `page_sizes_revision` invalidates the continuous layout.
    pub(crate) page_sizes: Vec<SizeMm>,
    pub(crate) page_sizes_revision: u64,
    pub(crate) page_text_cache: Option<Vec<Vec<String>>>,
    pub(crate) font_reports: HashMap<usize, Arc<ofd_core::FontRenderReport>>,
    pub(crate) thumbs: Vec<Option<Arc<RenderImage>>>,
    pub(crate) thumbs_started: bool,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

impl ActiveDocument {
    pub(crate) fn empty() -> Self {
        Self {
            doc: None,
            renderer: None,
            path: None,
            file_name: String::new(),
            file_size: 0,
            generation: 0,
            page_sizes: Vec::new(),
            page_sizes_revision: 0,
            page_text_cache: None,
            font_reports: HashMap::new(),
            thumbs: Vec::new(),
            thumbs_started: false,
            loading: false,
            error: None,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.doc.is_some()
    }

    pub(crate) fn page_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |doc| doc.page_count())
    }

    pub(crate) fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Drops everything owned for the previous document. The path and file
    /// name are left alone; [`Self::close`] clears them too.
    fn reset(&mut self) {
        self.next_generation();
        self.doc = None;
        self.renderer = None;
        self.page_sizes.clear();
        self.page_sizes_revision = self.page_sizes_revision.wrapping_add(1);
        self.page_text_cache = None;
        self.font_reports.clear();
        self.thumbs.clear();
        self.thumbs_started = false;
        self.loading = false;
        self.error = None;
    }

    /// Returns to the empty state: no document, no path, no page metadata.
    pub(crate) fn close(&mut self) {
        self.path = None;
        self.file_name.clear();
        self.file_size = 0;
        self.reset();
    }

    /// Takes ownership of a freshly opened document and prepares its page
    /// metadata. Page sizes other than the fallback are filled in later.
    pub(crate) fn adopt(&mut self, doc: Arc<OfdDocument>, path: PathBuf, file_size: u64) {
        let page_count = doc.page_count();
        let fallback = doc.meta.page_area.unwrap_or(SizeMm::new(210.0, 297.0));
        self.reset();
        self.file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.path = Some(path);
        self.file_size = file_size;
        self.page_sizes = vec![fallback; page_count];
        self.thumbs = vec![None; page_count];
        self.doc = Some(doc);
        self.loading = true;
    }

    /// Records a failed open and returns to the empty state.
    pub(crate) fn fail(&mut self, error: String) {
        self.reset();
        self.file_size = 0;
        self.error = Some(error);
    }
}
