//! #71: the ONE copy of the scrollbar-track arithmetic every panel with a
//! scrollable table or list used to hand-roll.
//!
//! Before this module there were **six** near-identical hit/apply pairs —
//! Audit (`audit.rs`), Reports (`reports.rs`), the Queue grid and its detail
//! pane (`drive_queue.rs`), and the pipeline Log tab (`render.rs`) — each
//! answering the same two questions: "is this pointer on a scrollbar
//! track?" and "jump the scroll offset to the fraction of the track the
//! pointer is at?" `queue_apply_vscroll` and `reports_apply_vscroll` were
//! line-for-line identical apart from where the row count came from and
//! which field the result landed in.
//!
//! Two shapes of track existed:
//!
//! - **A quadraui `DataTableLayout`** (Audit, Reports, the Queue grid): the
//!   track's origin/length has to be derived from the layout's
//!   `header_height`/`viewport_height`/`h_scrollbar_height`/
//!   `scrollbar_width` fields — [`scrollbar_axis_hit`], [`vscroll_offset`],
//!   [`hscroll_offset`] below own that derivation.
//! - **A plain quadraui `Scrollbar`'s cached `track: Rect`** (the Queue
//!   detail pane, the pipeline Log tab): the track is already a rect, so
//!   the hit test is just `Rect::contains` (quadraui owns that one, no
//!   wrapper needed here) and only the frac-to-offset math
//!   ([`track_vscroll_offset`]) was worth sharing.
//!
//! These are pure functions over `(Point, Rect, &DataTableLayout)` or
//! `(f32, Rect, usize, usize)` — no `CoordApp`, no `RefCell` borrow — so
//! they are unit-tested directly below rather than only indirectly through
//! each panel's own mouse tests. Follows the precedent `tree_nav.rs` set
//! for `scroll_to_visible`: a small free-function module with thin
//! per-panel wrappers at the call sites, not a big-bang widget refactor.
//!
//! What this module deliberately does NOT own (yet — see #71's step 3):
//! the eight per-panel `bool`/`Option<ScrollAxis>` drag-state flags on
//! `CoordApp`, or the `MouseUp` release fan-out in `events.rs`. Collapsing
//! those into one `Option<DragTarget>` touches every drag call site and
//! wants its own review/revert boundary, so it ships as a separate PR.

use quadraui::{DataTableLayout, Point, Rect};

use super::ScrollAxis;

/// #1094/#2043: hit-test a click/drag position against a `DataTable`'s
/// scrollbar strips, using the same geometry the TUI rasteriser paints
/// them at (`quadraui::tui::data_table::draw_data_table`: the vertical
/// track occupies the rightmost `scrollbar_width` columns below the header
/// row; the horizontal track occupies the bottom `h_scrollbar_height`
/// row(s), left of the vertical track).
///
/// `DataTableLayout::hit_test` (quadraui) has no concept of either strip at
/// all — a click there falls through to whatever row/header region happens
/// to be under the cursor. Callers must check this BEFORE their table's own
/// `hit_test` so a scrollbar click never mis-resolves to row selection.
///
/// Vertical takes priority in the bottom-right corner, matching
/// `hit_test`'s own divider-before-header priority style. `rect` is the
/// panel-space origin the layout was painted into (`pos` is in the same
/// space); `pos` outside `[0, viewport_width) x [0, viewport_height)`
/// relative to `rect` never hits either strip.
pub(crate) fn scrollbar_axis_hit(pos: Point, rect: Rect, layout: &DataTableLayout) -> Option<ScrollAxis> {
    let x = pos.x - rect.x;
    let y = pos.y - rect.y;
    if x < 0.0 || y < 0.0 || x >= layout.viewport_width || y >= layout.viewport_height {
        return None;
    }
    if layout.scrollbar_width > 0.0 {
        let sb_x0 = layout.viewport_width - layout.scrollbar_width;
        if x >= sb_x0 && y >= layout.header_height {
            return Some(ScrollAxis::Vertical);
        }
    }
    if layout.h_scrollbar_height > 0.0 {
        let hsb_y0 = layout.viewport_height - layout.h_scrollbar_height;
        if y >= hsb_y0 {
            return Some(ScrollAxis::Horizontal);
        }
    }
    None
}

