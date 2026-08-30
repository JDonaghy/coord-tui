//! #8 — the cross-backend **core smoke set**.
//!
//! This crate has thousands of `#[cfg(test)]` driver tests and every one of
//! them drives `quadraui::tui::testing` — the TUI backend, and only the TUI
//! backend. The app logic underneath is genuinely backend-neutral, so that
//! remains the right place for behavioural depth; what was missing was any
//! automated proof that the **GTK** backend still paints the same screens and
//! still routes the same clicks. Until this module, a GTK paint or hit-test
//! regression could only be caught by a human opening the window.
//!
//! So: a handful of test bodies, written **once**, generic over
//! [`quadraui::testing::ConformanceDriver`] (quadraui#488), and instantiated
//! for both backends by the two thin adapter modules at the bottom —
//! [`tui_backend`] always, [`gtk_backend`] under `--features gtk`. The GTK
//! half needs **no `DISPLAY`**: `GtkDriver` renders into an in-memory
//! `ImageSurface` and never calls `gtk::init`.
//!
//! ## Deliberately small
//!
//! This is a smoke set over the most-trafficked screens, **not** a port of the
//! TUI suite. It answers one question per body — "does this screen still paint
//! and still route?" — and leaves depth to the TUI tests. The intended growth
//! path is one more body here per behaviour-changing issue, not a bulk import.
//!
//! ## The two rules for shared bodies
//!
//! Both are quadraui's, from `quadraui/src/testing.rs`, and both are load-
//! bearing here rather than stylistic:
//!
//! 1. **Locate by semantics, never literal coordinates.** Every body below
//!    reaches its target through [`ConformanceDriver::click_text`] — or,
//!    for the one gesture the trait doesn't carry, through
//!    [`right_click_text`], which resolves the same way out of
//!    [`ConformanceDriver::inventory`]. TUI cells and GTK pixels are different
//!    units, so a literal `click(12.0, 3.0)` in a shared body is silently
//!    wrong on one of the two backends — it would "pass" by hitting nothing.
//!    There is no numeric coordinate anywhere in a shared body; the only
//!    numbers in this file are the viewport size and the px-per-cell scale,
//!    and both live in the adapters.
//! 2. **Assert on logic and text, not pixels.** [`ConformanceDriver::screen_has`]
//!    reads a character grid on the TUI side and a list of painted Pango runs
//!    on the GTK side, and means the same thing on both. Every needle below is
//!    additionally chosen to sit inside a *single* painted run / screen row,
//!    because neither backend's `screen_has` matches across a run or row
//!    boundary.

use quadraui::testing::{ConformanceDriver, LogicalViewport};

use super::fixtures::make_app_with_board_json;
use super::CoordApp;

/// The one gesture [`ConformanceDriver`] carries no verb for: quadraui#488
/// promoted left-click, drag and scroll, but not the secondary button, and
/// coord's context menus are reachable *only* by right-click.
///
/// Implemented per backend in the two adapter modules at the bottom of this
/// file, out of each driver's own `dispatch` — the same shape as the rest of
/// the adapter split, and the reason [`right_click_text`] can stay a shared
/// body with no coordinate literal in it. If quadraui ever adds a
/// `right_click_text` to the trait, delete this and use it.
trait RightClick {
    /// Dispatch a right-button press at `(x, y)` in this backend's own unit
    /// and repaint. Only ever called with coordinates read back out of
    /// [`ConformanceDriver::inventory`], never with a literal.
    fn right_click_at(&mut self, x: f32, y: f32);
}

/// Right-click the centre of the first painted run containing `needle` —
/// [`ConformanceDriver::click_text`]'s rule ("locate by semantics, never
/// literal coordinates") applied to the one gesture the trait is missing.
///
/// The bounds come from [`ConformanceDriver::inventory`], i.e. from what the
/// backend actually painted this frame, so the same call lands on cells under
/// TUI and on pixels under GTK.
fn right_click_text<D: ConformanceDriver + RightClick>(d: &mut D, needle: &str) {
    let bounds = d
        .inventory()
        .text_runs
        .iter()
        .find(|r| r.text.contains(needle))
        .unwrap_or_else(|| panic!("right_click_text: {needle:?} was not painted"))
        .bounds;
    d.right_click_at(bounds.x + bounds.width / 2.0, bounds.y + bounds.height / 2.0);
}

