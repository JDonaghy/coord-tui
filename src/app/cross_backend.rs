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
use quadraui::WidgetId;

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

/// #7: the two targeting helpers **both** concrete drivers carry
/// (`TuiDriver::tab_center` / `tab_close_center`, `GtkDriver::tab_center` /
/// `tab_close_center`, quadraui#594) but [`ConformanceDriver`] does not —
/// plus the raw left-click the trait also has no verb for (it promotes
/// `click_text`, not `click`).
///
/// This is the only way to target *one* tab's close button: every tab paints
/// the same `×`, so `click_text`/`find` can't disambiguate tab 0's from tab
/// 1's. The coordinates come out of the [`quadraui::TabBarLayout`] the painter
/// cached for that bar, i.e. out of the backend's own geometry in the
/// backend's own unit — so a shared body using these still has no coordinate
/// literal in it (rule 1), and it exercises exactly the `close_bounds` that
/// `resolve_doc_tab_click` (doc_tabs.rs) now hit-tests against.
trait TabTarget {
    fn tab_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)>;
    fn tab_close_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)>;
    /// Left-click at `(x, y)` in this backend's own unit, then repaint.
    fn click_at(&mut self, x: f32, y: f32);
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

/// #61: a board with one issue whose body is long enough to *have* to wrap —
/// `WRAP_FIRST`/`WRAP_SECOND` (below) sit ~200 characters apart in an
/// unbroken run of space-separated filler words, further apart than any
/// single row could hold at this module's [`VIEWPORT`] on either backend.
/// `body_truncated` is left at its JSON default (`false`), so
/// `issue_body_list` renders this text verbatim with no hydration fetch —
/// see `OpenIssue::body_truncated`'s doc comment.
const WRAP_BOARD_JSON: &str = r#"{
  "issues": [
    {"repo_name": "claude-coordinator", "number": 101, "title": "Fix login race timeout", "state": "open", "labels": ["coord"], "body": "WRAPFIRST lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua ut enim ad minim veniam quis nostrud exercitation ullamco WRAPSECOND"}
  ]
}"#;

/// [`smoke_app`]'s construction, against [`WRAP_BOARD_JSON`] instead of
/// [`SMOKE_BOARD_JSON`].
fn wrap_app() -> CoordApp {
    let mut app = make_app_with_board_json(WRAP_BOARD_JSON);
    let repos: Vec<String> = app.board_repo_names.clone();
    for repo in repos {
        app.board_milestone_expanded
            .insert((repo, "no-milestone".to_string()), true);
    }
    app.rebuild_board_sidebar();
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
/// #102's label in the doc-tab strip — the survivor a "closes that tab and no
/// other" body needs.
const ISSUE_102_TAB_LABEL: &str = "#102 Auth token ref…";

/// The `WidgetId` the Board doc-tab strip paints under
/// (`board_doc_tab_bar_for_pane`, render.rs) — the handle [`TabTarget`]'s
/// helpers resolve a specific tab's geometry against.
///
/// #7 note on why there is no "the active tab is bracketed" needle here any
/// more: §2c's `[`/`]` used to be characters inside `TabItem::label`, so
/// `screen_has("[#101 Fix login race…")` worked on both backends. The
/// rasteriser paints the framing now (quadraui#631), and GTK paints it as its
/// own Pango run — while GTK's `screen_has` matches only *within* a single
/// run. So the shared bodies below assert the active document through its
/// **consequence** ([`ISSUE_DETAIL_TITLE`] in the detail pane), which means
/// the same thing on both backends and is the behaviour a user would notice.
fn board_doc_tab_bar_id() -> WidgetId {
    WidgetId::new("board-doc-tabs")
}
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
/// The first word of [`WRAP_BOARD_JSON`]'s long issue body.
const WRAP_FIRST: &str = "WRAPFIRST";
/// The last word of [`WRAP_BOARD_JSON`]'s long issue body — separated from
/// [`WRAP_FIRST`] by enough filler prose that the two can only land on the
/// same painted row if the body did not wrap at all (#61's GTK symptom: the
/// wrap budget in pixels is so large nothing ever wraps).
const WRAP_SECOND: &str = "WRAPSECOND";

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
/// Covers the whole click → hit-test → `on_shell_event_ctx` → repaint loop,
/// which is the routing path most likely to be backend-specific.
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
        d.screen_has(ISSUE_TAB_LABEL) && !d.screen_has(ISSUE_DETAIL_TITLE),
        "precondition: #101 has a tab, and it is NOT the active document \
         (#102 was opened second), so its untruncated title is not in the \
         detail pane"
    );

    d.click_text(ISSUE_TAB_LABEL);

    assert!(
        d.screen_has(ISSUE_DETAIL_TITLE),
        "clicking #101's document tab must make #101 the active document, so \
         the detail pane follows it — a strip that repaints without swapping \
         the document underneath is worse than inert"
    );
}

