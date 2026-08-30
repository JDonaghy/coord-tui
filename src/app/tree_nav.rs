//! #12 (seam-audit finding A3): the ONE copy of the list/tree navigation
//! arithmetic that every sidebar and detail panel in this crate used to
//! hand-roll.
//!
//! Before this module there were **eight** character-for-character copies of
//! "clamp `scroll` so `selected` is inside a `visible`-row viewport"
//! (`fix_machine_scroll`, `fix_merge_queue_scroll`, `fix_terminal_tree_
//! scroll`, `fix_sessions_tree_scroll`, `fix_audit_scroll`,
//! `fix_plans_detail_scroll`, `fix_approved_scroll`, `fix_queue_scroll`) and
//! **three** of the flat pixel→row click math (`Terminal`, `Sessions`,
//! `Plans` in `events.rs`). Each copy carries a doc comment admitting it
//! mirrors one of the others — the exact churn pattern #12 was filed about.
//!
//! **Why this delegates to quadraui instead of re-deriving the arithmetic.**
//! `quadraui::TreeController::scroll_to_visible` *is* the canonical
//! implementation, and finding A3's whole point is that a local copy —
//! however well factored — becomes the next thing to keep in sync with the
//! framework. So [`scroll_to_visible`] below owns no arithmetic of its own:
//! it hands the offset to a `TreeController` and reads the answer back. If
//! quadraui's clamp rule changes, every panel in coord-tui follows for free.
//!
//! The per-panel `fix_*_scroll` methods survive as thin wrappers because each
//! one still resolves its *own* notion of "which flat row is selected"
//! (`merge_queue_display_idx_for_sel`, `terminal_tree_selected_flat_index`,
//! …) — that part is genuinely panel-specific. What they no longer own is the
//! clamp.
//!
//! Full `TreeController` adoption for the Terminal/Sessions/Plans trees
//! (`set_rows`/`render`/`handle`, and with it `TreeControllerEvent::
//! ContextMenuRequested`) is the other half of A3 and is blocked on
//! quadraui#475 for the selection plumbing; this module is the part that is
//! unblocked today.

use quadraui::TreeController;

/// Clamp `scroll` so that flat row `selected` is inside a `visible`-row
/// viewport, then write the (possibly unchanged) offset back.
///
/// `visible == 0` is a no-op — the panel has not been painted yet, so there
/// is no viewport to scroll into. That matches the `if visible == 0 { return }`
/// guard every call site used to open with.
///
/// Delegates to [`TreeController::scroll_to_visible`], which is stateless with
/// respect to the controller's rows/selection: it reads and writes only
/// `scroll_offset`. Constructing a throwaway controller is therefore exact,
/// and cheap enough for a per-keystroke path (one empty `Vec` + one small
/// `String`).
pub(crate) fn scroll_to_visible(scroll: &mut usize, selected: usize, visible: usize) {
    let mut probe = TreeController::new("coord-tui/tree_nav");
    probe.set_scroll_offset(*scroll);
    probe.scroll_to_visible(selected, visible);
    *scroll = probe.scroll_offset();
}

/// Map a click's y position inside a scrolled, flat row list to the row index
/// it landed on.
///
/// `top_y` is the top edge of the list's rect, `line_height` the backend's
/// line height, `scroll` the list's current first-visible-row offset. Returns
/// `None` for a click *above* the list (which the raw `TreeView` panels must
/// reject rather than saturate to row 0 — a click on the panel border is not
/// a click on the first row) and for a non-positive `line_height` (guards the
/// division; a zero line height would otherwise produce `inf`/`NaN`).
///
/// The returned index is unbounded on the high side: callers hit-test it
/// against their own row list (`terminal_tree_click_row`, `sessions_tree_
/// click_row`, `plans_tree_click_row`), which is where "past the last row"
/// belongs — those functions already answer it.
pub(crate) fn row_at_y(pos_y: f32, top_y: f32, line_height: f32, scroll: usize) -> Option<usize> {
    if pos_y < top_y || line_height <= 0.0 {
        return None;
    }
    Some(((pos_y - top_y) / line_height).floor() as usize + scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_to_visible_leaves_an_in_window_selection_alone() {
        let mut scroll = 3;
        scroll_to_visible(&mut scroll, 5, 10);
        assert_eq!(scroll, 3);
    }

    #[test]
    fn scroll_to_visible_follows_a_selection_past_the_bottom() {
        let mut scroll = 0;
        scroll_to_visible(&mut scroll, 12, 10);
        // Last visible row must be the selection: 12 - (10 - 1) == 3.
        assert_eq!(scroll, 3);
    }

    #[test]
    fn scroll_to_visible_follows_a_selection_above_the_top() {
        let mut scroll = 7;
        scroll_to_visible(&mut scroll, 2, 10);
        assert_eq!(scroll, 2);
    }

    #[test]
    fn scroll_to_visible_is_a_noop_with_no_viewport() {
        let mut scroll = 4;
        scroll_to_visible(&mut scroll, 99, 0);
        assert_eq!(scroll, 4);
    }

    #[test]
    fn scroll_to_visible_pins_selection_when_viewport_is_one_row() {
        let mut scroll = 0;
        scroll_to_visible(&mut scroll, 6, 1);
        assert_eq!(scroll, 6);
    }

    #[test]
    fn row_at_y_maps_pixels_to_scrolled_row_index() {
        // rect top 10.0, line height 2.0, scrolled down 5 rows.
        assert_eq!(row_at_y(10.0, 10.0, 2.0, 5), Some(5));
        assert_eq!(row_at_y(11.9, 10.0, 2.0, 5), Some(5));
        assert_eq!(row_at_y(12.0, 10.0, 2.0, 5), Some(6));
        assert_eq!(row_at_y(17.0, 10.0, 2.0, 5), Some(8));
    }

    #[test]
    fn row_at_y_rejects_a_click_above_the_list() {
        assert_eq!(row_at_y(9.9, 10.0, 2.0, 0), None);
    }

    #[test]
    fn row_at_y_rejects_a_degenerate_line_height() {
        assert_eq!(row_at_y(20.0, 10.0, 0.0, 0), None);
    }
}