/// The board every body below runs against: one repo, two `coord`-labelled
/// issues. Small on purpose — the point is that the screen paints at all, and
/// a bigger fixture only adds ways for the two backends to disagree about
/// truncation.
const SMOKE_BOARD_JSON: &str = r#"{
  "issues": [
    {"repo_name": "claude-coordinator", "number": 101, "title": "Fix login race timeout", "state": "open", "labels": ["coord"]},
    {"repo_name": "claude-coordinator", "number": 102, "title": "Auth token refresh bug", "state": "open", "labels": ["coord"]}
  ]
}"#;

/// Backend-neutral viewport for the whole smoke set. Each adapter converts it
/// into its own native units — see [`GTK_PX_PER_COL`].
const VIEWPORT: LogicalViewport = LogicalViewport::new(140, 40);

/// Build the shared fixture app — `make_app_with_board_json` plus the one
/// piece of setup the Board panel needs before it has visible issue rows: the
/// seeded repo's `No milestone` group is collapsed by default (#857), so
/// expand it.
fn smoke_app() -> CoordApp {
    let mut app = make_app_with_board_json(SMOKE_BOARD_JSON);
    let repos: Vec<String> = app.board_repo_names.clone();
    for repo in repos {
        app.board_milestone_expanded
            .insert((repo, "no-milestone".to_string()), true);
    }
    app.rebuild_board_sidebar();
    app
}

/// [`smoke_app`] with **both** seeded issues already open as *pinned* Board
/// documents, #102 active — the state the tab-strip bodies below need.
///
/// Seeded through `open_board_doc_tab(_, true)` rather than by clicking,
/// because the click path opens *preview* tabs and a second single click
/// replaces the first in place (VS Code semantics, #2282) — so no sequence of
/// `ConformanceDriver` clicks can produce two tabs, and the trait has no
/// double-click (the pin gesture) to reach for.
fn two_tab_app() -> CoordApp {
    let mut app = smoke_app();
    app.open_board_doc_tab(("claude-coordinator".to_string(), 101), true);
    app.open_board_doc_tab(("claude-coordinator".to_string(), 102), true);
    app
}

// ── Needles ───────────────────────────────────────────────────────────────
//
// Named rather than inlined so the "must sit inside one painted run" rule
// above has one place to be checked, and so a body reads as an assertion
// about the app rather than about string literals.

/// The Board panel's sidebar title.
const BOARD_TITLE: &str = "BOARD";
/// The Pipeline panel's sidebar title.
const PIPELINE_TITLE: &str = "PIPELINE";
/// The Pipeline activity-bar icon (`PanelDefinition { icon: "▶", .. }` in
/// [`CoordApp::shell_config`]). The sidebar tree also paints a `▶` collapse
/// marker, but the activity bar is painted first on both backends, and
/// `click_text` resolves the *first* match — so this targets the icon.
const PIPELINE_ICON: &str = "▶";
/// Issue #101's sidebar row.
const ISSUE_ROW: &str = "#101";
/// #101's title as the **detail pane** paints it — untruncated. The sidebar
/// row truncates to `Fix login race timeo`, so this needle is present only
/// when a detail view for #101 is actually open, which is exactly what the
/// detail/doc-tab bodies need to distinguish.
const ISSUE_DETAIL_TITLE: &str = "Fix login race timeout";
/// #101's label in the doc-tab strip. The `…` is the strip's own ellipsis, so
/// this needle appears nowhere else on screen.
const ISSUE_TAB_LABEL: &str = "#101 Fix login race…";
/// #101's doc-tab label **as the active tab paints it** — §2c brackets the
/// active document's tab (`doc_tab_label`), so the leading `[` is the whole
/// assertion: it is present only while #101 is the active document.
const ISSUE_TAB_LABEL_ACTIVE: &str = "[#101 Fix login race…";
/// The Board panel's `Board / Issue / Board Chat / Terminal` sub-tab bar,
/// second tab. Carries the spaces `board_detail_tab_bar` puts in the label so
/// it can't collide with the bare word "Issue" elsewhere on screen.
const BOARD_SUB_TAB_ISSUE: &str = " Issue ";
/// A line only the Board sub-tab's **Board** view paints (a section header).
/// Absent from the Issue view, which is what makes it a usable "the sub-tab
/// really switched" negative.
const BOARD_SUB_TAB_BOARD_MARKER: &str = "PIPELINE STAGES";
/// A line only the Board sub-tab's **Issue** view paints — the selected
/// issue's label row.
const BOARD_SUB_TAB_ISSUE_MARKER: &str = "labels: coord";
/// A Board-row context-menu item (`context_menu_target_for_selection`),
/// mid-list.
const MENU_VIEW_IN_PIPELINE: &str = "View in Pipeline";
/// A Board-row context-menu item further down the same menu, so a body can
/// tell "the popup laid out" from "the popup painted its first row".
const MENU_COPY_ISSUE: &str = "Copy issue #101";

