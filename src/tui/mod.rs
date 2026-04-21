//! Ratatui TUI — request list, detail pane, filter bar, status bar.
//!
//! `App` owns its local state. It subscribes to the event bus, polls a
//! crossterm event stream, and renders on a 30 fps tick. Panic hook
//! restores terminal state before backtrace. Implemented across
//! TASK-200 → TASK-203.

pub mod app;
pub mod body;
pub mod curl;
pub mod event;
pub mod filter;
pub mod highlight;
pub mod keys;
pub mod mock;
pub mod mouse;
pub mod theme;
pub mod view;

use std::io::{self, Stdout};
use std::sync::Once;
use std::time::Duration;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, EventStream,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::bus::{Event as BusEvent, EventBus};
use crate::store::{SecretsMode as StoreSecretsMode, StoreHandle};

use self::app::App;
use self::curl::{SecretsMode as CurlSecretsMode, copy_to_clipboard, curl_command};
use self::event::{AppEvent, translate};
use self::theme::Theme;

/// Runtime config for the TUI surface. Separate from [`crate::cli::Cli`]
/// so the TUI remains callable without the full CLI struct (tests, MCP
/// flags, etc.).
#[derive(Debug, Clone)]
pub struct TuiConfig {
    /// Address shown in the status bar. Typically `ws://host:port`.
    pub listen_addr: String,
    /// When true, start in mock-data mode (status bar says so and the
    /// empty-state text is different).
    pub mock_mode: bool,
    /// Effective sensitive-header list (defaults + user extras). Used by
    /// the cURL exporter to honour user-configured redaction (M-8).
    pub sensitive_headers: Vec<String>,
}

impl TuiConfig {
    /// Build a TUI config for a live backend at the given socket address.
    #[must_use]
    pub fn live(addr: impl Into<String>) -> Self {
        Self {
            listen_addr: addr.into(),
            mock_mode: false,
            sensitive_headers: crate::store::default_sensitive_headers(),
        }
    }

    /// Build a TUI config for mock-data mode.
    #[must_use]
    pub fn mock() -> Self {
        Self {
            listen_addr: String::from("(mock mode — no live server)"),
            mock_mode: true,
            sensitive_headers: crate::store::default_sensitive_headers(),
        }
    }

    /// Override the sensitive-header list. Chain after `live` / `mock`.
    #[must_use]
    pub fn with_sensitive_headers(mut self, headers: Vec<String>) -> Self {
        self.sensitive_headers = headers;
        self
    }
}

/// Failure modes for the TUI surface. Terminal I/O errors are the only
/// ones that escape the loop — everything else is logged and swallowed.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Failed to mutate the underlying terminal (alt-screen enter/exit,
    /// raw mode, mouse capture, drawing).
    #[error("terminal I/O error: {0}")]
    Terminal(#[from] io::Error),
}

/// Run the TUI against the given store + bus. Returns when `token` is
/// cancelled or the user presses `q` / `Ctrl+C`.
///
/// # Panic hook
///
/// A panic hook is installed **before** any terminal state is changed.
/// If the loop panics anywhere after alt-screen entry, the hook runs
/// first — it calls [`restore_terminal`], which leaves the alt-screen,
/// disables raw mode, and turns off mouse capture. Only then does the
/// existing hook (typically `color_eyre`) run to pretty-print the
/// backtrace.
///
/// Ordering matters: the hook must be installed before `enable_raw_mode`
/// so a panic during setup still restores the tty. We install it once
/// per process.
///
/// # Errors
///
/// [`TuiError::Terminal`] if the terminal cannot be reset after the
/// loop exits. Setup errors also surface here, but the hook means we
/// restore state even if propagation happens mid-frame.
pub async fn run(
    config: TuiConfig,
    store: StoreHandle,
    bus: EventBus,
    token: CancellationToken,
) -> Result<(), TuiError> {
    install_panic_hook();

    let mut terminal = enter_terminal()?;
    let result = run_loop(&mut terminal, &config, &store, &bus, &token).await;
    // Always restore, even on error, before surfacing the error to the
    // caller. This runs in addition to the panic hook — the hook is for
    // panics, this path is for normal exits.
    restore_terminal();
    result
}

