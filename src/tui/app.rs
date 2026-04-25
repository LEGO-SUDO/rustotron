//! `App` — local state for the TUI task.
//!
//! Owned by the one tokio task that runs the TUI main loop. Never shared
//! across tasks (see ADR-002 §D-3), so it's a plain struct mutated in
//! place. No `Arc<Mutex<App>>`.

use std::collections::HashSet;

use ratatui::widgets::ListState;

use super::event::PaneId;
use super::filter::{FilterState, StatusClass, toggle_in_set};
use super::mouse::HitRegion;
use crate::bus::ClientId;
use crate::store::Request;

/// Collapsible detail-pane section ids. Used as keys in
/// [`App::collapsed_sections`] so toggling persists across scrolls.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum SectionId {
    /// Request Headers block.
    RequestHeaders,
    /// Request Body block.
    RequestBody,
    /// Response Headers block.
    ResponseHeaders,
    /// Response Body block.
    ResponseBody,
}

impl SectionId {
    /// Order in which sections appear in the detail pane. Used by `z` to
    /// pick "the closest section to the scroll cursor".
    #[must_use]
    pub const fn ordered() -> [Self; 4] {
        [
            Self::RequestHeaders,
            Self::RequestBody,
            Self::ResponseHeaders,
            Self::ResponseBody,
        ]
    }
}

/// Summary of the connected Reactotron clients used by the status bar.
#[derive(Debug, Clone, Default)]
pub struct ConnectionState {
    /// Currently connected client ids (most-recent last).
    pub clients: Vec<ClientId>,
    /// Optional human-readable name of the last-connected device,
    /// extracted from its `client.intro.name`. Not populated in v1 —
    /// wire through when the server surfaces it.
    pub last_device: Option<String>,
}

impl ConnectionState {
    /// How many clients are currently connected.
    #[must_use]
    pub fn count(&self) -> usize {
        self.clients.len()
    }

    /// Remove a disconnected client.
    pub fn remove(&mut self, id: ClientId) {
        self.clients.retain(|c| *c != id);
    }

    /// Record a newly connected client (deduplicates).
    pub fn add(&mut self, id: ClientId) {
        if !self.clients.contains(&id) {
            self.clients.push(id);
        }
    }
}

/// TUI local state. Mutated only inside the TUI task.
#[derive(Debug)]
pub struct App {
    /// All rows the store has emitted, oldest → newest. Refreshed from
    /// the store on bus-change events. The *displayed* subset is derived
    /// on demand via [`App::visible_rows`].
    pub rows: Vec<Request>,
    /// Cached filtered rows. Recomputed on any mutation that could
    /// change the outcome (`set_rows`, filter toggle). Kept in sync via
    /// [`App::rebuild_visible`].
    pub visible: Vec<Request>,
    /// Ratatui-native selection state for the list view. Indices are
    /// into the *visible* rows.
    pub list_state: ListState,
    /// Vertical scroll offset for the detail pane.
    pub detail_scroll: u16,
    /// Which pane currently has focus.
    pub focused: PaneId,
    /// Address string shown in the status bar, e.g. `ws://127.0.0.1:9090`.
    pub listen_addr: String,
    /// Live connection state — client ids connected, device names if known.
    pub connections: ConnectionState,
    /// Hit-region table. Cleared at the top of each render; view code
    /// pushes regions for everything clickable on the current frame.
    pub hit_regions: Vec<HitRegion>,
    /// Frame counter. Increments every time state mutates in a way that
    /// needs a repaint; the main loop compares the last-drawn value to
    /// this to skip redundant renders.
    pub dirty_generation: u64,
    /// Most recently drawn generation. Only the main loop touches this.
    pub last_drawn_generation: u64,
    /// Set by the main loop when a terminal resize arrives — forces a
    /// full repaint on the next tick regardless of generation.
    pub force_redraw: bool,
    /// Quit latch. Main loop exits cleanly when this flips to `true`.
    pub should_quit: bool,
    /// When true, the detail pane is in mock/demo mode and the status
    /// bar reflects that instead of a real listen address.
    pub mock_mode: bool,

