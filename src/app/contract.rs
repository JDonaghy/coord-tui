//! Adapters from coord-tui's paint and hit-test call sites onto quadraui's
//! widget contract (`DrawsPrimitives::draw_primitive` / `draw_dyn` /
//! `layout_primitive`).
//!
//! Two shapes recur across the app and are kept here so every call site
//! agrees on them:
//!
//! - **Pinned toolbar hover.** coord-tui tracks the hovered toolbar button
//!   by `WidgetId` (`ToolbarHoverTracker`). The contract reads hover as a
//!   `(widget, sub-part)` address on the widget being painted, so
//!   [`toolbar_hover`] translates "button `id` is hovered" into the
//!   `InteractionState` that paints exactly that button hovered — and
//!   nothing else — when the bar is painted under `owner`'s id.
//! - **Host-placed popups.** The context menu and the prompt dialog are
//!   placed by coord-tui's own measurers (`build_context_menu_stack`,
//!   `render_prompt_dialog`), not by the widget's `Primitive::layout`.
//!   [`PlacedContextMenu`] and [`PlacedDialog`] wrap the widget with that
//!   already-resolved layout so the contract paints it where the host put
//!   it, and the click path keeps hit-testing the same geometry.
//! - **Native-unit table columns.** [`native_column_width`] keeps a fixed
//!   `DataTable` column in the backend's own unit (cells on TUI, pixels on
//!   GTK).
//! - **List paint.** [`draw_list_view`] paints every `ListView` in the app
//!   through the contract, except a `bordered` one (see its doc).

use quadraui::primitive::{LayoutCtx, PaintEnv, Primitive};
use quadraui::primitives::context_menu::ContextMenuPrimitiveLayout;
use quadraui::primitives::dialog::DialogPrimitiveLayout;
use quadraui::primitives::toolbar::ToolbarSubPart;
use quadraui::{A11yInfo, A11yPrimitive};
use quadraui::{
    Backend, ContextMenu, ContextMenuLayout, Dialog, DialogLayout, DrawsPrimitives,
    InteractionState, ListView, Rect, Toolbar, ToolbarButton, WidgetId,
};

/// The `InteractionState` that paints `hovered` — a button id inside
/// `bar` — as hovered, when `bar` is painted under `owner`'s id (the bar's
/// own id for a bare `Toolbar`; the panel's id for a `SidebarPanel`
/// header, which re-addresses its own hover onto the bar). `None`, or an
/// id that names no button in `bar`, paints nothing hovered.
pub(crate) fn toolbar_hover(
    owner: &WidgetId,
    bar: &Toolbar,
    hovered: Option<&WidgetId>,
) -> InteractionState {
    let part = hovered.and_then(|h| toolbar_part_of(bar, h));
    InteractionState::from_targets(part.map(|p| (*owner, Some(p.sub_target()))), None)
}

/// The sub-part of `bar` whose highlight id is `id`.
fn toolbar_part_of(bar: &Toolbar, id: &WidgetId) -> Option<ToolbarSubPart> {
    bar.buttons
        .iter()
        .enumerate()
        .find_map(|(i, button)| match button {
            ToolbarButton::Action { id: b, .. }
            | ToolbarButton::Field { id: b, .. }
            | ToolbarButton::MenuButton { id: b, .. }
                if b == id =>
            {
                Some(ToolbarSubPart::Button(i))
            }
            ToolbarButton::Group { items, .. } => {
                items
                    .iter()
                    .enumerate()
                    .find_map(|(child, item)| match item {
                        ToolbarButton::Action { id: b, .. } if b == id => {
                            Some(ToolbarSubPart::GroupChild { item: i, child })
                        }
                        _ => None,
                    })
            }
            _ => None,
        })
}

/// A [`ContextMenu`] at a host-resolved [`ContextMenuLayout`]. Its
/// `layout` is the widget's own for the box it is painted in, with the
/// inner placement replaced by the host's, so rows paint exactly where the
/// click path hit-tests them.
pub(crate) struct PlacedContextMenu<'a> {
    pub menu: &'a ContextMenu,
    pub layout: &'a ContextMenuLayout,
}

impl A11yPrimitive for PlacedContextMenu<'_> {
    fn a11y_info(&self) -> A11yInfo {
        self.menu.a11y_info()
    }
}