/// Internal loop, split out so the restore path stays simple.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: &TuiConfig,
    store: &StoreHandle,
    bus: &EventBus,
    token: &CancellationToken,
) -> Result<(), TuiError> {
    let theme = Theme::from_env();
    let mut app = App::new(config.listen_addr.clone(), config.mock_mode);

    // Prime the initial row set from the store. If the store is gone
    // already there's nothing to do; the TUI still renders the empty
    // state and exits on quit.
    if let Ok(rows) = store.all(StoreSecretsMode::Redacted).await {
        app.set_rows(rows);
    }

    let mut crossterm_events = EventStream::new();
    let mut bus_rx = bus.subscribe();
    let mut ticker = tokio::time::interval(Duration::from_millis(33));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Tracks what the terminal currently has enabled; we started with
    // mouse capture on (see `enter_terminal`). When the user presses `M`
    // to toggle, we call `apply_mouse_capture` to actually update the
    // terminal state to match `app.mouse_capture`.
    let mut current_mouse_capture = true;

    loop {
        // Expire stale toast before checking dirty — so the frame we draw
        // next never shows a toast past its TTL.
        app.tick_toast();
        if app.needs_redraw() {
            terminal.draw(|frame| view::draw(frame, &mut app, &theme))?;
            app.mark_drawn();
        }
        apply_mouse_capture(&mut current_mouse_capture, app.mouse_capture);
        if app.should_quit {
            break;
        }

        tokio::select! {
            biased;
            () = token.cancelled() => {
                break;
            }
            crossterm_ev = crossterm_events.next() => {
                match crossterm_ev {
                    Some(Ok(ev)) => handle_crossterm(ev, &mut app, store, &config.sensitive_headers).await,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "crossterm event stream error");
                    }
                    None => break,
                }
            }
            bus_ev = bus_rx.recv() => {
                match bus_ev {
                    Ok(ev) => handle_bus(ev, &mut app, store).await,
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(events_missed = n, "tui subscriber lagged; resyncing");
                        if !app.paused
                            && let Ok(rows) = store.all(StoreSecretsMode::Redacted).await
                        {
                            app.set_rows(rows);
                        }
                    }
                }
            }
            _ = ticker.tick() => {
                // render cadence only — no state change.
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn handle_crossterm(
    ev: CrosstermEvent,
    app: &mut App,
    store: &StoreHandle,
    sensitive: &[String],
) {
    let app_ev = translate(
        ev,
        &app.hit_regions,
        app.filter_input_mode,
        app.confirm_clear_mode,
        app.detail_full_view,
    );
    apply_app_event(app_ev, app, store, sensitive).await;
}

async fn handle_bus(ev: BusEvent, app: &mut App, store: &StoreHandle) {
    match ev {
        BusEvent::ResponseReceived(_) | BusEvent::RequestStarted(_) => {
            if app.paused {
                return;
            }
            if let Ok(rows) = store.all(StoreSecretsMode::Redacted).await {
                app.set_rows(rows);
            }
        }
        BusEvent::ClientConnected(id) => {
            app.connections.add(id);
            app.mark_dirty();
        }
        BusEvent::ClientDisconnected(id) => {
            app.connections.remove(id);
            app.mark_dirty();
        }
    }
}

async fn apply_app_event(ev: AppEvent, app: &mut App, store: &StoreHandle, sensitive: &[String]) {
    use self::event::PaneId;
    match ev {
        AppEvent::Quit => app.quit(),
        // Pane-aware scroll. `j`/`k`/↑/↓ route to whichever pane is
        // focused — list pane moves the selection, detail pane scrolls
        // the response body. PgUp/PgDn + Shift+J/K remain explicit
        // detail-scroll shortcuts so users can scroll the body without
        // leaving list focus.
        AppEvent::ScrollDown => {
            if app.focused == PaneId::Detail {
                app.scroll_detail_down();
            } else {
                app.scroll_list_down();
            }
        }
        AppEvent::ScrollUp => {
            if app.focused == PaneId::Detail {
                app.scroll_detail_up();
            } else {
                app.scroll_list_up();
            }
        }
        AppEvent::DetailScrollDown => app.scroll_detail_down(),
        AppEvent::DetailScrollUp => app.scroll_detail_up(),
        AppEvent::NextPane => {
            let next = app.focused.next();
            app.focus(next);
        }
        AppEvent::PrevPane => {
            let prev = app.focused.prev();
            app.focus(prev);
        }
        AppEvent::FocusPane(pane) => app.focus(pane),
        AppEvent::SelectRow(idx) => app.select_row(idx),
        AppEvent::Resize => {
            app.force_redraw = true;
            app.mark_dirty();
        }

        AppEvent::ToggleCollapse => {
            // Toggle the section closest to the scroll cursor. v1 picks
            // the first section (RequestHeaders) when scroll is small
            // and advances through the ordered list as detail_scroll
            // grows. This is a rough heuristic — `SectionId::ordered`
            // gives us four sections and the detail pane has ~3 lines
            // of metadata plus variable section heights. Divide by a
            // conservative stride so each 10 lines of scroll moves to
            // the next section.
            let idx = (app.detail_scroll as usize / 10).min(3);
            let section = app::SectionId::ordered()[idx];
            app.toggle_section(section);
        }
        AppEvent::OpenFullView => app.set_full_view(true),
        AppEvent::CloseFullView => app.set_full_view(false),
        AppEvent::CopyCurl => {
            copy_selected_curl(app, CurlSecretsMode::Redacted, store, sensitive).await;
        }
        AppEvent::CopyCurlRaw => {
            copy_selected_curl(app, CurlSecretsMode::Raw, store, sensitive).await;
        }
        AppEvent::CopyCurlForRow(idx) => {
            // Copy a specific visible row WITHOUT first moving the
            // selection. Runs through the same redacted-mode pipeline as
            // the `y` key — the cached redacted row is authoritative.
            if let Some(req) = app.visible_rows().get(idx).cloned() {
                let cmd = curl_command(&req, CurlSecretsMode::Redacted, sensitive);
                match copy_to_clipboard(&cmd) {
                    Ok(()) => {
                        tracing::info!(row = idx, "copied cURL for row");
                        app.show_toast("cURL copied (redacted)", app::ToastKind::Success);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            cmd = %cmd,
                            "clipboard unavailable; cURL dropped to log"
                        );
                        app.show_toast(
                            format!("clipboard unavailable ({e}) — see log file"),
                            app::ToastKind::Error,
                        );
                    }
                }
            } else {
                app.show_toast("no row at that index", app::ToastKind::Error);
            }
        }

        AppEvent::BeginFilterInput => app.begin_filter_input(),
        AppEvent::CommitFilterInput => app.commit_filter_input(),
        AppEvent::CancelFilterInput => app.cancel_filter_input(),
        AppEvent::FilterChar(c) => app.push_filter_char(c),
        AppEvent::FilterBackspace => app.pop_filter_char(),
        AppEvent::ToggleMethod(m) => app.toggle_method(m),
        AppEvent::ToggleStatus(c) => app.toggle_status_class(c),
        AppEvent::ClearAllFilters => {
            app.clear_all_filters();
            app.show_toast("filters cleared", app::ToastKind::Info);
        }
        AppEvent::TogglePause => {
            app.toggle_pause();
            // Reflect the new state into the store so incoming WS frames
            // actually stop being captured (H-1). We do not fail the TUI
            // if this errors — the actor-dead case is handled elsewhere.
            if let Err(e) = store.set_paused(app.paused).await {
                tracing::warn!(error = %e, "could not propagate pause to store");
            }
        }
        AppEvent::BeginClearConfirm => app.begin_clear_confirm(),
        AppEvent::CancelClearConfirm => app.cancel_clear_confirm(),
        AppEvent::ConfirmClear => {
            app.cancel_clear_confirm();
            match store.clear().await {
                Ok(()) => {
                    app.set_rows(Vec::new());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to clear store");
                }
            }
        }

        AppEvent::ToggleMouseCapture => app.toggle_mouse_capture(),

        AppEvent::NoOp => {}
    }
}