/// #1094/#1910/#2043: the row offset implied by a click/drag position
/// along a `DataTable`'s vertical scrollbar track — standard
/// click/drag-to-position scrollbar behaviour (not thumb-relative
/// dragging). `0` when there is nothing to scroll (`item_count` fits
/// inside `layout.visible_rows`).
///
/// Callers own the "nothing painted yet" / "no rows" guards themselves
/// (they return early rather than call this) since those are a `bool`
/// "did anything change" the panel state carries, not something this pure
/// function has an opinion on.
pub(crate) fn vscroll_offset(pointer_y: f32, rect: Rect, layout: &DataTableLayout, item_count: usize) -> usize {
    let visible_rows = layout.visible_rows.max(1);
    let max_scroll = item_count.saturating_sub(visible_rows);
    if max_scroll == 0 {
        return 0;
    }
    let track_y0 = rect.y + layout.header_height;
    let track_h =
        (layout.viewport_height - layout.header_height - layout.h_scrollbar_height).max(1.0);
    let frac = ((pointer_y - track_y0) / track_h).clamp(0.0, 1.0);
    (frac * max_scroll as f32).round() as usize
}

/// #1094/#2043: the column offset (in the same surface-native units as
/// `layout.content_width`) implied by a click/drag position along a
/// `DataTable`'s horizontal scrollbar track. `0.0` when the content fits
/// (no horizontal scrollbar on screen to hit).
pub(crate) fn hscroll_offset(pointer_x: f32, rect: Rect, layout: &DataTableLayout) -> f32 {
    let visible_w = (layout.viewport_width - layout.scrollbar_width).max(1.0);
    let max_scroll = (layout.content_width - visible_w).max(0.0);
    if max_scroll <= 0.0 {
        return 0.0;
    }
    let frac = ((pointer_x - rect.x) / visible_w).clamp(0.0, 1.0);
    frac * max_scroll
}