// ── Shared bodies ─────────────────────────────────────────────────────────
//
// Each takes a live driver whose first frame is already painted (both
// `driver_with_shell` constructors run `setup` + render), and each performs at
// most one click, so no body can trip a backend's double-click folding.

/// The Board panel paints its chrome and its issue rows.
///
/// The baseline: if this fails on a backend, that backend is not rendering the
/// app's default screen at all.
fn board_panel_renders_its_rows<D: ConformanceDriver>(d: &mut D) {
    assert!(
        d.screen_has(BOARD_TITLE),
        "the Board panel's sidebar title must paint"
    );
    assert!(
        d.screen_has("claude-coordinator"),
        "the seeded repo's tree row must paint"
    );
    assert!(
        d.screen_has(ISSUE_ROW),
        "issue #101's Board row must paint"
    );
    assert!(
        d.screen_has("#102"),
        "issue #102's Board row must paint too — one row is not a list"
    );
}

/// Clicking the Pipeline icon in the activity bar swaps the panel.
///
/// Covers the whole click → hit-test → `on_shell_event` → repaint loop, which
/// is the routing path most likely to be backend-specific.
fn activity_bar_switches_to_pipeline<D: ConformanceDriver>(d: &mut D) {
    assert!(
        d.screen_has(BOARD_TITLE) && !d.screen_has(PIPELINE_TITLE),
        "precondition: the app starts on the Board panel"
    );

    d.click_text(PIPELINE_ICON);

    assert!(
        d.screen_has(PIPELINE_TITLE),
        "clicking the activity bar's Pipeline icon must switch to the \
         Pipeline panel"
    );
    assert!(
        !d.screen_has(BOARD_TITLE),
        "…and the Board panel must be gone — a panel switch that only \
         *adds* the new title has not switched anything"
    );
    assert!(
        d.screen_has("Work") && d.screen_has("Review") && d.screen_has("Merge"),
        "the Pipeline panel's stage boxes must paint"
    );
}

/// Clicking a Board issue row opens that issue's detail.
fn clicking_a_board_row_opens_the_issue_detail<D: ConformanceDriver>(d: &mut D) {
    assert!(
        !d.screen_has(ISSUE_DETAIL_TITLE),
        "precondition: with nothing selected, #101's untruncated title is \
         not on screen (the sidebar row truncates it)"
    );

    d.click_text(ISSUE_ROW);

    assert!(
        d.screen_has(ISSUE_DETAIL_TITLE),
        "clicking #101's Board row must open its detail, which paints the \
         issue's full title"
    );
}

/// A doc tab opens on a row click and closes on `Ctrl-W`.
///
/// The tab strip is its own painter on each backend (`draw_tab_bar`), so it
/// can regress independently of the panel around it.
fn a_doc_tab_opens_on_click_and_closes_on_ctrl_w<D: ConformanceDriver>(d: &mut D) {
    assert!(
        !d.screen_has(ISSUE_TAB_LABEL),
        "precondition: no document is open, so no tab strip is painted"
    );

    d.click_text(ISSUE_ROW);
    assert!(
        d.screen_has(ISSUE_TAB_LABEL),
        "clicking a Board row must open a document tab for that issue"
    );

    d.ctrl_char('w');
    assert!(
        !d.screen_has(ISSUE_TAB_LABEL),
        "Ctrl-W must close the active document tab"
    );
}

/// `q` quits.
///
/// The cheapest possible proof that plain character keys reach the app's
/// handler — and that the backend propagates `Reaction::Exit` back out — on
/// both backends.
fn typing_q_exits<D: ConformanceDriver>(d: &mut D) {
    assert!(!d.exited(), "precondition: the app has not exited");
    d.type_char('q');
    assert!(d.exited(), "`q` on the Board panel must quit");
}

