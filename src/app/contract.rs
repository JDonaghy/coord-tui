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
//! - **Raw-event hosts.** [`InjectChat`] and [`PaletteModal`] keep a
//!   controlled widget together with the interaction state quadraui keeps
//!   for it, paint it with that state lent, and store what each event
//!   reports back onto the descriptor — the hosting pattern quadraui
//!   documents for an app whose events do not reach the widget through a
//!   frame tree.
//! - **List and table paint.** [`draw_list_view`] paints every `ListView` in
//!   the app through the contract; [`draw_data_table`] paints a `DataTable`
//!   with no row hover and returns the geometry its click path hit-tests.

use quadraui::primitive::{LayoutCtx, PaintEnv, Primitive};
use quadraui::primitives::context_menu::ContextMenuPrimitiveLayout;
use quadraui::primitives::dialog::DialogPrimitiveLayout;
use quadraui::primitives::toolbar::ToolbarSubPart;
use quadraui::{A11yInfo, A11yPrimitive, Interactive};
use quadraui::{
    drive_ui_event, lend_widget_state, Backend, Chat, ChatEvent, ChatState, ChatTurn, ContextMenu,
    ContextMenuLayout, DataTable, DataTableLayout, Dialog, DialogLayout, DrawsPrimitives,
    DualModePalette, DualModePaletteAction, DualModePaletteState, Handled, InteractionState,
    ListView, PaletteItem, Rect, StyledText, Toolbar, ToolbarButton, UiEvent, WidgetId,
};
use std::cell::RefCell;

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
/// (`DrawsPrimitives::draw_primitive`), bordered or not.
pub(crate) fn draw_list_view(backend: &mut dyn Backend, rect: Rect, list: &ListView) {
    let _ = backend.draw_primitive(rect, list);
}

/// Paint `widget` at `rect` through the contract with nothing hovered or
/// pressed — whatever the runtime's pointer is over — and return the layout
/// the paint used, for the click path to hit-test.
pub(crate) fn draw_unhovered<W>(backend: &mut dyn Backend, rect: Rect, widget: &W) -> W::Layout
where
    W: Interactive + 'static,
    W::Layout: Clone + 'static,
    W::State: 'static,
{
    let painted = backend.draw_dyn(rect, widget, &InteractionState::new());
    painted
        .downcast_ref::<W::Layout>()
        .expect("draw_dyn lays a widget out as its own Primitive::Layout")
        .clone()
}

/// Paint `table` at `rect` through the `DataTable` contract with no row
/// hovered or pressed, and hand back the table-local geometry the paint
/// used, so a click hit-tests exactly what is on screen.
pub(crate) fn draw_data_table(
    backend: &mut dyn Backend,
    rect: Rect,
    table: &DataTable,
) -> DataTableLayout {
    draw_unhovered(backend, rect, table).layout
}

/// The table-local geometry `draw_data_table` would paint `table` with at
/// `rect`, without painting it.
pub(crate) fn data_table_layout(
    backend: &dyn Backend,
    rect: Rect,
    table: &DataTable,
) -> DataTableLayout {
    backend.layout_primitive(rect, table).layout
}

/// The inline chat composer: the `Chat` descriptor coord-tui owns, plus the
/// `ChatState` quadraui's `Interactive` keeps for it (the input history, the
/// gesture in flight). The state is a `RefCell` so the `&self` paint path
/// can lend it to the widget.
pub(crate) struct InjectChat {
    pub(crate) chat: Chat,
    state: RefCell<ChatState>,
}

impl InjectChat {
    /// An empty-input chat under `id`, `status` in its status strip, showing
    /// `transcript`.
    pub(crate) fn new(id: &str, status: StyledText, transcript: Vec<ChatTurn>) -> Self {
        let mut chat = Chat::new(id);
        chat.status = status;
        chat.transcript = transcript;
        Self {
            chat,
            state: RefCell::default(),
        }
    }

    /// Paint the chat at `rect`, with nothing hovered or pressed.
    pub(crate) fn draw(&self, backend: &mut dyn Backend, rect: Rect) {
        let (lent, _) = lend_widget_state(self.chat.id, self.state.take(), || {
            draw_unhovered(backend, rect, &self.chat)
        });
        self.state.replace(lent);
    }

    /// Drive the chat with one raw event, laid out at `rect` the way
    /// `backend` paints it, and store every write-back it reports (input
    /// edits, transcript scroll, turn focus and collapse) onto
    /// [`Self::chat`]. What is left for the app — a submit, a cancel, a stop
    /// request, an unclaimed key, a turn click — comes back as the event;
    /// [`Handled::Consumed`] when the event only changed what the chat
    /// stores.
    pub(crate) fn handle_event(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
    ) -> Handled<ChatEvent> {
        let Some(driven) = drive_ui_event(&self.chat, self.state.get_mut(), event, backend, rect)
        else {
            return Handled::Ignored;
        };
        for emitted in driven.emitted {
            if let UiEvent::Chat(id, ev) = emitted {
                if id == self.chat.id {
                    self.chat.apply(&ev);
                }
            }
        }
        match driven.handled {
            Handled::Event(ev) => {
                self.chat.apply(&ev);
                match ev {
                    ChatEvent::InputChanged { .. }
                    | ChatEvent::TranscriptScrolled { .. }
                    | ChatEvent::FocusedTurnChanged { .. }
                    | ChatEvent::TurnCollapsed { .. } => Handled::Consumed,
                    ev => Handled::Event(ev),
                }
            }
            other => other,
        }
    }
}

/// A modal search palette — the issue finder, the command palette, the
/// doc-tab picker: the `DualModePalette` coord-tui owns plus the
/// `DualModePaletteState` quadraui's `Interactive` keeps for it (the live
/// query caret).
pub(crate) struct PaletteModal {
    pub(crate) palette: DualModePalette,
    state: DualModePaletteState,
}

impl PaletteModal {
    /// A list-mode palette under `id`, titled `title`, showing `items`.
    pub(crate) fn new(id: &str, title: &str, items: Vec<PaletteItem>) -> Self {
        Self {
            palette: DualModePalette::new(WidgetId::new(id), title, None, items),
            state: DualModePaletteState::default(),
        }
    }

    /// Paint the palette inside `rect`.
    pub(crate) fn draw(&self, rect: Rect, backend: &mut dyn Backend) {
        let _ = lend_widget_state(self.palette.id, self.state.clone(), || {
            backend.draw_dyn(rect, &self.palette, &())
        });
    }

    /// Drive the palette with one keystroke, laid out at `rect` the way
    /// `backend` paints it. A highlight, query or mode change is stored onto
    /// [`Self::palette`] (`visible_rows` is the list height a highlight move
    /// scrolls within); a highlight move alone answers
    /// [`Handled::Consumed`], and everything else — a query change to
    /// refilter on, a confirmation, a dismissal — comes back for the app.
    pub(crate) fn handle_event(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
        visible_rows: usize,
    ) -> Handled<DualModePaletteAction> {
        let Some(driven) = drive_ui_event(&self.palette, &mut self.state, event, backend, rect)
        else {
            return Handled::Ignored;
        };
        for emitted in driven.emitted {
            if let UiEvent::DualModePalette(id, action) = emitted {
                if id == self.palette.id {
                    self.palette.apply(&action, visible_rows);
                }
            }
        }
        match driven.handled {
            Handled::Event(action) => {
                self.palette.apply(&action, visible_rows);
                match action {
                    DualModePaletteAction::SelectionChanged { .. } => Handled::Consumed,
                    action => Handled::Event(action),
                }
            }
            other => other,
        }
    }
}