/// #2017/#64: the row offset implied by a click/drag position along a
/// *cached* `Scrollbar.track` rect — the shape the Queue detail pane and
/// the pipeline Log tab both use (a plain quadraui `Scrollbar`, not a
/// `DataTableLayout`). `0` when there is nothing to scroll or the track
/// wasn't actually painted with any height.
///
/// The track-rect hit test these two panels also share needs no wrapper
/// here — it's exactly `Rect::contains`, already public on quadraui's own
/// `Rect`.
pub(crate) fn track_vscroll_offset(pointer_y: f32, track: Rect, item_count: usize, visible_rows: usize) -> usize {
    let max_scroll = item_count.saturating_sub(visible_rows.max(1));
    if max_scroll == 0 || track.height <= 0.0 {
        return 0;
    }
    let frac = ((pointer_y - track.y) / track.height).clamp(0.0, 1.0);
    (frac * max_scroll as f32).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::{Column, ColumnAlign, ColumnMeasure, ColumnWidth, DataTable, WidgetId};

    /// Build a real `DataTableLayout` via quadraui's own `DataTable::layout`
    /// — not hand-rolled — so these tests exercise exactly the fields
    /// `scrollbar_axis_hit`/`vscroll_offset`/`hscroll_offset` read.
    ///
    /// `wide` forces `content_width` past the viewport (via
    /// `min_total_width`) so the layout also grows a horizontal scrollbar
    /// strip, exercising the axis-priority corner case.
    fn test_layout(wide: bool) -> (Rect, DataTableLayout) {
        let table = DataTable {
            id: WidgetId::new("table-nav-test"),
            columns: vec![Column {
                title: "A".to_string(),
                width: ColumnWidth::Fixed(10.0),
                align: ColumnAlign::Left,
            }],
            rows: Vec::new(),
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: true,
            min_total_width: if wide { Some(100.0) } else { None },
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        };
        let rect = Rect::new(2.0, 3.0, 20.0, 10.0);
        let layout = table.layout(rect.width, rect.height, 1.0, 1.0, 2.0, |_| {
            ColumnMeasure::new(0.0)
        });
        (rect, layout)
    }

    #[test]
    fn vscroll_offset_top_of_track_is_zero() {
        let (rect, layout) = test_layout(false);
        assert_eq!(vscroll_offset(rect.y + layout.header_height, rect, &layout, 100), 0);
    }

    #[test]
    fn vscroll_offset_bottom_of_track_is_max_scroll() {
        let (rect, layout) = test_layout(false);
        let track_y0 = rect.y + layout.header_height;
        let track_h = layout.viewport_height - layout.header_height - layout.h_scrollbar_height;
        let item_count = 100;
        let max_scroll = item_count - layout.visible_rows;
        let offset = vscroll_offset(track_y0 + track_h, rect, &layout, item_count);
        assert_eq!(offset, max_scroll);
    }

    #[test]
    fn vscroll_offset_clamps_pointer_outside_track() {
        let (rect, layout) = test_layout(false);
        let track_y0 = rect.y + layout.header_height;
        // Far above the track clamps to 0, far below clamps to max_scroll.
        let item_count = 50;
        let max_scroll = item_count - layout.visible_rows;
        assert_eq!(vscroll_offset(track_y0 - 1000.0, rect, &layout, item_count), 0);
        assert_eq!(
            vscroll_offset(track_y0 + 1000.0, rect, &layout, item_count),
            max_scroll
        );
    }

    #[test]
    fn vscroll_offset_is_zero_when_everything_fits() {
        let (rect, layout) = test_layout(false);
        // item_count <= visible_rows: nothing to scroll, regardless of pointer.
        assert_eq!(vscroll_offset(rect.y + 500.0, rect, &layout, layout.visible_rows), 0);
    }

    #[test]
    fn hscroll_offset_left_of_track_is_zero() {
        let (rect, layout) = test_layout(true);
        assert_eq!(hscroll_offset(rect.x, rect, &layout), 0.0);
    }

    #[test]
    fn hscroll_offset_right_of_track_is_max_scroll() {
        let (rect, layout) = test_layout(true);
        let visible_w = layout.viewport_width - layout.scrollbar_width;
        let max_scroll = layout.content_width - visible_w;
        let offset = hscroll_offset(rect.x + visible_w + 1000.0, rect, &layout);
        assert!((offset - max_scroll).abs() < 0.01);
    }

    #[test]
    fn hscroll_offset_is_zero_when_content_fits() {
        let (rect, layout) = test_layout(false);
        assert_eq!(hscroll_offset(rect.x + 5.0, rect, &layout), 0.0);
    }

    #[test]
    fn scrollbar_axis_hit_finds_vertical_strip() {
        let (rect, layout) = test_layout(false);
        let pos = Point::new(
            rect.x + layout.viewport_width - 1.0,
            rect.y + layout.header_height + 1.0,
        );
        assert_eq!(scrollbar_axis_hit(pos, rect, &layout), Some(ScrollAxis::Vertical));
    }

    #[test]
    fn scrollbar_axis_hit_finds_horizontal_strip() {
        let (rect, layout) = test_layout(true);
        // Left edge of the horizontal strip, away from the vertical strip's column.
        let pos = Point::new(rect.x, rect.y + layout.viewport_height - 1.0);
        assert_eq!(scrollbar_axis_hit(pos, rect, &layout), Some(ScrollAxis::Horizontal));
    }

    #[test]
    fn scrollbar_axis_hit_prefers_vertical_in_corner() {
        let (rect, layout) = test_layout(true);
        // Bottom-right corner: inside both strips at once.
        let pos = Point::new(
            rect.x + layout.viewport_width - 1.0,
            rect.y + layout.viewport_height - 1.0,
        );
        assert_eq!(scrollbar_axis_hit(pos, rect, &layout), Some(ScrollAxis::Vertical));
    }

    #[test]
    fn scrollbar_axis_hit_misses_body() {
        let (rect, layout) = test_layout(false);
        let pos = Point::new(rect.x + 1.0, rect.y + layout.header_height + 1.0);
        assert_eq!(scrollbar_axis_hit(pos, rect, &layout), None);
    }

    #[test]
    fn scrollbar_axis_hit_outside_viewport_is_none() {
        let (rect, layout) = test_layout(false);
        let pos = Point::new(rect.x - 5.0, rect.y + 1.0);
        assert_eq!(scrollbar_axis_hit(pos, rect, &layout), None);
    }

    #[test]
    fn track_vscroll_offset_top_is_zero() {
        let track = Rect::new(10.0, 20.0, 1.0, 30.0);
        assert_eq!(track_vscroll_offset(track.y, track, 100, 10), 0);
    }

    #[test]
    fn track_vscroll_offset_bottom_is_max_scroll() {
        let track = Rect::new(10.0, 20.0, 1.0, 30.0);
        let item_count = 100;
        let visible_rows = 10;
        assert_eq!(
            track_vscroll_offset(track.y + track.height, track, item_count, visible_rows),
            item_count - visible_rows
        );
    }

    #[test]
    fn track_vscroll_offset_zero_height_track_is_zero() {
        let track = Rect::new(10.0, 20.0, 1.0, 0.0);
        assert_eq!(track_vscroll_offset(25.0, track, 100, 10), 0);
    }

    #[test]
    fn track_vscroll_offset_nothing_to_scroll_is_zero() {
        let track = Rect::new(10.0, 20.0, 1.0, 30.0);
        assert_eq!(track_vscroll_offset(track.y + track.height, track, 5, 10), 0);
    }
}