// ── #24 (GTK parity walk): characters-vs-pixels ───────────────────────────
//
// Every body below was **dead or wrong on GTK** before #24, and passing on
// TUI the whole time, because a coord-side measurement counted *characters*
// and then compared the answer against a coordinate the backend reports in
// *its own* unit — cells under ratatui, pixels under GTK. None of them could
// have been caught by a TUI-only test; that is the whole argument for this
// module. See `events.rs::resolve_tab_bar_click` and
// `dialogs.rs::build_context_menu_stack`.

/// Clicking the Board panel's `Issue` sub-tab switches the detail view.
///
/// The `Board / Issue / Board Chat / Terminal` bar is the most-used control
/// in the panel and is painted by `draw_tab_bar` on both backends, so a
/// wrong-unit hit-test makes the whole Board panel look inert without
/// changing a single pixel of what is drawn.
fn a_board_sub_tab_switches_the_detail_view<D: ConformanceDriver>(d: &mut D) {
    assert!(
        d.screen_has(BOARD_SUB_TAB_BOARD_MARKER) && !d.screen_has(BOARD_SUB_TAB_ISSUE_MARKER),
        "precondition: the Board sub-tab is the one showing"
    );

    d.click_text(BOARD_SUB_TAB_ISSUE);

    assert!(
        d.screen_has(BOARD_SUB_TAB_ISSUE_MARKER),
        "clicking the `Issue` sub-tab must show the issue view"
    );
    assert!(
        !d.screen_has(BOARD_SUB_TAB_BOARD_MARKER),
        "…and the Board view must be gone — a sub-tab click that leaves the \
         old view up has not switched anything"
    );
}

/// Clicking an inactive document tab activates that document.
///
/// The doc-tab strip is a second, independently-painted `draw_tab_bar`
/// (`board_doc_tab_strip`) with its own hit-test call site, so it can regress
/// separately from the sub-tab bar above.
fn a_doc_tab_activates_on_click<D: ConformanceDriver>(d: &mut D) {
    assert!(
        d.screen_has(ISSUE_TAB_LABEL) && !d.screen_has(ISSUE_TAB_LABEL_ACTIVE),
        "precondition: #101 has a tab, and it is NOT the active document \
         (#102 was opened second)"
    );

    d.click_text(ISSUE_TAB_LABEL);

    assert!(
        d.screen_has(ISSUE_TAB_LABEL_ACTIVE),
        "clicking #101's document tab must make #101 the active document, \
         which brackets its label"
    );
    assert!(
        d.screen_has(ISSUE_DETAIL_TITLE),
        "…and the detail pane must follow the tab — a strip that repaints \
         without swapping the document underneath is worse than inert"
    );
}

/// Right-clicking a Board row opens that row's context menu on both
/// backends.
///
/// Right-click is the only way into coord's context menus and is the one
/// gesture `ConformanceDriver` has no verb for (hence [`RightClick`]), so
/// until this body nothing automated proved the GTK half of that route
/// existed at all.
///
/// **Deliberately stops at "the menu is painted."** The obvious next
/// assertion — click an item, see it run — cannot be written honestly today:
/// `open_context_menu` anchors this menu at the raw click point, which on the
/// TUI backend is a `row + 0.5` cell centre, and quadraui's
/// `ContextMenuLayout` computes hit regions from the unrounded anchor while
/// the TUI rasteriser paints at `anchor_y.round()`. The painted label for
/// item *N* therefore sits over item *N+1*'s hit region, so clicking a label
/// activates its neighbour. `events.rs`'s Board-doc-tab right-click already
/// carries a long comment about this and works around it locally by
/// anchoring at `pos.y.floor() + 2.0`; the Board-row menu does not. Making
/// the two agree is a real fix and a real regression surface, and it is not
/// #24's — #24 is characters-vs-pixels. Filed rather than smuggled in here.
fn a_context_menu_opens_on_right_click<D: ConformanceDriver + RightClick>(d: &mut D) {
    assert!(
        !d.screen_has(MENU_VIEW_IN_PIPELINE),
        "precondition: no context menu is open"
    );

    right_click_text(d, ISSUE_ROW);

    assert!(
        d.screen_has(MENU_VIEW_IN_PIPELINE),
        "right-clicking a Board row must open that row's context menu"
    );
    assert!(
        d.screen_has(MENU_COPY_ISSUE),
        "…with its later items too, not just the first — a popup that lays \
         out only one row high is not a menu"
    );
}