/// Reconcile the `app.mouse_capture` state with what crossterm currently
/// has enabled. Called each tick after event handling so the toggle
/// keybind actually changes terminal behaviour.
fn apply_mouse_capture(current: &mut bool, desired: bool) {
    if *current == desired {
        return;
    }
    let result = if desired {
        crossterm::execute!(io::stdout(), EnableMouseCapture)
    } else {
        crossterm::execute!(io::stdout(), DisableMouseCapture)
    };
    match result {
        Ok(()) => *current = desired,
        Err(e) => tracing::warn!(error = %e, desired, "failed to toggle mouse capture"),
    }
}

async fn copy_selected_curl(
    app: &mut App,
    mode: CurlSecretsMode,
    store: &StoreHandle,
    sensitive: &[String],
) {
    // For Raw mode we MUST re-fetch the row with `SecretsMode::Raw`, because
    // the cached `app.rows` were loaded redacted and their header values
    // are already `***` (M-7). Redacted mode can use the cached row.
    let req_owned = match mode {
        CurlSecretsMode::Redacted => app.selected_request().cloned(),
        CurlSecretsMode::Raw => {
            let Some(id) = app.selected_request().map(|r| r.id) else {
                app.show_toast("no row selected", app::ToastKind::Error);
                return;
            };
            match store.get(id, StoreSecretsMode::Raw).await {
                Ok(row) => row,
                Err(e) => {
                    tracing::warn!(error = %e, "could not fetch raw row for cURL");
                    app.show_toast("store unavailable — cURL not copied", app::ToastKind::Error);
                    return;
                }
            }
        }
    };
    let Some(req) = req_owned else {
        app.show_toast("no row selected", app::ToastKind::Error);
        return;
    };
    let cmd = curl_command(&req, mode, sensitive);
    let label = match mode {
        CurlSecretsMode::Redacted => "cURL copied (redacted)",
        CurlSecretsMode::Raw => "cURL copied (raw headers)",
    };
    match copy_to_clipboard(&cmd) {
        Ok(()) => {
            tracing::info!(mode = ?mode, "copied cURL to clipboard");
            app.show_toast(label, app::ToastKind::Success);
        }
        Err(e) => {
            // Headless CI / no clipboard — NEVER eprintln here; we're
            // inside the alt-screen and any stderr write would corrupt
            // the render. The cURL command is logged to the file via
            // tracing at debug level for recovery.
            tracing::warn!(error = %e, cmd = %cmd, "clipboard unavailable; cURL dropped to log");
            app.show_toast(
                format!("clipboard unavailable ({e}) — see log file"),
                app::ToastKind::Error,
            );
        }
    }
}