    // ── TASK-202: detail-pane enhancements ─────────────────────────────
    /// Sections the user has collapsed. Missing ⇒ expanded (default).
    pub collapsed_sections: HashSet<SectionId>,
    /// When true, the detail pane opens a full-view modal that ignores
    /// the 1 MB truncation limit. Toggled by `F`. `Esc` exits.
    pub detail_full_view: bool,

    // ── TASK-203: filter bar state ────────────────────────────────────
    /// Composed filter — URL substring + methods + status classes.
    pub filter: FilterState,
    /// True while the `/url` input field is focused and taking keystrokes.
    pub filter_input_mode: bool,
    /// Draft URL substring being typed; committed to `filter.url_substring`
    /// when the user presses Enter.
    pub filter_draft: String,
    /// When true, incoming bus events do not refresh the row set. The
    /// store keeps committing; only the display is paused.
    pub paused: bool,
    /// When true, the clear-confirmation modal is up. `y` clears, `n` /
    /// `Esc` cancels.
    pub confirm_clear_mode: bool,
    /// Whether crossterm mouse capture is currently enabled. When off,
    /// the terminal handles mouse events natively — most importantly,
    /// text selection and copy work again. Toggle at runtime with `M`.
    pub mouse_capture: bool,
    /// Optional transient toast message — shown overlaid on the status
    /// bar for ~2 s after a user-triggered action (cURL copied,
    /// clipboard unavailable, filters cleared, etc.). Cleared by
    /// [`App::tick_toast`] when it expires.
    pub current_toast: Option<Toast>,
}

/// Short-lived user-facing message shown on top of the status bar after
/// an action. Styled by [`ToastKind`]. Created via
/// [`App::show_toast`] and expired by [`App::tick_toast`].
#[derive(Debug, Clone)]
pub struct Toast {
    /// The line of text to display.
    pub message: String,
    /// Controls colour / accent glyph.
    pub kind: ToastKind,
    /// Wall-clock instant after which the toast should be cleared.
    pub expires_at: std::time::Instant,
}

/// Severity of a toast. Drives the accent glyph + colour in the render
/// path; no behavioural implications.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ToastKind {
    /// Green ✓. Action succeeded.
    Success,
    /// Red ✗. Action failed — usually with a hint about the fallback.
    Error,
    /// Blue ·. Neutral informational message.
    Info,
}

impl ToastKind {
    /// Leading glyph shown before the message text.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Error => "✗",
            Self::Info => "·",
        }
    }
}

/// How long a toast stays visible before auto-clearing. Short enough
/// to not clutter the UI, long enough to read a one-line message.
pub const TOAST_TTL: std::time::Duration = std::time::Duration::from_millis(2200);