/// Generate, for every shared body named, one `#[test]` per backend that
/// builds that backend's driver from [`VIEWPORT`] and runs the body against
/// the fixture its group names.
///
/// A macro rather than hand-written wrappers so that adding a body is a
/// one-line change and can never be added to one backend but not the other —
/// the failure mode this whole module exists to prevent. Bodies are grouped
/// by fixture (`fixture => [bodies…]`) because which fixture a body needs is
/// part of what it asserts and belongs beside it, not inside the adapter.
macro_rules! cross_backend_smoke {
    ($($fixture:ident => [$($body:ident),+ $(,)?]),+ $(,)?) => {
        /// The TUI half — native unit: character cells.
        #[cfg(feature = "tui")]
        mod tui_backend {
            $($(
                #[test]
                fn $body() {
                    let mut driver = quadraui::tui::testing::driver_with_shell(
                        super::$fixture(),
                        super::CoordApp::shell_config(),
                        super::VIEWPORT.cols as u16,
                        super::VIEWPORT.rows as u16,
                    );
                    super::$body(&mut driver);
                }
            )+)+
        }

        /// The GTK half — native unit: pixels. No `DISPLAY` required.
        #[cfg(feature = "gtk")]
        mod gtk_backend {
            $($(
                #[test]
                fn $body() {
                    let mut driver = quadraui::gtk::testing::driver_with_shell(
                        super::$fixture(),
                        super::CoordApp::shell_config(),
                        super::VIEWPORT.cols as i32 * super::GTK_PX_PER_COL,
                        super::VIEWPORT.rows as i32 * super::GTK_PX_PER_ROW,
                    );
                    super::$body(&mut driver);
                }
            )+)+
        }
    };
}

/// Pixels per logical column on the GTK side — `GtkBackend::new`'s nominal
/// `char_width`, i.e. exactly what `<GtkDriver as ConformanceDriver>::
/// new_fixture` uses to turn a [`LogicalViewport`] into a pixel surface. Kept
/// in the adapter, never in a shared body (rule 1).
#[cfg(feature = "gtk")]
const GTK_PX_PER_COL: i32 = 8;

/// Pixels per logical row on the GTK side — `GtkBackend::new`'s nominal
/// `line_height`. See [`GTK_PX_PER_COL`].
#[cfg(feature = "gtk")]
const GTK_PX_PER_ROW: i32 = 16;

// The [`RightClick`] adapters. Each is the one-liner its driver already
// supports — `dispatch` repaints on `Reaction::Redraw` on both backends, so
// there is no separate render step to keep in sync.

/// The right-button `UiEvent` both adapters dispatch. A helper rather than
/// two copies so the two backends cannot drift on modifiers or button.
fn right_button_press(x: f32, y: f32) -> quadraui::UiEvent {
    quadraui::UiEvent::MouseDown {
        widget: None,
        button: quadraui::MouseButton::Right,
        position: quadraui::Point::new(x, y),
        modifiers: quadraui::Modifiers::default(),
    }
}

#[cfg(feature = "tui")]
impl<A: quadraui::runner::AppLogic> RightClick for quadraui::tui::testing::TuiDriver<A> {
    fn right_click_at(&mut self, x: f32, y: f32) {
        self.dispatch(right_button_press(x, y));
    }
}

#[cfg(feature = "gtk")]
impl<A: quadraui::runner::AppLogic> RightClick for quadraui::gtk::testing::GtkDriver<A> {
    fn right_click_at(&mut self, x: f32, y: f32) {
        self.dispatch(right_button_press(x, y));
    }
}

cross_backend_smoke!(
    smoke_app => [
        board_panel_renders_its_rows,
        activity_bar_switches_to_pipeline,
        clicking_a_board_row_opens_the_issue_detail,
        a_doc_tab_opens_on_click_and_closes_on_ctrl_w,
        typing_q_exits,
        a_context_menu_opens_on_right_click,
    ],
    two_tab_app => [
        a_board_sub_tab_switches_the_detail_view,
        a_doc_tab_activates_on_click,
    ],
);