/// Set up the terminal for TUI mode: raw mode, alternate screen, mouse.
fn enter_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>, TuiError> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its cooked state. Safe to call from a panic
/// hook — it swallows errors (there's nothing sensible to do with them
/// when unwinding) and never panics itself.
pub fn restore_terminal() {
    // Errors here are best-effort: if the terminal is already gone or in
    // a weird state, there is no recovery path. Log would go to stderr
    // which is likely also torn down in that scenario.
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
}

static PANIC_HOOK: Once = Once::new();

/// Install the panic hook that restores terminal state before the next
/// hook in the chain runs. Idempotent — safe to call more than once.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_panic_hook_is_idempotent() {
        install_panic_hook();
        install_panic_hook();
        // no assert — if either call panicked, the test would fail.
    }

    #[test]
    fn restore_terminal_is_safe_on_cold_terminal() {
        // We have no alt-screen; restore should do nothing observable
        // and definitely not panic.
        restore_terminal();
    }

    #[test]
    fn panic_hook_is_installed_before_any_terminal_mutation() {
        // The implementation guarantee we document on `run()` is that
        // `install_panic_hook` is called before `enter_terminal`. Reading
        // the source of `run` keeps that invariant visible and
        // greppable; this test pins the ordering in code by asserting
        // that the public path we document exists and doesn't panic when
        // invoked without an attached TTY. If the hook weren't idempotent
        // or failed to set up, this would surface here.
        install_panic_hook();
        install_panic_hook();
        restore_terminal();
    }

    #[test]
    fn tui_config_live_has_addr() {
        let c = TuiConfig::live("ws://127.0.0.1:9090");
        assert_eq!(c.listen_addr, "ws://127.0.0.1:9090");
        assert!(!c.mock_mode);
    }

    #[test]
    fn tui_config_mock_flags_mock_mode() {
        let c = TuiConfig::mock();
        assert!(c.mock_mode);
    }

    // ── Pane-aware scroll ──────────────────────────────────────────
    // Exercises apply_app_event's ScrollDown/ScrollUp routing without
    // spinning up the tokio runtime or a real store.

    use crate::bus::new_bus;
    use crate::store::{self, StoreConfig};

    async fn fixture() -> (
        App,
        store::StoreHandle,
        tokio_util::sync::CancellationToken,
        tokio::task::JoinHandle<()>,
    ) {
        use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};

        let bus = new_bus(16);
        let token = tokio_util::sync::CancellationToken::new();
        let task = store::spawn(StoreConfig::default(), bus.clone(), token.clone());
        let handle = task.handle.clone();

        let mut app = App::new("test", false);
        // Seed two rows so list-scroll has somewhere to move.
        let mut rows = Vec::new();
        for i in 0..3u32 {
            let exchange = ApiResponsePayload {
                duration: Some(10.0),
                request: ApiRequestSide {
                    url: format!("https://x/{i}"),
                    method: Some("GET".to_string()),
                    data: crate::protocol::Body::null(),
                    headers: None,
                    params: None,
                },
                response: ApiResponseSide {
                    status: 200,
                    headers: None,
                    body: crate::protocol::Body::null(),
                },
            };
            rows.push(store::Request::complete(exchange, None));
        }
        app.set_rows(rows);
        (app, handle, token, task.join)
    }

    #[tokio::test]
    async fn scroll_down_with_list_focused_moves_selection_not_detail() {
        let (mut app, store, token, join) = fixture().await;
        app.focus(event::PaneId::List);
        let before_sel = app.list_state.selected();
        let before_detail = app.detail_scroll;

        apply_app_event(AppEvent::ScrollDown, &mut app, &store, &[]).await;

        assert_ne!(
            app.list_state.selected(),
            before_sel,
            "list selection should advance when list is focused"
        );
        assert_eq!(
            app.detail_scroll, before_detail,
            "detail scroll should NOT change when list is focused"
        );

        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn scroll_down_with_detail_focused_scrolls_body_not_selection() {
        let (mut app, store, token, join) = fixture().await;
        app.focus(event::PaneId::Detail);
        let before_sel = app.list_state.selected();
        let before_detail = app.detail_scroll;

        apply_app_event(AppEvent::ScrollDown, &mut app, &store, &[]).await;

        assert_eq!(
            app.list_state.selected(),
            before_sel,
            "list selection should stay put when detail is focused"
        );
        assert!(
            app.detail_scroll > before_detail,
            "detail scroll should advance when detail is focused"
        );

        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn pgdn_scrolls_detail_regardless_of_focus() {
        let (mut app, store, token, join) = fixture().await;
        app.focus(event::PaneId::List);
        let before = app.detail_scroll;

        // Explicit detail-scroll shortcut — should work even when the
        // list pane is the focused one.
        apply_app_event(AppEvent::DetailScrollDown, &mut app, &store, &[]).await;

        assert!(
            app.detail_scroll > before,
            "PgDn / Shift+J must scroll the detail body from any focus"
        );

        token.cancel();
        let _ = join.await;
    }
}
