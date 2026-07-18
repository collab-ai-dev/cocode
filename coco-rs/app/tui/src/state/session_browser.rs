//! Saved-session picker model and content-search merge policy.

/// Session browser state (list of saved sessions).
#[derive(Debug, Clone)]
pub struct SessionBrowserState {
    pub sessions: Vec<SessionOption>,
    pub filter: String,
    pub selected: i32,
    pub current_cwd: String,
    pub content_hits: std::collections::HashMap<String, String>,
    pub is_searching: bool,
    /// Process-unique identity of the currently requested content search.
    pub search_request_id: u64,
}

/// A selectable session option.
#[derive(Debug, Clone)]
pub struct SessionOption {
    pub id: String,
    pub label: String,
    pub message_count: i32,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub cwd: String,
    pub first_prompt: String,
    pub last_message_preview: Option<String>,
}

impl SessionBrowserState {
    pub(crate) fn next_search_request_id() -> u64 {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn display_sessions(&self) -> Vec<&SessionOption> {
        let query = self.filter.to_lowercase();
        let mut current = Vec::new();
        let mut groups = std::collections::BTreeMap::<&str, Vec<&SessionOption>>::new();
        for session in self.sessions.iter().filter(|session| {
            query.is_empty()
                || session.label.to_lowercase().contains(&query)
                || session.cwd.to_lowercase().contains(&query)
                || session.first_prompt.to_lowercase().contains(&query)
                || session
                    .last_message_preview
                    .as_deref()
                    .is_some_and(|preview| preview.to_lowercase().contains(&query))
                || self.content_hits.contains_key(&session.id)
        }) {
            if session.cwd == self.current_cwd {
                current.push(session);
            } else {
                groups.entry(&session.cwd).or_default().push(session);
            }
        }
        current.extend(groups.into_values().flatten());
        current
    }

    pub(crate) fn reset_content_search(&mut self) {
        self.content_hits.clear();
        self.is_searching = !self.filter.is_empty();
        self.search_request_id = Self::next_search_request_id();
        self.selected = 0;
    }

    pub(crate) fn apply_content_hits(
        &mut self,
        hits: impl IntoIterator<Item = (String, String)>,
        complete: bool,
    ) {
        let selected_id = self
            .display_sessions()
            .get(self.selected.max(0) as usize)
            .map(|session| session.id.clone());
        self.content_hits.extend(hits);
        self.is_searching = !complete;
        if let Some(selected_id) = selected_id
            && let Some(index) = self
                .display_sessions()
                .iter()
                .position(|session| session.id == selected_id)
        {
            self.selected = i32::try_from(index).unwrap_or(i32::MAX);
        }
    }
}