// ── #7 (the doc tab's `×` is the backend's now) ───────────────────────────
//
// Before #7 the close glyph was a character coord had concatenated onto
// `TabItem::label`, and the close hit-test found it by scanning that string
// backwards. A character offset is not a pixel position, so on GTK the close
// zone was wherever the proportional-font label happened to put `cols - 2` —
// dead, or worse, overlapping the body. Both bodies below resolve their target
// from the backend's own `TabBarLayout` instead, which is the same geometry the
// hit-test reads, so they mean the same thing in cells and in pixels.

/// Clicking the backend-reported **centre** of an inactive doc tab activates
/// that document.
///
/// `tab_center` rather than `click_text`: this is the same gesture
/// [`a_doc_tab_activates_on_click`] makes, but aimed with the geometry the
/// painter cached — so it still passes if a future label change makes the tab
/// un-findable by text, and it fails if paint and layout ever disagree.
fn a_doc_tab_activates_from_its_reported_centre<D: ConformanceDriver + TabTarget>(d: &mut D) {
    assert!(
        !d.screen_has(ISSUE_DETAIL_TITLE),
        "precondition: #102 is the active document, not #101"
    );

    let (x, y) = d
        .tab_center_of(&board_doc_tab_bar_id(), 0)
        .expect("tab 0 is painted this frame, so the bar reports its centre");
    d.click_at(x, y);

    assert!(
        d.screen_has(ISSUE_DETAIL_TITLE),
        "a click on tab 0's reported centre must activate #101 — the detail \
         pane is the observable half"
    );
}

/// Clicking the backend-reported **close box** of a doc tab closes that tab,
/// and only that tab.
///
/// The half that was GTK-fatal before #7, and the reason `close_bounds` is now
/// the authority: a tab's close button has a real, backend-measured rect, and
/// this asserts a click inside it closes rather than activates.
fn a_doc_tab_closes_from_its_reported_close_button<D: ConformanceDriver + TabTarget>(d: &mut D) {
    assert!(
        d.screen_has(ISSUE_TAB_LABEL),
        "precondition: #101's tab is painted"
    );

    let (x, y) = d
        .tab_close_center_of(&board_doc_tab_bar_id(), 0)
        .expect("tab 0 is closable and painted, so the bar reports a close box");
    d.click_at(x, y);

    assert!(
        !d.screen_has(ISSUE_TAB_LABEL),
        "a click inside tab 0's reported close box must CLOSE it, not activate \
         it — this is the assertion a character-offset close zone could not \
         satisfy on a pixel backend"
    );
    assert!(
        d.screen_has(ISSUE_102_TAB_LABEL),
        "…and no other tab: #102's tab must survive #101's close"
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

/// #61: a long issue body must word-wrap to the pane, on **both** backends.
///
/// This is exactly the class of bug this module exists to catch (module doc
/// comment): the wrap budget `issue_body_list` (render.rs) computes is
/// character columns, but the Rect width it starts from is backend units —
/// already columns on TUI, but *pixels* on GTK/macOS. Before #61, a GTK pane
/// a little over 1000px wide turned into a budget of a little over 1000
/// *characters*, so `WRAP_FIRST` and `WRAP_SECOND` — under 200 characters
/// apart — always painted on the same unwrapped line on GTK, while TUI (whose
/// backend units already are columns) wrapped correctly the whole time. A
/// TUI-only test could not have caught that; this one runs against both.
fn a_long_issue_body_word_wraps_to_the_pane<D: ConformanceDriver>(d: &mut D) {
    d.click_text(ISSUE_ROW);
    d.click_text(BOARD_SUB_TAB_ISSUE);

    let inv = d.inventory();
    assert!(
        inv.screen_has(WRAP_FIRST) && inv.screen_has(WRAP_SECOND),
        "precondition: the seeded issue's long body must have painted at all"
    );
    assert!(
        inv.above(WRAP_FIRST, WRAP_SECOND),
        "a long issue body must word-wrap to the pane width — {WRAP_FIRST:?} \
         and {WRAP_SECOND:?} landed on the same row, meaning the body never \
         wrapped at all (#61)"
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

// The [`TabTarget`] adapters. Both drivers already carry the two helpers
// verbatim (quadraui#594); the trait exists only because `ConformanceDriver`
// does not, so a shared body can't name them.

#[cfg(feature = "tui")]
impl<A: quadraui::runner::AppLogic> TabTarget for quadraui::tui::testing::TuiDriver<A> {
    fn tab_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_center(bar, tab_idx)
    }

    fn tab_close_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_close_center(bar, tab_idx)
    }

    fn click_at(&mut self, x: f32, y: f32) {
        self.click(x, y);
        self.render();
    }
}

#[cfg(feature = "gtk")]
impl<A: quadraui::runner::AppLogic> TabTarget for quadraui::gtk::testing::GtkDriver<A> {
    fn tab_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_center(bar, tab_idx)
    }

    fn tab_close_center_of(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_close_center(bar, tab_idx)
    }

    fn click_at(&mut self, x: f32, y: f32) {
        self.click(x, y);
        self.render();
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
        a_doc_tab_activates_from_its_reported_centre,
        a_doc_tab_closes_from_its_reported_close_button,
    ],
    wrap_app => [
        a_long_issue_body_word_wraps_to_the_pane,
    ],
);
