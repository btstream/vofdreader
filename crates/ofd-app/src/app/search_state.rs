//! Search state owned by the reader.
//!
//! Split from `app/search.rs`, which holds the behaviour: queries, hit
//! navigation, highlight recomputation and the sidebar input widget.

use crate::*;

#[derive(Clone)]
pub(crate) struct SearchHit {
    pub(crate) page: usize,
    /// Ordinal of this match within the page's `page_search_rects` list.
    pub(crate) match_index: usize,
    pub(crate) snippet: String,
}

/// What an inactive tab keeps from a search session.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct SearchSnapshot {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) index: usize,
}

pub(crate) struct SearchState {
    pub(crate) open: bool,
    /// Created on first use because it needs a `Window`.
    pub(crate) input: Option<Entity<InputState>>,
    pub(crate) query: String,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) index: usize,
    pub(crate) results_scroll: ScrollHandle,
    pub(crate) highlights: Vec<search_highlight::Highlight>,
    pub(crate) active_highlight: Option<search_highlight::Highlight>,
    /// Guards the input subscription while the value is written from code.
    pub(crate) suppress_change: bool,
    /// Query and index to restore once the tab's hits have been recomputed.
    pub(crate) pending_index: Option<(String, usize)>,
    /// Input value to write once a window is available.
    pub(crate) pending_input: Option<String>,
}

impl SearchState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            input: None,
            query: String::new(),
            hits: Vec::new(),
            index: 0,
            results_scroll: ScrollHandle::new(),
            highlights: Vec::new(),
            active_highlight: None,
            suppress_change: false,
            pending_index: None,
            pending_input: None,
        }
    }

    /// Drops results and on-page highlights, keeping the input widget.
    pub(crate) fn clear_results(&mut self) {
        self.hits.clear();
        self.index = 0;
        self.highlights.clear();
        self.active_highlight = None;
        self.pending_index = None;
        self.pending_input = None;
    }

    /// Returns to a closed search with no residual query.
    pub(crate) fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.clear_results();
    }

    pub(crate) fn snapshot(&self) -> SearchSnapshot {
        SearchSnapshot {
            open: self.open,
            query: self.query.clone(),
            index: self.index,
        }
    }

    /// Restores a tab's snapshot before the document is reopened. Hits are
    /// recomputed from the query by `restore_tab_search`.
    pub(crate) fn apply_snapshot(&mut self, snapshot: &SearchSnapshot) {
        self.open = snapshot.open;
        self.query = snapshot.query.clone();
        self.index = snapshot.index;
    }
}