impl App {
    /// Construct a freshly-initialised app.
    #[must_use]
    pub fn new(listen_addr: impl Into<String>, mock_mode: bool) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            rows: Vec::new(),
            visible: Vec::new(),
            list_state,
            detail_scroll: 0,
            focused: PaneId::List,
            listen_addr: listen_addr.into(),
            connections: ConnectionState::default(),
            hit_regions: Vec::new(),
            dirty_generation: 1,
            last_drawn_generation: 0,
            force_redraw: true,
            should_quit: false,
            mock_mode,
            collapsed_sections: HashSet::new(),
            detail_full_view: false,
            filter: FilterState::default(),
            filter_input_mode: false,
            filter_draft: String::new(),
            paused: false,
            confirm_clear_mode: false,
            mouse_capture: true,
            current_toast: None,
        }
    }

    /// Post a toast message that will auto-expire after [`TOAST_TTL`].
    /// Replaces any toast currently visible — newer actions always win.
    pub fn show_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.current_toast = Some(Toast {
            message: message.into(),
            kind,
            expires_at: std::time::Instant::now() + TOAST_TTL,
        });
        self.mark_dirty();
    }

    /// Clear the toast if it has expired. Called on every tick by the
    /// TUI main loop so expired toasts disappear without needing a key
    /// press to trigger the check.
    pub fn tick_toast(&mut self) {
        if let Some(t) = self.current_toast.as_ref()
            && std::time::Instant::now() >= t.expires_at
        {
            self.current_toast = None;
            self.mark_dirty();
        }
    }

    /// Flip the mouse-capture state. The main loop compares against the
    /// currently-applied terminal state and issues the crossterm command
    /// on change — see `tui::mod::apply_mouse_capture`.
    ///
    /// Posts a toast so the user sees what changed — the visual delta
    /// is otherwise zero until they try to drag-select or click.
    pub fn toggle_mouse_capture(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        let msg = if self.mouse_capture {
            "mouse capture ON — clicks/scroll active (M for text-select)"
        } else {
            "text-select ON — drag to select, copy w/ terminal (M to restore mouse)"
        };
        self.show_toast(msg, ToastKind::Info);
        self.mark_dirty();
    }

    /// Replace the displayed rows (called on bus events / mock init).
    ///
    /// Preserves the user's selection by id when possible, otherwise
    /// clamps to the new range.
    pub fn set_rows(&mut self, rows: Vec<Request>) {
        let previously_selected_id = self.selected_request().map(|r| r.id);
        self.rows = rows;
        self.rebuild_visible_with_selection(previously_selected_id);
        self.mark_dirty();
    }

    fn rebuild_visible_with_selection(&mut self, preserve_id: Option<crate::bus::RequestId>) {
        self.visible = self
            .rows
            .iter()
            .filter(|r| self.filter.matches(r))
            .cloned()
            .collect();
        let fallback = if self.visible.is_empty() {
            None
        } else {
            Some(0)
        };
        let new_idx = match preserve_id {
            Some(prev) => self.visible.iter().position(|r| r.id == prev),
            None => None,
        }
        .or(fallback);
        self.list_state.select(new_idx);
        self.detail_scroll = 0;
    }

    /// Rebuild the visible slice after a filter mutation. Preserves the
    /// current selection by id when the selected row survives the new
    /// filter, otherwise resets to the first visible row.
    pub fn rebuild_visible(&mut self) {
        let preserve = self.selected_request().map(|r| r.id);
        self.rebuild_visible_with_selection(preserve);
        self.mark_dirty();
    }

    /// Return the currently-displayed (filtered) rows.
    #[must_use]
    pub fn visible_rows(&self) -> &[Request] {
        &self.visible
    }

    /// Return the currently selected row (from the visible slice).
    #[must_use]
    pub fn selected_request(&self) -> Option<&Request> {
        self.list_state.selected().and_then(|i| self.visible.get(i))
    }

    /// Move the selection down by one (saturates at the last row).
    pub fn scroll_list_down(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let next = match self.list_state.selected() {
            Some(i) if i + 1 < self.visible.len() => i + 1,
            Some(i) => i,
            None => 0,
        };
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
        self.mark_dirty();
    }

    /// Move the selection up by one (saturates at the first row).
    pub fn scroll_list_up(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let next = self.list_state.selected().unwrap_or(0).saturating_sub(1);
        self.list_state.select(Some(next));
        self.detail_scroll = 0;
        self.mark_dirty();
    }

    /// Scroll the detail pane down by one line.
    pub fn scroll_detail_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
        self.mark_dirty();
    }

    /// Scroll the detail pane up by one line.
    pub fn scroll_detail_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
        self.mark_dirty();
    }

    /// Focus a specific pane.
    pub fn focus(&mut self, pane: PaneId) {
        if self.focused != pane {
            self.focused = pane;
            self.mark_dirty();
        }
    }

    /// Select the list row at the given index (no-op if out of bounds).
    pub fn select_row(&mut self, index: usize) {
        if index < self.visible.len() {
            self.list_state.select(Some(index));
            self.detail_scroll = 0;
            self.focused = PaneId::List;
            self.mark_dirty();
        }
    }

    /// Toggle whether the named section is collapsed.
    pub fn toggle_section(&mut self, section: SectionId) {
        if self.collapsed_sections.contains(&section) {
            self.collapsed_sections.remove(&section);
        } else {
            self.collapsed_sections.insert(section);
        }
        self.mark_dirty();
    }

    /// Is the named section currently collapsed?
    #[must_use]
    pub fn is_collapsed(&self, section: SectionId) -> bool {
        self.collapsed_sections.contains(&section)
    }

    /// Enter / leave the full-view modal.
    pub fn set_full_view(&mut self, open: bool) {
        if self.detail_full_view != open {
            self.detail_full_view = open;
            self.mark_dirty();
        }
    }

    // ── Filter bar mutations ─────────────────────────────────────────

    /// Begin URL filter input mode. The draft starts from the committed
    /// URL substring so the user can edit it in place.
    pub fn begin_filter_input(&mut self) {
        self.filter_input_mode = true;
        self.filter_draft = self.filter.url_substring.clone();
        self.mark_dirty();
    }

    /// Commit the current draft to the filter. Closes input mode.
    pub fn commit_filter_input(&mut self) {
        self.filter.url_substring = self.filter_draft.clone();
        self.filter_input_mode = false;
        self.rebuild_visible();
    }

    /// Abandon the current draft, leaving the committed value intact.
    pub fn cancel_filter_input(&mut self) {
        self.filter_input_mode = false;
        self.filter_draft.clear();
        self.mark_dirty();
    }

    /// Append a character to the draft.
    pub fn push_filter_char(&mut self, c: char) {
        self.filter_draft.push(c);
        self.mark_dirty();
    }

    /// Remove the last character from the draft.
    pub fn pop_filter_char(&mut self) {
        self.filter_draft.pop();
        self.mark_dirty();
    }

    /// Toggle a method chip.
    pub fn toggle_method(&mut self, method: &str) {
        toggle_in_set(&mut self.filter.methods, method.to_ascii_uppercase());
        self.rebuild_visible();
    }

    /// Toggle a status-class chip.
    pub fn toggle_status_class(&mut self, class: StatusClass) {
        toggle_in_set(&mut self.filter.status_classes, class);
        self.rebuild_visible();
    }

    /// Reset every filter (URL substring, method chips, status chips,
    /// any pending draft) in one shot. Pause state is left untouched —
    /// pausing and filtering are distinct concerns.
    pub fn clear_all_filters(&mut self) {
        let was_active = !self.filter.url_substring.is_empty()
            || !self.filter.methods.is_empty()
            || !self.filter.status_classes.is_empty()
            || self.filter_input_mode
            || !self.filter_draft.is_empty();
        self.filter = FilterState::default();
        self.filter_draft.clear();
        self.filter_input_mode = false;
        self.rebuild_visible();
        if was_active {
            // rebuild_visible already marks dirty; the extra nudge
            // guarantees the status bar re-renders when the only
            // observable change is the filter summary line.
            self.mark_dirty();
        }
    }

    /// Toggle the pause flag.
    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.mark_dirty();
    }

    /// Show the clear-confirmation modal.
    pub fn begin_clear_confirm(&mut self) {
        self.confirm_clear_mode = true;
        self.mark_dirty();
    }

    /// Dismiss the clear-confirmation modal.
    pub fn cancel_clear_confirm(&mut self) {
        if self.confirm_clear_mode {
            self.confirm_clear_mode = false;
            self.mark_dirty();
        }
    }

    /// Bump the dirty generation so the next tick renders.
    pub fn mark_dirty(&mut self) {
        self.dirty_generation = self.dirty_generation.wrapping_add(1);
    }

    /// Should the main loop render this tick?
    #[must_use]
    pub fn needs_redraw(&self) -> bool {
        self.force_redraw || self.dirty_generation != self.last_drawn_generation
    }

    /// Called by the main loop after it successfully draws a frame.
    pub fn mark_drawn(&mut self) {
        self.last_drawn_generation = self.dirty_generation;
        self.force_redraw = false;
    }

    /// Flip the quit latch. The main loop checks this after each event.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};

    fn sample(url: &str) -> Request {
        sample_full(url, "GET", 200)
    }

    fn sample_full(url: &str, method: &str, status: u16) -> Request {
        let exchange = ApiResponsePayload {
            duration: Some(10.0),
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some(method.to_string()),
                data: crate::protocol::Body::null(),
                headers: None,
                params: None,
            },
            response: ApiResponseSide {
                status,
                headers: None,
                body: crate::protocol::Body::null(),
            },
        };
        Request::complete(exchange, None)
    }

    #[test]
    fn new_app_has_no_rows_and_is_dirty() {
        let app = App::new("ws://127.0.0.1:9090", false);
        assert!(app.rows.is_empty());
        assert!(app.visible.is_empty());
        assert_eq!(app.list_state.selected(), Some(0));
        assert!(app.needs_redraw());
    }

    #[test]
    fn set_rows_populates_and_selects_first() {
        let mut app = App::new("", false);
        app.set_rows(vec![sample("/a"), sample("/b")]);
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.visible.len(), 2);
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn set_rows_preserves_selection_by_id_when_still_present() {
        let mut app = App::new("", false);
        let rows = vec![sample("/a"), sample("/b"), sample("/c")];
        let second_id = rows[1].id;
        app.set_rows(rows.clone());
        app.list_state.select(Some(1));
        // Shuffle the order; the row with id `second_id` now moves.
        let mut new_rows = rows.clone();
        new_rows.reverse();
        app.set_rows(new_rows);
        let picked = app.list_state.selected().and_then(|i| app.visible.get(i));
        assert_eq!(picked.map(|r| r.id), Some(second_id));
    }

    #[test]
    fn scroll_list_down_saturates() {
        let mut app = App::new("", false);
        app.set_rows(vec![sample("/a"), sample("/b")]);
        app.scroll_list_down();
        app.scroll_list_down();
        app.scroll_list_down();
        assert_eq!(app.list_state.selected(), Some(1));
    }

    #[test]
    fn scroll_list_up_saturates() {
        let mut app = App::new("", false);
        app.set_rows(vec![sample("/a"), sample("/b")]);
        app.scroll_list_up();
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn mark_drawn_clears_dirty_flag() {
        let mut app = App::new("", false);
        assert!(app.needs_redraw());
        app.mark_drawn();
        assert!(!app.needs_redraw());
    }

    #[test]
    fn scroll_list_on_empty_is_noop() {
        let mut app = App::new("", false);
        let before = app.list_state.selected();
        app.scroll_list_down();
        app.scroll_list_up();
        assert_eq!(app.list_state.selected(), before);
    }

    #[test]
    fn select_row_ignores_out_of_bounds() {
        let mut app = App::new("", false);
        app.set_rows(vec![sample("/a"), sample("/b")]);
        app.select_row(99);
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn toggle_section_persists_until_toggled_back() {
        let mut app = App::new("", false);
        assert!(!app.is_collapsed(SectionId::RequestBody));
        app.toggle_section(SectionId::RequestBody);
        assert!(app.is_collapsed(SectionId::RequestBody));
        app.toggle_section(SectionId::RequestBody);
        assert!(!app.is_collapsed(SectionId::RequestBody));
    }

    #[test]
    fn visible_rows_reflects_method_filter() {
        let mut app = App::new("", false);
        app.set_rows(vec![
            sample_full("/a", "GET", 200),
            sample_full("/b", "POST", 200),
            sample_full("/c", "GET", 200),
        ]);
        assert_eq!(app.visible_rows().len(), 3);
        app.toggle_method("GET");
        assert_eq!(app.visible_rows().len(), 2);
        for r in app.visible_rows() {
            assert_eq!(r.exchange.request.method.as_deref(), Some("GET"));
        }
    }

    #[test]
    fn visible_rows_reflects_status_class_filter() {
        let mut app = App::new("", false);
        app.set_rows(vec![
            sample_full("/a", "GET", 200),
            sample_full("/b", "GET", 404),
            sample_full("/c", "GET", 500),
        ]);
        app.toggle_status_class(StatusClass::ClientError);
        assert_eq!(app.visible_rows().len(), 1);
        assert_eq!(app.visible_rows()[0].exchange.response.status, 404);
    }

    #[test]
    fn visible_rows_composes_url_method_status_with_and() {
        let mut app = App::new("", false);
        app.set_rows(vec![
            sample_full("https://x/login", "POST", 200),
            sample_full("https://x/login", "GET", 200),
            sample_full("https://x/users", "POST", 200),
            sample_full("https://x/login", "POST", 500),
        ]);
        app.toggle_method("POST");
        app.toggle_status_class(StatusClass::Success);
        app.filter.url_substring = "login".to_string();
        app.rebuild_visible();
        assert_eq!(app.visible_rows().len(), 1);
    }

    #[test]
    fn clear_all_filters_resets_url_methods_and_status_but_not_pause() {
        let mut app = App::new("", false);
        app.set_rows(vec![
            sample_full("https://api/login", "GET", 200),
            sample_full("https://api/logout", "POST", 404),
        ]);
        app.toggle_method("POST");
        app.toggle_status_class(StatusClass::ClientError);
        app.filter.url_substring = "log".to_string();
        app.rebuild_visible();
        app.paused = true;
        assert_eq!(app.visible_rows().len(), 1);

        app.clear_all_filters();

        assert!(app.filter.methods.is_empty());
        assert!(app.filter.status_classes.is_empty());
        assert!(app.filter.url_substring.is_empty());
        assert!(app.filter_draft.is_empty());
        assert!(!app.filter_input_mode);
        assert_eq!(
            app.visible_rows().len(),
            2,
            "all rows should be visible after clearing filters"
        );
        assert!(
            app.paused,
            "clear_all_filters must not touch the pause flag"
        );
    }

    #[test]
    fn filter_mutations_bump_dirty_generation() {
        let mut app = App::new("", false);
        app.mark_drawn();
        let before = app.dirty_generation;
        app.toggle_method("POST");
        assert_ne!(app.dirty_generation, before);
        let before = app.dirty_generation;
        app.toggle_status_class(StatusClass::Success);
        assert_ne!(app.dirty_generation, before);
        let before = app.dirty_generation;
        app.toggle_pause();
        assert_ne!(app.dirty_generation, before);
    }

    #[test]
    fn pause_flag_is_toggled_idempotently() {
        let mut app = App::new("", false);
        assert!(!app.paused);
        app.toggle_pause();
        assert!(app.paused);
        app.toggle_pause();
        assert!(!app.paused);
    }

    #[test]
    fn begin_and_commit_filter_input_flow() {
        let mut app = App::new("", false);
        app.set_rows(vec![
            sample_full("/a/login", "GET", 200),
            sample_full("/a/users", "GET", 200),
        ]);
        app.begin_filter_input();
        assert!(app.filter_input_mode);
        for c in "login".chars() {
            app.push_filter_char(c);
        }
        app.commit_filter_input();
        assert!(!app.filter_input_mode);
        assert_eq!(app.filter.url_substring, "login");
        assert_eq!(app.visible_rows().len(), 1);
    }

    #[test]
    fn cancel_filter_input_preserves_committed_value() {
        let mut app = App::new("", false);
        app.filter.url_substring = "existing".to_string();
        app.begin_filter_input();
        app.push_filter_char('x');
        app.cancel_filter_input();
        assert_eq!(app.filter.url_substring, "existing");
    }

    #[test]
    fn confirm_clear_modal_toggles() {
        let mut app = App::new("", false);
        app.begin_clear_confirm();
        assert!(app.confirm_clear_mode);
        app.cancel_clear_confirm();
        assert!(!app.confirm_clear_mode);
    }
}