impl Primitive for PlacedContextMenu<'_> {
    type Layout = ContextMenuPrimitiveLayout;
    type Event = <ContextMenu as Primitive>::Event;

    fn id(&self) -> Option<&WidgetId> {
        Primitive::id(self.menu)
    }

    fn paint_bleed(&self, style: &quadraui::Style) -> f32 {
        Primitive::paint_bleed(self.menu, style)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let mut layout = Primitive::layout(self.menu, ctx, rect);
        layout.layout = self.layout.clone();
        layout
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn quadraui::PaintSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint(self.menu, layout, surface, env)
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn quadraui::CellSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint_cells(self.menu, layout, cells, env)
    }
}

/// Paint `menu` at its already-placed `layout`. The box handed to the
/// contract is `layout.bounds` grown by the border ring the widget reserves
/// (one cell on a cell grid, nothing on a pixel surface), the same box the
/// legacy rasteriser drew its border in.
pub(crate) fn draw_placed_context_menu(
    backend: &mut dyn Backend,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
) {
    let inset = backend.layout_primitive(layout.bounds, menu).border_inset;
    let b = layout.bounds;
    let rect = Rect::new(
        b.x - inset,
        b.y - inset,
        b.width + inset * 2.0,
        b.height + inset * 2.0,
    );
    let _ = backend.draw_primitive(rect, &PlacedContextMenu { menu, layout });
}

/// A [`Dialog`] at a host-resolved [`DialogLayout`]; see
/// [`PlacedContextMenu`].
pub(crate) struct PlacedDialog<'a> {
    pub dialog: &'a Dialog,
    pub layout: &'a DialogLayout,
}

impl A11yPrimitive for PlacedDialog<'_> {
    fn a11y_info(&self) -> A11yInfo {
        self.dialog.a11y_info()
    }
}

impl Primitive for PlacedDialog<'_> {
    type Layout = DialogPrimitiveLayout;
    type Event = <Dialog as Primitive>::Event;

    fn id(&self) -> Option<&WidgetId> {
        Primitive::id(self.dialog)
    }

    fn paint_bleed(&self, style: &quadraui::Style) -> f32 {
        Primitive::paint_bleed(self.dialog, style)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let mut layout = Primitive::layout(self.dialog, ctx, rect);
        layout.layout = self.layout.clone();
        layout
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn quadraui::PaintSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint(self.dialog, layout, surface, env)
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn quadraui::CellSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint_cells(self.dialog, layout, cells, env)
    }
}

/// Paint `dialog` at its already-placed `layout`, centred in `viewport`.
pub(crate) fn draw_placed_dialog(
    backend: &mut dyn Backend,
    viewport: Rect,
    dialog: &Dialog,
    layout: &DialogLayout,
) {
    let _ = backend.draw_primitive(viewport, &PlacedDialog { dialog, layout });
}

/// Paint `list` at `rect` through the `ListView` contract
/// (`DrawsPrimitives::draw_primitive`). On a cell grid the contract paint
/// runs the same `tui::draw_list` rasteriser as the legacy method, and on a
/// pixel backend an unbordered list paints identically too.
///
/// A `bordered` list stays on the legacy `Backend::draw_list`: the contract
/// `ListView` paint on pixel backends strokes no frame and insets its rows
/// by nothing (`supports_border: false`, `border_inset: 0`), whereas the
/// legacy GTK paint strokes the rounded frame and clips inside it.
pub(crate) fn draw_list_view(backend: &mut dyn Backend, rect: Rect, list: &ListView) {
    if list.bordered {
        // TODO(quadraui widget contract, list/tree/table group, quadraui #2609): legacy `Backend::draw_list` for bordered lists only — move to `draw_primitive` once the contract `ListView` paint strokes the `bordered` frame on pixel backends.
        #[allow(deprecated)]
        backend.draw_list(rect, list);
    } else {
        let _ = backend.draw_primitive(rect, list);
    }
}

/// A fixed `DataTable` column `width` wide in the backend's native unit —
/// cells on a cell grid, pixels on a pixel backend — so a TUI column and its
/// GTK twin keep the widths they have always had.
///
/// TODO(quadraui widget contract, list/tree/table group, quadraui #2609): `ColumnWidth::Fixed` is deprecated, and neither non-deprecated replacement keeps the native unit (`FixedCells` widens GTK columns from N px to N glyphs; `FixedPx` narrows TUI ones); move once quadraui offers a native-unit variant or the GTK widths are deliberately changed.
#[allow(deprecated)]
pub(crate) const fn native_column_width(width: f32) -> quadraui::ColumnWidth {
    quadraui::ColumnWidth::Fixed(width)
}
