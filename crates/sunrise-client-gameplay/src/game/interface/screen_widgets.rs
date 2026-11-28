//! Mounted screen widgets (`ScreenEvent.Init.Post` / `Screen.addRenderableWidget`):
//! engine-owned buttons, local `EditBox` text fields, and custom [`ScreenWidget`]
//! instances with synchronous callback execution via temporary ownership extraction.

#[cfg(feature = "client")]
use super::stations::tooltip::{
    PreparedTooltipItem, WidgetTooltipPositioner, quantize_tooltip_anchor,
};
#[cfg(feature = "client")]
use crate::game::host::draw::GameDrawContext;
use mod_api::{
    __private::game::CallbackOwner,
    ScreenView, ScreenWidget, ScreenWidgetId, ScreenWidgets, Text as ApiText,
    client::widgets::{
        Button, ButtonPress, ButtonSprites, ButtonState, ScreenChild, TextEdit, TextField,
        TextFieldCallback, WidgetTooltip, is_allowed_chat_character,
    },
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use sunrise_client::{
    clipboard::EditShortcut,
    hud::HudBatch,
    keybinds,
    preedit::PreeditEvent,
    ui::{self, EditBox},
    widgets::{self, Kind, Narratable, Target},
};
#[cfg(feature = "client")]
use sunrise_game_data::Result;
use sunrise_game_data::text::{DARK_GRAY, Style, Text};
use sunrise_gameplay_common::game::api_values::{TryIntoApi, TryIntoEngine};
#[cfg(feature = "client")]
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// One engine-drawn widget listed on the active screen instance.
#[derive(Clone, Debug)]
pub(in crate::game) struct EngineChildSlot {
    pub(in crate::game) id: ScreenWidgetId,
    pub(in crate::game) key: widgets::EngineWidgetKey,
    pub(in crate::game) default_rect: [f32; 4],
    pub(in crate::game) default_message: Option<ApiText>,
    pub(in crate::game) default_active: bool,
    pub(in crate::game) default_tab_order_group: i32,
}

/// How an incoming pointer click should be routed against active engine widget overrides.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::game) enum EngineClickRoute {
    PassThrough,
    Select {
        key: widgets::EngineWidgetKey,
        resolved: [f32; 4],
        original: [f32; 4],
    },
    Block,
}

/// The engine widget selected on press, carried through drag and release so moved widgets
/// receive press/drag/release exclusively without leaking to widgets underneath.
#[derive(Clone, Debug)]
pub(in crate::game) struct ActiveEngineTarget {
    pub(in crate::game) key: widgets::EngineWidgetKey,
    pub(in crate::game) redirect: Option<([f32; 4], [f32; 4])>,
}

pub(in crate::game) struct MountedWidgetsInner {
    pub(in crate::game) order: Vec<ScreenWidgetId>,
    pub(in crate::game) engine_children: Vec<EngineChildSlot>,
    pub(in crate::game) engine_id_by_key: HashMap<widgets::EngineWidgetKey, ScreenWidgetId>,
    pub(in crate::game) engine_overrides: widgets::EngineOverrideMaps,
    pub(in crate::game) engine_hits: Vec<widgets::EngineHitEntry>,
    pub(in crate::game) engine_hovered: HashMap<widgets::EngineWidgetKey, bool>,
    pub(in crate::game) pending_tooltip: Option<widgets::PendingWidgetTooltip>,
    pub(in crate::game) mounted_hover_since: HashMap<ScreenWidgetId, Instant>,
    pub(in crate::game) entries: Vec<Option<MountedWidgetEntry>>,
    pub(in crate::game) discovered: bool,
    pub(in crate::game) needs_engine_resync: bool,
    pub(in crate::game) overrides_active: bool,
    pub(in crate::game) focused: Option<usize>,
    pub(in crate::game) dragging: Option<(usize, i32)>,
    pub(in crate::game) active_redirect: Option<ActiveEngineTarget>,
}

/// Mounted widgets and engine widget overrides attached to the currently open screen.
pub struct MountedScreenWidgets {
    pub(in crate::game) generation: Cell<u64>,
    pub(in crate::game) next_id: Cell<u64>,
    /// Synchronized child snapshot returned by `ScreenWidgets::children(&self)`.
    pub(in crate::game) children_cache: Vec<ScreenChild>,
    pub(in crate::game) inner: RefCell<MountedWidgetsInner>,
}

thread_local! {
    /// True while dispatching a redirected or programmatic (`press`) click at an engine widget's
    /// original coordinates, so moved widgets still pass screen-local hit checks.
    static REDIRECTING_ENGINE_CLICK: Cell<bool> = const { Cell::new(false) };
}

impl Default for MountedScreenWidgets {
    fn default() -> Self {
        Self {
            generation: Cell::new(0),
            next_id: Cell::new(1),
            children_cache: Vec::new(),
            inner: RefCell::new(MountedWidgetsInner {
                order: Vec::new(),
                engine_children: Vec::new(),
                engine_id_by_key: HashMap::new(),
                engine_overrides: widgets::EngineOverrideMaps::default(),
                engine_hits: Vec::new(),
                engine_hovered: HashMap::new(),
                pending_tooltip: None,
                mounted_hover_since: HashMap::new(),
                entries: Vec::new(),
                discovered: false,
                needs_engine_resync: false,
                overrides_active: false,
                focused: None,
                dragging: None,
                active_redirect: None,
            }),
        }
    }
}

impl Drop for MountedScreenWidgets {
    fn drop(&mut self) {
        self.clear();
    }
}

impl MountedScreenWidgets {
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.borrow().entries.iter().all(Option::is_none)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn len(&self) -> usize {
        self.inner
            .borrow()
            .entries
            .iter()
            .filter(|slot| slot.is_some())
            .count()
    }

    pub fn has_focus(&self) -> bool {
        self.inner.borrow().focused.is_some()
    }

    pub fn focused_index(&self) -> Option<usize> {
        self.inner.borrow().focused
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn dragging(&self) -> Option<(usize, i32)> {
        self.inner.borrow().dragging
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn is_disabled(&self, idx: usize) -> bool {
        self.inner
            .borrow()
            .entries
            .get(idx)
            .and_then(Option::as_ref)
            .is_some_and(|e| e.disabled)
    }

    pub(in crate::game) fn index_of(&self, id: ScreenWidgetId) -> Option<usize> {
        self.inner
            .borrow()
            .entries
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|entry| entry.id == id))
    }

    pub fn focused_id(&self) -> Option<ScreenWidgetId> {
        let inner = self.inner.borrow();
        let idx = inner.focused?;
        inner.entries.get(idx)?.as_ref().map(|entry| entry.id)
    }

    /// Bump the screen-widget generation, clear focus/drag/override state, and dispose all entries.
    pub fn clear(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let (mut entries, focused) = {
            let mut inner = self.inner.borrow_mut();
            inner.order.clear();
            inner.engine_children.clear();
            inner.engine_id_by_key.clear();
            inner.engine_overrides = widgets::EngineOverrideMaps::default();
            inner.engine_hits.clear();
            inner.engine_hovered.clear();
            inner.pending_tooltip = None;
            inner.mounted_hover_since.clear();
            inner.dragging = None;
            inner.active_redirect = None;
            inner.discovered = false;
            inner.needs_engine_resync = false;
            inner.overrides_active = false;
            (std::mem::take(&mut inner.entries), inner.focused.take())
        };
        if let Some(entry) = focused.and_then(|idx| entries.get_mut(idx)?.as_mut()) {
            entry.apply_focus(false);
        }
        for entry in entries.into_iter().flatten() {
            entry.dispose();
        }
    }

    pub(in crate::game) fn clear_links_for(&self, idx: usize) {
        let mut inner = self.inner.borrow_mut();
        if inner.focused == Some(idx) {
            inner.focused = None;
        }
        if inner.dragging.is_some_and(|(drag_idx, _)| drag_idx == idx) {
            inner.dragging = None;
        }
    }
}

/// Convert and validate an API [`WidgetTooltip`] into engine [`widgets::EngineWidgetTooltip`].
/// Returns `None` when a `Text` tree inside the tooltip fails boundary validation.
pub(in crate::game) fn convert_widget_tooltip(
    tooltip: Option<WidgetTooltip>,
) -> Option<Option<widgets::EngineWidgetTooltip>> {
    let Some(tip) = tooltip else {
        return Some(None);
    };
    let message: Text = tip.message.try_into_engine(&()).ok()?;
    let narration = match tip.narration {
        Some(narration) => {
            let text: Text = narration.try_into_engine(&()).ok()?;
            text.plain()
        }
        None => message.plain(),
    };
    Some(Some(widgets::EngineWidgetTooltip {
        message,
        narration,
        component: tip.component,
        style: tip.style,
    }))
}

/// One widget mounted on the active screen, owned by the engine with per-widget panic isolation.
pub struct MountedWidgetEntry {
    pub(in crate::game) id: ScreenWidgetId,
    pub(in crate::game) owner: CallbackOwner,
    pub(in crate::game) disabled: bool,
    pub kind: MountedWidgetKind,
}

/// Engine representation of each mounted widget variant.
pub enum MountedWidgetKind {
    Button {
        state: ButtonState,
        on_press: Option<Box<ButtonPress>>,
        narration: Option<ApiText>,
    },
    TextField {
        edit_box: EditBox,
        hint: Option<Text>,
        active: bool,
        visible: bool,
        alpha: f32,
        tooltip: Option<WidgetTooltip>,
        tooltip_delay: Duration,
        tab_order_group: i32,
        on_change: Option<Box<TextFieldCallback>>,
        on_submit: Option<Box<TextFieldCallback>>,
    },
    Custom(Box<dyn ScreenWidget>),
}

impl MountedWidgetEntry {
    pub(in crate::game) fn dispose(self) {
        let Self { owner, kind, .. } = self;
        owner.dispose("screen widget", "screen_widget_drop", kind);
    }

    #[cfg(feature = "client")]
    pub(in crate::game) fn press_button(&mut self, game: &mut crate::game::ClientGame<'_>) -> bool {
        let MountedWidgetKind::Button {
            state, on_press, ..
        } = &mut self.kind
        else {
            return false;
        };
        let Some(callback) = on_press.as_mut() else {
            return true;
        };
        let rect = state.rect;
        let answer = call_owner(&self.owner, &mut self.disabled, "button_press", || {
            callback(state, game);
        });
        if !valid_widget_rect(state.rect) {
            state.rect = rect;
        }
        answer.is_some()
    }

    pub(in crate::game) fn can_focus(&mut self) -> bool {
        if self.disabled || !self.owner.is_enabled() {
            self.disabled = true;
            return false;
        }
        match &self.kind {
            MountedWidgetKind::Button { state, .. } => state.visible && state.active,
            MountedWidgetKind::TextField {
                active, visible, ..
            } => *visible && *active,
            MountedWidgetKind::Custom(widget) => call_owner(
                &self.owner,
                &mut self.disabled,
                "screen_widget_focusable",
                || widget.visible() && widget.active() && valid_widget_rect(widget.bounds()),
            )
            .unwrap_or(false),
        }
    }

    pub(in crate::game) fn apply_focus(&mut self, focused: bool) -> bool {
        if self.disabled || !self.owner.is_enabled() {
            self.disabled = true;
            return false;
        }
        match &mut self.kind {
            MountedWidgetKind::Button { .. } => true,
            MountedWidgetKind::TextField { edit_box, .. } => {
                edit_box.focused = focused;
                if focused {
                    edit_box.reset_blink();
                }
                true
            }
            MountedWidgetKind::Custom(widget) => call_owner(
                &self.owner,
                &mut self.disabled,
                "screen_widget_focus",
                || widget.set_focused(focused),
            )
            .is_some(),
        }
    }
}

/// One validated mutation operation for an existing widget, shared by staged init hooks
/// (`ScreenWidgetMount`) and live `ClientGame` mutations so both paths enforce identical policy.
#[derive(Clone, Debug)]
pub(in crate::game) enum WidgetOp {
    Bounds([f32; 4]),
    Message(ApiText, Text),
    Active(bool),
    Visible(bool),
    Alpha(f32),
    LabelTextColor(Option<u32>),
    Tooltip(Option<WidgetTooltip>, Option<widgets::EngineWidgetTooltip>),
    TooltipDelay(Duration),
    TabOrderGroup(i32),
    Sprites(Option<ButtonSprites>),
    OverrideHighlighted(Option<bool>),
}

impl WidgetOp {
    fn button_only(&self) -> bool {
        matches!(
            self,
            Self::LabelTextColor(_) | Self::Sprites(_) | Self::OverrideHighlighted(_)
        )
    }

    fn affects_hits(&self) -> bool {
        matches!(self, Self::Bounds(_) | Self::Active(_) | Self::Visible(_))
    }
}

fn button_to_kind(button: Button) -> MountedWidgetKind {
    let (state, on_press, narration) = button.into_parts();
    MountedWidgetKind::Button {
        state,
        on_press,
        narration,
    }
}

fn text_field_to_kind(field: TextField) -> MountedWidgetKind {
    let parts = field.into_parts();
    let [x, y, w, h] = parts.rect;
    let mut edit_box = EditBox::new(x, y, w, h);
    edit_box.max_length = parts.max_length;
    edit_box.bordered = parts.bordered;
    edit_box.editable = parts.editable;
    edit_box.focused = false;
    edit_box.set_value(&parts.value);
    // `EditBox.setHint`: an unstyled hint takes `DEFAULT_HINT_STYLE` (`DARK_GRAY`).
    let hint = parts.hint.and_then(|hint| {
        let text: Text = hint.try_into_engine(&()).ok()?;
        let unstyled = text.spans.iter().all(|span| span.style == Style::default());
        Some(if unstyled {
            text.with_color(DARK_GRAY)
        } else {
            text
        })
    });
    MountedWidgetKind::TextField {
        edit_box,
        hint,
        active: parts.active,
        visible: parts.visible,
        alpha: parts.alpha,
        tooltip: parts.tooltip,
        tooltip_delay: parts.tooltip_delay,
        tab_order_group: parts.tab_order_group,
        on_change: parts.on_change,
        on_submit: parts.on_submit,
    }
}

/// Apply one validated widget mutation to a mounted entry; returns `false` if the entry's kind
/// does not support `op`.
pub(in crate::game) fn apply_widget_op_to_mounted(
    entry: &mut MountedWidgetEntry,
    op: WidgetOp,
) -> bool {
    match (&mut entry.kind, op) {
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Bounds(rect)) => {
            state.rect = rect;
            true
        }
        (MountedWidgetKind::TextField { edit_box, .. }, WidgetOp::Bounds(rect)) => {
            edit_box.x = rect[0];
            edit_box.y = rect[1];
            edit_box.width = rect[2];
            edit_box.height = rect[3];
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Message(api, _)) => {
            state.message = api;
            true
        }
        (MountedWidgetKind::TextField { edit_box, .. }, WidgetOp::Message(_, engine)) => {
            edit_box.message = engine.plain();
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Active(v)) => {
            state.active = v;
            true
        }
        (MountedWidgetKind::TextField { active, .. }, WidgetOp::Active(v)) => {
            *active = v;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Visible(v)) => {
            state.visible = v;
            true
        }
        (MountedWidgetKind::TextField { visible, .. }, WidgetOp::Visible(v)) => {
            *visible = v;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Alpha(v)) => {
            state.alpha = v;
            true
        }
        (MountedWidgetKind::TextField { alpha, .. }, WidgetOp::Alpha(v)) => {
            *alpha = v;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::LabelTextColor(c)) => {
            state.label_text_color = c;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Tooltip(api, _)) => {
            state.tooltip = api;
            true
        }
        (MountedWidgetKind::TextField { tooltip, .. }, WidgetOp::Tooltip(api, _)) => {
            *tooltip = api;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::TooltipDelay(d)) => {
            state.tooltip_delay = d;
            true
        }
        (MountedWidgetKind::TextField { tooltip_delay, .. }, WidgetOp::TooltipDelay(d)) => {
            *tooltip_delay = d;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::TabOrderGroup(g)) => {
            state.tab_order_group = g;
            true
        }
        (
            MountedWidgetKind::TextField {
                tab_order_group, ..
            },
            WidgetOp::TabOrderGroup(g),
        ) => {
            *tab_order_group = g;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::Sprites(s)) => {
            state.sprites = s;
            true
        }
        (MountedWidgetKind::Button { state, .. }, WidgetOp::OverrideHighlighted(h)) => {
            state.override_render_highlighted_sprite = h;
            true
        }
        _ => false,
    }
}

/// Apply one validated widget mutation to an engine widget's override entry.
pub(in crate::game) fn apply_widget_op_to_engine(
    ov: &mut widgets::EngineWidgetOverride,
    op: WidgetOp,
) {
    match op {
        WidgetOp::Bounds(rect) => ov.rect = Some(rect),
        WidgetOp::Message(_, engine) => ov.message = Some(engine),
        WidgetOp::Active(v) => ov.active = Some(v),
        WidgetOp::Visible(v) => ov.visible = Some(v),
        WidgetOp::Alpha(v) => ov.alpha = Some(v),
        WidgetOp::LabelTextColor(c) => ov.label_text_color = Some(c),
        WidgetOp::Tooltip(_, engine_tip) => ov.tooltip = Some(engine_tip),
        WidgetOp::TooltipDelay(d) => ov.tooltip_delay = Some(d),
        WidgetOp::TabOrderGroup(g) => ov.tab_order_group = Some(g),
        WidgetOp::Sprites(s) => ov.sprites = Some(s),
        WidgetOp::OverrideHighlighted(h) => ov.override_render_highlighted_sprite = Some(h),
    }
}

/// Reflect a validated widget mutation into a staged `ScreenChild` snapshot.
fn apply_widget_op_to_child(child: &mut ScreenChild, op: &WidgetOp) {
    match op {
        WidgetOp::Bounds(rect) => child.rect = *rect,
        WidgetOp::Message(api, _) => child.message = Some(api.clone()),
        WidgetOp::Active(v) => child.active = *v,
        WidgetOp::Visible(v) => child.visible = *v,
        WidgetOp::Alpha(v) => child.alpha = *v,
        WidgetOp::LabelTextColor(c) => child.label_text_color = *c,
        WidgetOp::Tooltip(_, _) | WidgetOp::TooltipDelay(_) | WidgetOp::Sprites(_) => {}
        WidgetOp::TabOrderGroup(g) => child.tab_order_group = *g,
        WidgetOp::OverrideHighlighted(h) => child.override_render_highlighted_sprite = *h,
    }
}

/// Build hit-test entries directly from discovered engine slots and active overrides so clicks,
/// blocking, and `press(id)` match the override table immediately before the next frame draws.
pub(in crate::game) fn build_engine_hits(
    slots: &[EngineChildSlot],
    maps: &widgets::EngineOverrideMaps,
) -> Vec<widgets::EngineHitEntry> {
    slots
        .iter()
        .map(|slot| {
            if maps.suppress_all {
                return widgets::EngineHitEntry {
                    key: slot.key.clone(),
                    original_rect: slot.default_rect,
                    resolved_rect: slot.default_rect,
                    interactive: false,
                    blocks_original: true,
                };
            }
            let ov = maps.overrides.get(&slot.key);
            let removed = ov.is_some_and(|o| o.removed);
            let visible = ov.and_then(|o| o.visible).unwrap_or(true);
            if removed || !visible {
                return widgets::EngineHitEntry {
                    key: slot.key.clone(),
                    original_rect: slot.default_rect,
                    resolved_rect: slot.default_rect,
                    interactive: false,
                    blocks_original: true,
                };
            }
            let resolved_rect = ov.and_then(|o| o.rect).unwrap_or(slot.default_rect);
            let active = ov.and_then(|o| o.active).unwrap_or(slot.default_active);
            let moved = ov.and_then(|o| o.rect).is_some();
            widgets::EngineHitEntry {
                key: slot.key.clone(),
                original_rect: slot.default_rect,
                resolved_rect,
                interactive: active,
                blocks_original: moved || (!active && slot.default_active),
            }
        })
        .collect()
}

/// Registrar passed to `Hooks::screen_init_pre` and `Hooks::screen_init_post`.
pub(in crate::game) struct ScreenWidgetMount {
    pub(in crate::game) next_id: u64,
    pub(in crate::game) entries: Vec<MountedWidgetEntry>,
    pub(in crate::game) children: Vec<ScreenChild>,
    pub(in crate::game) engine_ids: Vec<ScreenWidgetId>,
    pub(in crate::game) field_ids: Vec<ScreenWidgetId>,
    pub(in crate::game) custom_ids: Vec<ScreenWidgetId>,
    pub(in crate::game) removed: Vec<ScreenWidgetId>,
    pub(in crate::game) ops: Vec<(ScreenWidgetId, WidgetOp)>,
    pub(in crate::game) initial_focus: Option<ScreenWidgetId>,
}

impl Drop for ScreenWidgetMount {
    fn drop(&mut self) {
        for entry in std::mem::take(&mut self.entries) {
            entry.dispose();
        }
    }
}

impl ScreenWidgetMount {
    pub(in crate::game) fn alloc_id(&mut self) -> ScreenWidgetId {
        let raw = self.next_id;
        self.next_id = self.next_id.saturating_add(1).max(1);
        ScreenWidgetId::from_raw(raw)
    }

    pub(in crate::game) fn stage(&mut self, kind: MountedWidgetKind) -> ScreenWidgetId {
        let id = self.alloc_id();
        let is_field = matches!(kind, MountedWidgetKind::TextField { .. });
        let is_custom = matches!(kind, MountedWidgetKind::Custom(_));
        let mut entry = MountedWidgetEntry {
            id,
            owner: widget_callback_owner(),
            disabled: false,
            kind,
        };
        let child = match &entry.kind {
            MountedWidgetKind::Button { state, .. } => {
                let mut child = ScreenChild::new(id, state.rect, Some(state.message.clone()));
                child.active = state.active;
                child.visible = state.visible;
                child.alpha = state.alpha;
                child.label_text_color = state.label_text_color;
                child.tab_order_group = state.tab_order_group;
                child.override_render_highlighted_sprite = state.override_render_highlighted_sprite;
                Some(child)
            }
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                alpha,
                tab_order_group,
                ..
            } => {
                let mut child = ScreenChild::new(
                    id,
                    [edit_box.x, edit_box.y, edit_box.width, edit_box.height],
                    None,
                );
                child.active = *active;
                child.visible = *visible;
                child.alpha = *alpha;
                child.tab_order_group = *tab_order_group;
                Some(child)
            }
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_bounds",
                || {
                    let mut child = ScreenChild::new(id, widget.bounds(), None);
                    child.active = widget.active();
                    child.visible = widget.visible();
                    child.tab_order_group = widget.tab_order_group();
                    child
                },
            ),
        };
        if let Some(child) = child
            && valid_widget_rect(child.rect)
        {
            if is_field {
                self.field_ids.push(id);
            } else if is_custom {
                self.custom_ids.push(id);
            }
            self.children.push(child);
            self.entries.push(entry);
        } else {
            entry.dispose();
        }
        id
    }
    fn stage_widget_op(&mut self, id: ScreenWidgetId, op: WidgetOp) -> bool {
        if self.custom_ids.contains(&id) || (op.button_only() && self.field_ids.contains(&id)) {
            return false;
        }
        let Some(child) = self.children.iter_mut().find(|child| child.id == id) else {
            return false;
        };
        apply_widget_op_to_child(child, &op);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            let applied = apply_widget_op_to_mounted(entry, op);
            debug_assert!(applied);
        } else {
            self.ops.push((id, op));
        }
        true
    }
}

impl ScreenWidgets for ScreenWidgetMount {
    fn children(&self) -> &[ScreenChild] {
        &self.children
    }

    fn add_button(&mut self, button: Button) -> ScreenWidgetId {
        self.stage(button_to_kind(button))
    }

    fn add_text_field(&mut self, field: TextField) -> ScreenWidgetId {
        self.stage(text_field_to_kind(field))
    }

    fn add_widget(&mut self, widget: Box<dyn ScreenWidget>) -> ScreenWidgetId {
        self.stage(MountedWidgetKind::Custom(widget))
    }

    fn set_initial_focus(&mut self, id: ScreenWidgetId) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            if !entry.can_focus() {
                return false;
            }
            self.initial_focus = Some(id);
            return true;
        }
        if !self.engine_ids.contains(&id)
            && self
                .children
                .iter()
                .any(|child| child.id == id && child.visible && child.active)
        {
            self.initial_focus = Some(id);
            return true;
        }
        false
    }

    fn remove(&mut self, id: ScreenWidgetId) -> bool {
        let Some(pos) = self.children.iter().position(|child| child.id == id) else {
            return false;
        };
        self.children.remove(pos);
        if self.initial_focus == Some(id) {
            self.initial_focus = None;
        }
        if let Some(pos) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries.remove(pos).dispose();
        } else {
            self.removed.push(id);
        }
        true
    }

    fn clear(&mut self) {
        let ids: Vec<_> = self.children.iter().map(|child| child.id).collect();
        for id in ids {
            self.remove(id);
        }
    }

    fn set_bounds(&mut self, id: ScreenWidgetId, rect: [f32; 4]) -> bool {
        if !valid_widget_rect(rect) || rect[2] < 0.0 || rect[3] < 0.0 {
            return false;
        }
        self.stage_widget_op(id, WidgetOp::Bounds(rect))
    }

    fn set_message(&mut self, id: ScreenWidgetId, message: ApiText) -> bool {
        let Ok(engine_text): std::result::Result<Text, _> = message.try_into_engine(&()) else {
            return false;
        };
        self.stage_widget_op(id, WidgetOp::Message(message, engine_text))
    }

    fn set_active(&mut self, id: ScreenWidgetId, active: bool) -> bool {
        self.stage_widget_op(id, WidgetOp::Active(active))
    }

    fn set_visible(&mut self, id: ScreenWidgetId, visible: bool) -> bool {
        self.stage_widget_op(id, WidgetOp::Visible(visible))
    }

    fn set_alpha(&mut self, id: ScreenWidgetId, alpha: f32) -> bool {
        if !alpha.is_finite() {
            return false;
        }
        self.stage_widget_op(id, WidgetOp::Alpha(alpha))
    }

    fn set_label_text_color(&mut self, id: ScreenWidgetId, color: Option<u32>) -> bool {
        self.stage_widget_op(id, WidgetOp::LabelTextColor(color))
    }

    fn set_tooltip(&mut self, id: ScreenWidgetId, tooltip: Option<WidgetTooltip>) -> bool {
        let Some(engine_tip) = convert_widget_tooltip(tooltip.clone()) else {
            return false;
        };
        self.stage_widget_op(id, WidgetOp::Tooltip(tooltip, engine_tip))
    }

    fn set_tooltip_delay(&mut self, id: ScreenWidgetId, delay: Duration) -> bool {
        self.stage_widget_op(id, WidgetOp::TooltipDelay(delay))
    }

    fn set_tab_order_group(&mut self, id: ScreenWidgetId, group: i32) -> bool {
        self.stage_widget_op(id, WidgetOp::TabOrderGroup(group))
    }

    fn set_sprites(&mut self, id: ScreenWidgetId, sprites: Option<ButtonSprites>) -> bool {
        self.stage_widget_op(id, WidgetOp::Sprites(sprites))
    }

    fn set_override_render_highlighted_sprite(
        &mut self,
        id: ScreenWidgetId,
        highlighted: Option<bool>,
    ) -> bool {
        self.stage_widget_op(id, WidgetOp::OverrideHighlighted(highlighted))
    }

    fn press(&mut self, _id: ScreenWidgetId) -> bool {
        false
    }
}

pub(in crate::game) fn widget_callback_owner() -> CallbackOwner {
    CallbackOwner::for_mod(mod_api::content::crash::current_native_mod().unwrap_or("sunrise"))
}

pub(in crate::game) fn call_owner<R>(
    owner: &CallbackOwner,
    disabled: &mut bool,
    operation: &'static str,
    invoke: impl FnOnce() -> R,
) -> Option<R> {
    if *disabled || !owner.is_enabled() {
        *disabled = true;
        return None;
    }
    match owner.call("screen widget", operation, invoke) {
        Ok(value) => Some(value),
        Err(_) => {
            if !owner.is_enabled() {
                *disabled = true;
            }
            None
        }
    }
}

pub(in crate::game) fn valid_widget_rect([x, y, w, h]: [f32; 4]) -> bool {
    [x, y, w, h, x + w, y + h].into_iter().all(f32::is_finite)
}

pub(in crate::game) fn point_in_rect(mouse: [f32; 2], rect: [f32; 4]) -> bool {
    let [mx, my] = mouse;
    let [x, y, w, h] = rect;
    w > 0.0 && h > 0.0 && mx >= x && my >= y && mx < x + w && my < y + h
}

impl MountedWidgetEntry {
    pub(in crate::game) fn target(&mut self) -> Option<Target> {
        if self.disabled || !self.owner.is_enabled() {
            self.disabled = true;
            return None;
        }
        match &self.kind {
            MountedWidgetKind::Button { state, .. } => {
                (state.visible && state.active).then_some(Target {
                    rect: state.rect,
                    kind: Kind::Mounted {
                        id: self.id.raw(),
                        field: false,
                    },
                })
            }
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                ..
            } => (*visible && *active).then_some(Target {
                rect: [edit_box.x, edit_box.y, edit_box.width, edit_box.height],
                kind: Kind::Mounted {
                    id: self.id.raw(),
                    field: true,
                },
            }),
            MountedWidgetKind::Custom(widget) => call_owner(
                &self.owner,
                &mut self.disabled,
                "screen_widget_target",
                || {
                    let rect = widget.bounds();
                    (widget.visible() && widget.active() && valid_widget_rect(rect)).then(|| {
                        Target {
                            rect,
                            kind: Kind::Mounted {
                                id: self.id.raw(),
                                field: widget.captures_input(),
                            },
                        }
                    })
                },
            )
            .flatten(),
        }
    }
}

#[cfg(feature = "client")]
impl crate::game::ClientGame<'_> {
    /// Allocate the next screen-local widget id for the open screen instance.
    pub(in crate::game) fn alloc_widget_id(&self) -> ScreenWidgetId {
        let raw = self.client.screen_state.0.widgets.next_id.get();
        self.client
            .screen_state
            .0
            .widgets
            .next_id
            .set(raw.saturating_add(1).max(1));
        ScreenWidgetId::from_raw(raw)
    }

    /// Rebuild `children_cache` from `order`, `engine_children`, `engine_overrides`, and `entries`
    /// so `ScreenWidgets::children(&self)` reflects every live mutation and frame hover state.
    pub(in crate::game) fn sync_children_cache(&mut self) {
        let mouse = self.cursor();
        let snapshot = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            let mut out = Vec::with_capacity(inner.order.len());
            let order = inner.order.clone();
            for id in order {
                if let Some(slot) = inner.engine_children.iter().find(|s| s.id == id) {
                    let ov = inner.engine_overrides.overrides.get(&slot.key);
                    if ov.is_some_and(|o| o.removed) {
                        continue;
                    }
                    let rect = ov.and_then(|o| o.rect).unwrap_or(slot.default_rect);
                    let message = ov
                        .and_then(|o| o.message.as_ref())
                        .and_then(|text| text.try_into_api().ok())
                        .or_else(|| slot.default_message.clone());
                    let mut child = ScreenChild::new(id, rect, message);
                    child.active = ov.and_then(|o| o.active).unwrap_or(slot.default_active);
                    child.visible = ov.and_then(|o| o.visible).unwrap_or(true);
                    child.hovered = inner
                        .engine_hovered
                        .get(&slot.key)
                        .copied()
                        .unwrap_or_else(|| child.is_mouse_over(mouse));
                    child.alpha = ov.and_then(|o| o.alpha).unwrap_or(1.0);
                    child.label_text_color = ov.and_then(|o| o.label_text_color).flatten();
                    child.tab_order_group = ov
                        .and_then(|o| o.tab_order_group)
                        .unwrap_or(slot.default_tab_order_group);
                    child.override_render_highlighted_sprite = ov
                        .and_then(|o| o.override_render_highlighted_sprite)
                        .flatten();
                    out.push(child);
                    continue;
                }
                if let Some(entry) = inner
                    .entries
                    .iter_mut()
                    .flatten()
                    .find(|entry| entry.id == id)
                {
                    if entry.disabled || !entry.owner.is_enabled() {
                        entry.disabled = true;
                        continue;
                    }
                    match &entry.kind {
                        MountedWidgetKind::Button { state, .. } => {
                            let mut child =
                                ScreenChild::new(id, state.rect, Some(state.message.clone()));
                            child.active = state.active;
                            child.visible = state.visible;
                            child.hovered = state.visible && point_in_rect(mouse, state.rect);
                            child.alpha = state.alpha;
                            child.label_text_color = state.label_text_color;
                            child.tab_order_group = state.tab_order_group;
                            child.override_render_highlighted_sprite =
                                state.override_render_highlighted_sprite;
                            out.push(child);
                        }
                        MountedWidgetKind::TextField {
                            edit_box,
                            active,
                            visible,
                            alpha,
                            tab_order_group,
                            ..
                        } => {
                            let rect = [edit_box.x, edit_box.y, edit_box.width, edit_box.height];
                            let message = (!edit_box.message.is_empty())
                                .then(|| ApiText::literal(edit_box.message.clone()));
                            let mut child = ScreenChild::new(id, rect, message);
                            child.active = *active;
                            child.visible = *visible;
                            child.hovered = *visible && point_in_rect(mouse, rect);
                            child.alpha = *alpha;
                            child.tab_order_group = *tab_order_group;
                            out.push(child);
                        }
                        MountedWidgetKind::Custom(widget) => {
                            if let Some(child) = call_owner(
                                &entry.owner,
                                &mut entry.disabled,
                                "screen_widget_snapshot",
                                || {
                                    let rect = widget.bounds();
                                    let mut child = ScreenChild::new(id, rect, None);
                                    child.active = widget.active();
                                    child.visible = widget.visible();
                                    child.hovered = child.visible && point_in_rect(mouse, rect);
                                    child.tab_order_group = widget.tab_order_group();
                                    child
                                },
                            ) {
                                out.push(child);
                            }
                        }
                    }
                }
            }
            out
        };
        self.client.screen_state.0.widgets.children_cache = snapshot;
    }

    /// Reconcile `engine_children` and `order` with the latest observed engine widgets (`seen`),
    /// preserving stable `ScreenWidgetId`s for existing keys, adding newly revealed sub-component
    /// widgets (such as an opened recipe book or switched tab), and pruning gone ones without
    /// disturbing mounted mod widgets.
    fn reconcile_engine_children(&mut self, seen: Vec<widgets::CollectedWidget>) {
        let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
        inner.needs_engine_resync = false;
        if inner.engine_overrides.suppress_all {
            return;
        }
        let mut new_slots = Vec::with_capacity(seen.len());
        let mut new_ids = Vec::with_capacity(seen.len());
        for item in seen {
            if inner
                .engine_overrides
                .overrides
                .get(&item.key)
                .is_some_and(|ov| ov.removed)
            {
                continue;
            }
            let id = if let Some(&existing) = inner.engine_id_by_key.get(&item.key) {
                existing
            } else {
                let fresh = self.alloc_widget_id();
                inner.engine_id_by_key.insert(item.key.clone(), fresh);
                fresh
            };
            let default_message = item.message.as_ref().and_then(|t| t.try_into_api().ok());
            new_ids.push(id);
            new_slots.push(EngineChildSlot {
                id,
                key: item.key,
                default_rect: item.rect,
                default_message,
                default_active: item.active,
                default_tab_order_group: item.tab_order_group,
            });
        }
        let old_ids: Vec<ScreenWidgetId> = inner.engine_children.iter().map(|s| s.id).collect();
        if old_ids != new_ids {
            inner
                .order
                .retain(|id| !old_ids.contains(id) || new_ids.contains(id));
            for (idx, &id) in new_ids.iter().enumerate() {
                if inner.order.contains(&id) {
                    continue;
                }
                let insert_at = new_ids[..idx]
                    .iter()
                    .rev()
                    .find_map(|prev| {
                        inner
                            .order
                            .iter()
                            .position(|cid| cid == prev)
                            .map(|p| p + 1)
                    })
                    .or_else(|| {
                        new_ids[idx + 1..]
                            .iter()
                            .find_map(|next| inner.order.iter().position(|cid| cid == next))
                    })
                    .unwrap_or(inner.order.len());
                inner.order.insert(insert_at, id);
            }
        }
        inner.engine_children = new_slots;
    }

    pub(in crate::game) fn is_redirecting_engine_click(&self) -> bool {
        REDIRECTING_ENGINE_CLICK.get()
    }

    fn with_redirecting_engine_click<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        struct Guard(bool);
        impl Drop for Guard {
            fn drop(&mut self) {
                REDIRECTING_ENGINE_CLICK.set(self.0);
            }
        }
        let _guard = Guard(REDIRECTING_ENGINE_CLICK.replace(true));
        f(self)
    }

    /// Live widget collection for the currently open screen (`ClientHost::screen_widgets`).
    pub fn screen_widgets(&mut self) -> Option<&mut dyn ScreenWidgets> {
        self.observed_screen_view()?;
        let (needs_discover, needs_resync) = {
            let inner = self.client.screen_state.0.widgets.inner.borrow();
            (
                !inner.discovered,
                inner.discovered && inner.needs_engine_resync,
            )
        };
        if needs_discover {
            let collected = self.discover_screen_widgets();
            let mut new_slots = Vec::with_capacity(collected.len());
            let mut new_ids = Vec::with_capacity(collected.len());
            let mut id_by_key = HashMap::with_capacity(collected.len());
            for item in collected {
                let id = self.alloc_widget_id();
                let default_message = item.message.as_ref().and_then(|t| t.try_into_api().ok());
                id_by_key.insert(item.key.clone(), id);
                new_ids.push(id);
                new_slots.push(EngineChildSlot {
                    id,
                    key: item.key,
                    default_rect: item.rect,
                    default_message,
                    default_active: item.active,
                    default_tab_order_group: item.tab_order_group,
                });
            }
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            let existing_order = std::mem::take(&mut inner.order);
            inner.order = new_ids;
            inner.order.extend(existing_order);
            inner.engine_children = new_slots;
            inner.engine_id_by_key = id_by_key;
            inner.engine_hits = build_engine_hits(&inner.engine_children, &inner.engine_overrides);
            inner.discovered = true;
            inner.needs_engine_resync = false;
            inner.overrides_active = true;
        } else {
            if needs_resync && !widgets::in_override_pass() {
                let collected = self.discover_screen_widgets();
                self.reconcile_engine_children(collected);
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                inner.engine_hits =
                    build_engine_hits(&inner.engine_children, &inner.engine_overrides);
            }
            self.client
                .screen_state
                .0
                .widgets
                .inner
                .borrow_mut()
                .overrides_active = true;
        }
        self.sync_children_cache();
        Some(self)
    }

    /// Resolved `tab_order_group` for an engine widget identified by collector key `key`.
    pub(in crate::game) fn engine_tab_order_group(&self, key: &widgets::EngineWidgetKey) -> i32 {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        if let Some(group) = inner
            .engine_overrides
            .overrides
            .get(key)
            .and_then(|ov| ov.tab_order_group)
        {
            return group;
        }
        inner
            .engine_children
            .iter()
            .find(|slot| &slot.key == key)
            .map_or(0, |slot| slot.default_tab_order_group)
    }

    /// Resolved `tab_order_group` for any focus `Target` (mounted or engine) by identity, never
    /// by comparing rectangles (`ContainerEventHandler.nextFocusPath`).
    pub(in crate::game) fn tab_order_group_for_target(
        &self,
        target: Target,
        meta: &widgets::TargetMeta,
    ) -> i32 {
        if let Kind::Mounted { id, .. } = target.kind {
            let wid = ScreenWidgetId::from_raw(id);
            let inner = self.client.screen_state.0.widgets.inner.borrow();
            if let Some(entry) = inner.entries.iter().flatten().find(|e| e.id == wid) {
                return match &entry.kind {
                    MountedWidgetKind::Button { state, .. } => state.tab_order_group,
                    MountedWidgetKind::TextField {
                        tab_order_group, ..
                    } => *tab_order_group,
                    MountedWidgetKind::Custom(_) => self
                        .client
                        .screen_state
                        .0
                        .widgets
                        .children_cache
                        .iter()
                        .find(|c| c.id == wid)
                        .map_or(0, |c| c.tab_order_group),
                };
            }
            return 0;
        }
        if let Some(key) = &meta.key {
            let inner = self.client.screen_state.0.widgets.inner.borrow();
            if let Some(group) = inner
                .engine_overrides
                .overrides
                .get(key)
                .and_then(|ov| ov.tab_order_group)
            {
                return group;
            }
        }
        meta.tab_order_group
    }

    /// Whether `screen_init_pre` cancelled engine widgets on the active screen.
    pub(in crate::game) fn engine_overrides_suppress_all(&self) -> bool {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        inner.overrides_active && inner.engine_overrides.suppress_all
    }

    /// Whether the engine widget identified by collector key `key` is currently non-interactive
    /// (removed, hidden, disabled, or suppressed by `screen_init_pre` cancellation).
    pub(in crate::game) fn engine_key_blocked(&self, key: &widgets::EngineWidgetKey) -> bool {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        if !inner.overrides_active {
            return false;
        }
        if inner.engine_overrides.suppress_all {
            return true;
        }
        let Some(ov) = inner.engine_overrides.overrides.get(key) else {
            return false;
        };
        ov.removed || ov.visible == Some(false) || ov.active == Some(false)
    }

    /// Run a scoped collector pass over the currently open engine screen or overlay to enumerate
    /// its built-in widgets in exact draw/registration order, preserving any rasterized font or
    /// pack icon atlas patches without touching live widget/target/narration buffers.
    pub(in crate::game) fn discover_screen_widgets(&mut self) -> Vec<widgets::CollectedWidget> {
        let size = self.renderer_framebuffer().unwrap_or([854, 480]);
        let mut batch = HudBatch::new(size, self.client.settings.gui_scale_inputs());
        let prev_room = std::mem::replace(
            &mut self.client.chat_ui.room,
            sunrise_client::hud_kit::room::Room::new(batch.width, batch.height),
        );
        let prev_frame_ticks = std::mem::replace(&mut self.client.frame_ticks, 0.0);
        let ((), collected) = widgets::with_widget_collector(|| {
            if self.client.command_prompt.0.is_some() {
                self.draw_command_prompt(&mut batch);
            } else if self.client.link_prompt.is_some() {
                self.draw_link_prompt(&mut batch);
            } else if self.client.code_of_conduct.is_some() {
                self.draw_code_of_conduct(&mut batch);
            } else if self.client.server_packs.0.prompt.is_some() {
                self.draw_pack_prompt(&mut batch);
            } else if self.dialog_open() {
                self.draw_dialog(&mut batch);
            } else if self.client.book.is_some() {
                self.draw_book(&mut batch);
            } else if self.client.sign_edit.is_some() {
                self.draw_sign_editor(&mut batch);
            } else if self.client.screen == super::Screen::Win {
                self.draw_win_screen(&mut batch);
            } else if self.client.screen == super::Screen::Options && self.client.title.0.is_some()
            {
                self.draw_options(&mut batch, true);
            } else if self.client.screen == super::Screen::DebugOptions
                && self.client.title.0.is_some()
            {
                self.draw_debug_options(&mut batch);
            } else if self.client.screen == super::Screen::Mod && self.client.title.0.is_some() {
                // Mod-owned screens have no built-in engine widgets.
            } else if let Some(title) = &self.client.title.0 {
                title.draw_with_items(
                    &self.client.gui.atlas,
                    Some(&self.client.client_pack.item_renderer),
                    &mut batch,
                    self.client.cursor,
                    self.client.settings.file.bool("hideSplashTexts"),
                    self.client.settings.file.bool("hideServerAddress"),
                    &mut |_, _| {},
                );
            } else if self.remote_loading() {
                let _ = self.remote_overlay(&mut batch);
            } else {
                match self.client.screen {
                    super::Screen::Options => self.draw_options(&mut batch, true),
                    super::Screen::Paused if self.client.pause_menu => {
                        self.draw_pause_menu(&mut batch)
                    }
                    super::Screen::GameModeSwitcher => self.draw_gamemode_switcher(&mut batch),
                    super::Screen::Dead => self.draw_death_screen(&mut batch),
                    super::Screen::Stats => self.draw_stats(&mut batch),
                    super::Screen::Advancements => self.draw_advancements(&mut batch),
                    super::Screen::WorldOptions => self.draw_world_options(&mut batch),
                    super::Screen::Friends => self.draw_friends_screen(&mut batch),
                    super::Screen::Social => self.draw_social_screen(&mut batch),
                    super::Screen::ServerLinks => self.draw_server_links(&mut batch),
                    super::Screen::ConfirmLink => self.draw_confirm_link(&mut batch),
                    super::Screen::DemoIntro => self.draw_demo_intro(&mut batch),
                    super::Screen::DialogWarning => self.draw_dialog_warning(&mut batch),
                    super::Screen::CommandBlock => self.draw_command_block(&mut batch),
                    super::Screen::BlockEditor => self.draw_block_editor(&mut batch),
                    super::Screen::DebugOptions => self.draw_debug_options(&mut batch),
                    super::Screen::Inventory => {
                        let _ = self.draw_inventory(&mut batch);
                    }
                    super::Screen::Creative => {
                        let _ = self.draw_creative(&mut batch);
                    }
                    super::Screen::Chat => self.draw_chat_screen(&mut batch),
                    super::Screen::Playing
                    | super::Screen::Win
                    | super::Screen::Paused
                    | super::Screen::Mod => {}
                }
            }
        });
        self.client.frame_ticks = prev_frame_ticks;
        self.client.chat_ui.room = prev_room;
        // Keep any newly rasterized font glyphs or pack icons queued for GPU upload.
        self.client
            .text_patches
            .borrow_mut()
            .append(&mut batch.patches);
        collected
    }

    /// Re-layout engine widgets after a resize on a screen whose vanilla counterpart overrides
    /// `repositionElements()`: clear only engine bounds overrides while keeping all non-bounds
    /// overrides and all mounted mod widgets (with their bounds) intact.
    pub(in crate::game) fn reposition_screen_widgets(&mut self) {
        let should_rediscover = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            for ov in inner.engine_overrides.overrides.values_mut() {
                ov.rect = None;
            }
            inner.discovered && !inner.engine_overrides.suppress_all
        };
        if should_rediscover {
            let collected = self.discover_screen_widgets();
            self.reconcile_engine_children(collected);
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            inner.engine_hits = build_engine_hits(&inner.engine_children, &inner.engine_overrides);
        }
        self.sync_children_cache();
    }

    /// Run a screen draw closure under the active screen's engine widget override table when
    /// overrides or widget inspection are active for this screen instance.
    pub(in crate::game) fn with_screen_widget_overrides<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let active = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .overrides_active;
        if !active || widgets::in_override_pass() {
            return f(self);
        }
        let cursor = self.client.cursor;
        let now = Instant::now();
        let mut maps = std::mem::take(
            &mut self
                .client
                .screen_state
                .0
                .widgets
                .inner
                .borrow_mut()
                .engine_overrides,
        );
        let (result, output) = widgets::with_widget_overrides(&mut maps, cursor, now, || f(self));
        {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            inner.engine_overrides = maps;
            if !output.render_vetoed {
                inner.engine_hits = output.hits;
                inner.engine_hovered = output.hovered;
                if output.pending_tooltip.is_some() {
                    inner.pending_tooltip = output.pending_tooltip;
                }
            }
        }
        if !output.render_vetoed {
            self.reconcile_engine_children(output.seen);
        }
        result
    }

    /// Resolve how a pointer click at `cursor` interacts with active engine widget overrides.
    pub(in crate::game) fn resolve_engine_click_cursor(
        &self,
        cursor: (f32, f32),
    ) -> EngineClickRoute {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        if !inner.overrides_active {
            return EngineClickRoute::PassThrough;
        }
        let pt = [cursor.0, cursor.1];
        if inner.engine_overrides.suppress_all {
            if inner
                .engine_hits
                .iter()
                .any(|hit| point_in_rect(pt, hit.original_rect))
            {
                return EngineClickRoute::Block;
            }
            return EngineClickRoute::PassThrough;
        }
        if let Some(hit) = inner
            .engine_hits
            .iter()
            .rev()
            .find(|hit| {
                hit.interactive
                    && hit.resolved_rect != hit.original_rect
                    && point_in_rect(pt, hit.resolved_rect)
            })
            .or_else(|| {
                inner
                    .engine_hits
                    .iter()
                    .rev()
                    .find(|hit| hit.interactive && point_in_rect(pt, hit.resolved_rect))
            })
        {
            return EngineClickRoute::Select {
                key: hit.key.clone(),
                resolved: hit.resolved_rect,
                original: hit.original_rect,
            };
        }
        if inner
            .engine_hits
            .iter()
            .any(|hit| hit.blocks_original && point_in_rect(pt, hit.original_rect))
        {
            return EngineClickRoute::Block;
        }
        EngineClickRoute::PassThrough
    }

    /// Run an input handler with a snapshot of the active screen's `EngineOverrideMaps` exposed
    /// via `widgets::resolve_engine_input`.
    pub(in crate::game) fn with_screen_input_overrides<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let maps = {
            let inner = self.client.screen_state.0.widgets.inner.borrow();
            inner
                .overrides_active
                .then(|| inner.engine_overrides.clone())
        };
        let Some(mut maps) = maps else {
            return f(self);
        };
        widgets::with_input_overrides(&mut maps, || f(self))
    }

    fn map_redirect_cursor(
        cursor: (f32, f32),
        [rx, ry, rw, rh]: [f32; 4],
        [ox, oy, ow, oh]: [f32; 4],
    ) -> (f32, f32) {
        let u = (cursor.0 - rx) / rw;
        let v = (cursor.1 - ry) / rh;
        (ox + u * ow, oy + v * oh)
    }

    fn dispatch_selected_engine_target<R>(
        &mut self,
        target: &ActiveEngineTarget,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if let Some((resolved, original)) = target.redirect {
            let prev = self.client.cursor;
            self.client.cursor = Self::map_redirect_cursor(prev, resolved, original);
            let res = widgets::with_selected_engine_widget(&target.key, || {
                self.with_redirecting_engine_click(|game| game.with_screen_input_overrides(f))
            });
            self.client.cursor = prev;
            res
        } else {
            widgets::with_selected_engine_widget(&target.key, || {
                self.with_screen_input_overrides(f)
            })
        }
    }

    /// Route a real mouse drag through any active selected/redirected engine widget before calling
    /// [`Self::screen_drag_response`].
    pub(in crate::game) fn screen_drag_response_with_overrides(
        &mut self,
        overlay: Option<&str>,
        button: MouseButton,
        framebuffer: [u32; 2],
        gui: (f32, f32),
    ) -> Result<bool> {
        let options_screen =
            self.observed_overlay_now().is_none() && self.client.screen == super::Screen::Options;
        if options_screen {
            return self.with_screen_input_overrides(|game| {
                game.screen_drag_response(overlay, button, framebuffer, gui)
            });
        }
        let target = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .active_redirect
            .clone();
        if let Some(target) = target {
            self.dispatch_selected_engine_target(&target, |game| {
                game.screen_drag_response(overlay, button, framebuffer, gui)
            })
        } else {
            self.with_screen_input_overrides(|game| {
                game.screen_drag_response(overlay, button, framebuffer, gui)
            })
        }
    }

    /// Route a real mouse button edge through active engine widget overrides before calling
    /// [`Self::route_screen_button`], so moved engine widgets click, drag, and release at their
    /// new rectangle exclusively, removed/hidden/disabled/suppressed widgets never activate, and
    /// container screens still dispatch slots when an engine widget is blocked.
    pub(in crate::game) fn route_screen_button_with_overrides(
        &mut self,
        button: MouseButton,
        code: i32,
        pressed: bool,
        double_click: bool,
    ) -> Result<bool> {
        let options_screen =
            self.observed_overlay_now().is_none() && self.client.screen == super::Screen::Options;
        if options_screen {
            let generation = self.client.screen_state.0.widgets.generation();
            let res = self.with_screen_input_overrides(|game| {
                game.route_screen_button(button, code, pressed, double_click)
            });
            if pressed
                && matches!(res, Ok(true))
                && self.client.screen_state.0.widgets.generation() == generation
            {
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                if inner.discovered {
                    inner.needs_engine_resync = true;
                }
            }
            return res;
        }
        if !pressed {
            let target = self
                .client
                .screen_state
                .0
                .widgets
                .inner
                .borrow_mut()
                .active_redirect
                .take();
            if let Some(target) = target {
                return self.dispatch_selected_engine_target(&target, |game| {
                    game.route_screen_button(button, code, false, double_click)
                });
            }
            return self.with_screen_input_overrides(|game| {
                game.route_screen_button(button, code, false, double_click)
            });
        }
        let generation = self.client.screen_state.0.widgets.generation();
        let direct_override_screen = self.title_input_active()
            || (self.observed_overlay_now().is_none()
                && self.client.screen == super::Screen::Paused
                && self.client.pause_menu);
        let container_screen = self.observed_overlay_now().is_none()
            && matches!(
                self.client.screen,
                super::Screen::Inventory | super::Screen::Creative
            );
        let res = match self.resolve_engine_click_cursor(self.client.cursor) {
            EngineClickRoute::PassThrough => {
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow_mut()
                    .active_redirect = None;
                self.with_screen_input_overrides(|game| {
                    game.route_screen_button(button, code, true, double_click)
                })
            }
            EngineClickRoute::Block if direct_override_screen => {
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow_mut()
                    .active_redirect = None;
                self.with_screen_input_overrides(|game| {
                    game.route_screen_button(button, code, true, double_click)
                })
            }
            EngineClickRoute::Block if container_screen => {
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow_mut()
                    .active_redirect = None;
                widgets::with_blocking_engine_widget_click(|| {
                    self.with_screen_input_overrides(|game| {
                        game.route_screen_button(button, code, true, double_click)
                    })
                })
            }
            EngineClickRoute::Block => {
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow_mut()
                    .active_redirect = None;
                Ok(false)
            }
            EngineClickRoute::Select {
                key,
                resolved,
                original,
            } => {
                let redirect = (!direct_override_screen && resolved != original)
                    .then_some((resolved, original));
                let target = ActiveEngineTarget { key, redirect };
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow_mut()
                    .active_redirect = Some(target.clone());
                let res = self.dispatch_selected_engine_target(&target, |game| {
                    game.route_screen_button(button, code, true, double_click)
                });
                if !matches!(res, Ok(true)) {
                    self.client
                        .screen_state
                        .0
                        .widgets
                        .inner
                        .borrow_mut()
                        .active_redirect = None;
                }
                res
            }
        };
        if matches!(res, Ok(true)) && self.client.screen_state.0.widgets.generation() == generation
        {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            if inner.discovered {
                inner.needs_engine_resync = true;
            }
        }
        res
    }

    /// Paint any deferred tooltip scheduled by an overridden engine widget or mounted widget
    /// during the current screen pass, before `ScreenEvent.Render.Post`.
    pub(in crate::game) fn draw_pending_widget_tooltip(&self, batch: &mut HudBatch) {
        let pending = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow_mut()
            .pending_tooltip
            .take()
            .or_else(widgets::take_active_pending_tooltip);
        let Some(pending) = pending else {
            return;
        };
        let atlas = &self.client.gui.atlas;
        let lines = atlas.font.split(&pending.tooltip.message, 170.0);
        if let Some(component) = pending.tooltip.component {
            let mut items: Vec<PreparedTooltipItem> = lines
                .into_iter()
                .map(|text| PreparedTooltipItem::Text {
                    text,
                    color: u32::MAX,
                })
                .collect();
            if let Ok(prepared) = self.prepare_tooltip_component(&component) {
                items.push(prepared);
            }
            let bounds = pending.rect.map(|v| v.round() as i32);
            let positioner = if pending.focused_only {
                WidgetTooltipPositioner::BelowOrAbove(bounds)
            } else {
                WidgetTooltipPositioner::Menu(bounds)
            };
            let anchor = quantize_tooltip_anchor(self.widget_tooltip_cursor());
            self.draw_prepared_tooltip(batch, None, &mut items, anchor, &positioner, false);
            return;
        }
        let style = pending
            .tooltip
            .style
            .as_ref()
            .map(|id| (id.namespace(), id.path()));
        if pending.focused_only {
            ui::tooltip_focused_styled(batch, atlas, &lines, style, pending.rect);
        } else {
            ui::tooltip_menu_styled(
                batch,
                atlas,
                &lines,
                style,
                self.widget_tooltip_cursor(),
                pending.rect,
            );
        }
    }

    /// Run `ScreenEvent.Init.Pre`, discover the open screen's built-in widgets unless cancelled,
    /// run `ScreenEvent.Init.Post`, and commit mounted widgets, engine overrides, and initial focus.
    pub fn init_screen_widgets(&mut self, view: &ScreenView<'_>) {
        let listen_pre = mod_api::__private::listened(mod_api::HookId::ScreenInitPre);
        let listen_post = mod_api::__private::listened(mod_api::HookId::ScreenInitPost);
        if !listen_pre && !listen_post {
            return;
        }
        let mut mount = ScreenWidgetMount {
            next_id: self.client.screen_state.0.widgets.next_id.get(),
            entries: Vec::new(),
            children: Vec::new(),
            engine_ids: Vec::new(),
            field_ids: Vec::new(),
            custom_ids: Vec::new(),
            removed: Vec::new(),
            ops: Vec::new(),
            initial_focus: None,
        };
        let mut entries: Vec<MountedWidgetEntry> = Vec::new();
        let mut engine_children: Vec<EngineChildSlot> = Vec::new();
        let mut engine_overrides = widgets::EngineOverrideMaps::default();
        let mut initial_focus: Option<ScreenWidgetId> = None;

        let commit_listener =
            |mount: &mut ScreenWidgetMount,
             entries: &mut Vec<MountedWidgetEntry>,
             engine_children: &[EngineChildSlot],
             engine_overrides: &mut widgets::EngineOverrideMaps,
             initial_focus: &mut Option<ScreenWidgetId>| {
                for id in mount.removed.drain(..) {
                    if *initial_focus == Some(id) {
                        *initial_focus = None;
                    }
                    if let Some(pos) = entries.iter().position(|entry| entry.id == id) {
                        entries.remove(pos).dispose();
                    } else if let Some(slot) = engine_children.iter().find(|s| s.id == id) {
                        engine_overrides
                            .overrides
                            .entry(slot.key.clone())
                            .or_default()
                            .removed = true;
                    }
                }
                for (id, op) in mount.ops.drain(..) {
                    if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                        apply_widget_op_to_mounted(entry, op);
                    } else if let Some(slot) = engine_children.iter().find(|s| s.id == id) {
                        let ov = engine_overrides
                            .overrides
                            .entry(slot.key.clone())
                            .or_default();
                        apply_widget_op_to_engine(ov, op);
                    }
                }
                *initial_focus = mount.initial_focus.take().or(*initial_focus);
                entries.append(&mut mount.entries);
            };

        let mut pre_canceled = false;
        if listen_pre {
            self.native_screen_init_pre(
                view,
                |listen| {
                    let children = mount.children.clone();
                    let field_ids = mount.field_ids.clone();
                    let custom_ids = mount.custom_ids.clone();
                    if listen(&mut mount) {
                        commit_listener(
                            &mut mount,
                            &mut entries,
                            &engine_children,
                            &mut engine_overrides,
                            &mut initial_focus,
                        );
                    } else {
                        mount.children = children;
                        mount.field_ids = field_ids;
                        mount.custom_ids = custom_ids;
                        mount.initial_focus = None;
                        mount.removed.clear();
                        mount.ops.clear();
                        for entry in mount.entries.drain(..) {
                            entry.dispose();
                        }
                    }
                },
                &mut pre_canceled,
            );
        }

        let mut suppressed_hits: Vec<widgets::EngineHitEntry> = Vec::new();
        if pre_canceled {
            engine_overrides.suppress_all = true;
            // Pure discovery populates suppressed hit rects immediately so a press before the
            // first draw cannot reach the original engine controls, while keeping `children()`
            // free of engine widgets.
            for item in self.discover_screen_widgets() {
                suppressed_hits.push(widgets::EngineHitEntry {
                    key: item.key,
                    original_rect: item.rect,
                    resolved_rect: item.rect,
                    interactive: false,
                    blocks_original: true,
                });
            }
        } else {
            let collected = self.discover_screen_widgets();
            for item in collected {
                let id = mount.alloc_id();
                let api_msg = item.message.as_ref().and_then(|t| t.try_into_api().ok());
                let mut child = ScreenChild::new(id, item.rect, api_msg.clone());
                child.active = item.active;
                child.tab_order_group = item.tab_order_group;
                mount.children.push(child);
                mount.engine_ids.push(id);
                engine_children.push(EngineChildSlot {
                    id,
                    key: item.key,
                    default_rect: item.rect,
                    default_message: api_msg,
                    default_active: item.active,
                    default_tab_order_group: item.tab_order_group,
                });
            }
        }

        if listen_post {
            self.native_screen_init_post(view, |listen| {
                let children = mount.children.clone();
                let field_ids = mount.field_ids.clone();
                let custom_ids = mount.custom_ids.clone();
                if listen(&mut mount) {
                    commit_listener(
                        &mut mount,
                        &mut entries,
                        &engine_children,
                        &mut engine_overrides,
                        &mut initial_focus,
                    );
                } else {
                    mount.children = children;
                    mount.field_ids = field_ids;
                    mount.custom_ids = custom_ids;
                    mount.initial_focus = None;
                    mount.removed.clear();
                    mount.ops.clear();
                    for entry in mount.entries.drain(..) {
                        entry.dispose();
                    }
                }
                // IDs issued to a failed listener are never reused.
            });
        }

        self.client
            .screen_state
            .0
            .widgets
            .next_id
            .set(mount.next_id);
        let initial_focus = initial_focus.and_then(|id| {
            entries
                .iter_mut()
                .position(|entry| entry.id == id && entry.can_focus())
        });
        let order: Vec<ScreenWidgetId> = mount.children.iter().map(|c| c.id).collect();
        let engine_hits = if pre_canceled {
            suppressed_hits
        } else {
            build_engine_hits(&engine_children, &engine_overrides)
        };
        let engine_id_by_key: HashMap<widgets::EngineWidgetKey, ScreenWidgetId> = engine_children
            .iter()
            .map(|slot| (slot.key.clone(), slot.id))
            .collect();
        let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
        inner.order = order;
        inner.engine_children = engine_children;
        inner.engine_id_by_key = engine_id_by_key;
        inner.engine_overrides = engine_overrides;
        inner.engine_hits = engine_hits;
        inner.entries = entries.into_iter().map(Some).collect();
        inner.discovered = true;
        inner.needs_engine_resync = false;
        inner.overrides_active = true;
        drop(inner);
        self.sync_children_cache();
        if let Some(idx) = initial_focus {
            self.set_mounted_widget_focus(Some(idx));
        }
    }

    /// Clear and dispose all mounted widgets for the current screen without
    /// discarding `last_click` (same-instance resize rebuilds widgets while
    /// preserving `MouseHandler.LastClick`).
    pub fn clear_screen_widgets(&mut self) {
        self.client.screen_state.0.widgets.clear();
        self.client.screen_state.0.widgets.children_cache.clear();
        self.reset_screen_ime();
    }

    pub(in crate::game) fn reset_screen_ime(&mut self) {
        sunrise_client::preedit::reset();
        self.client.ime_area = None;
    }

    /// Remove focus from any currently focused mounted widget.
    pub fn clear_mounted_widget_focus(&mut self) {
        self.set_mounted_widget_focus(None);
    }

    pub(in crate::game) fn set_mounted_widget_focus(&mut self, target: Option<usize>) {
        let previous = self.client.screen_state.0.widgets.focused_index();
        if previous == target {
            return;
        }
        sunrise_client::preedit::reset();
        self.client.ime_area = None;
        self.client
            .screen_state
            .0
            .widgets
            .inner
            .borrow_mut()
            .focused = target;
        if let Some(old_idx) = previous {
            let disabled = {
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                inner
                    .entries
                    .get_mut(old_idx)
                    .and_then(Option::as_mut)
                    .is_some_and(|entry| {
                        entry.apply_focus(false);
                        entry.disabled
                    })
            };
            if disabled {
                self.client.screen_state.0.widgets.clear_links_for(old_idx);
            }
        }
        if let Some(new_idx) = target {
            let (ok, widget_target) = {
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                match inner.entries.get_mut(new_idx).and_then(Option::as_mut) {
                    Some(entry) => {
                        let ok = entry.apply_focus(true);
                        let t = if ok { entry.target() } else { None };
                        (ok, t)
                    }
                    None => (false, None),
                }
            };
            if !ok {
                self.client.screen_state.0.widgets.clear_links_for(new_idx);
            } else if let Some(t) = widget_target {
                self.clear_screen_click_focus();
                self.client.pad.0.menu.origin = Some(t.rect);
                if self.client.pad.0.menu.keyboard {
                    self.client.pad.0.menu.focus = Some(t);
                }
            }
        }
    }

    /// All visible, active mounted widgets as unified focus/pad targets.
    pub fn mounted_widget_targets(&self) -> Vec<Target> {
        let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
        let mut out = Vec::new();
        let mut disabled = Vec::new();
        for (idx, slot) in inner.entries.iter_mut().enumerate() {
            if let Some(entry) = slot.as_mut() {
                if let Some(t) = entry.target() {
                    out.push(t);
                } else if entry.disabled {
                    disabled.push(idx);
                }
            }
        }
        drop(inner);
        for idx in disabled {
            self.client.screen_state.0.widgets.clear_links_for(idx);
        }
        out
    }

    /// Synchronize with the selected child, not whichever widget overlaps its bounds.
    pub fn sync_mounted_focus(&mut self, target: Target) {
        let idx = match target.kind {
            Kind::Mounted { id, .. } => self
                .client
                .screen_state
                .0
                .widgets
                .index_of(ScreenWidgetId::from_raw(id)),
            _ => None,
        };
        self.set_mounted_widget_focus(idx);
    }

    /// Temporarily extract mounted widget `idx` so its callback can receive `&mut dyn ClientHost`
    /// and execute screen transitions synchronously (`Gui.setScreen` / `Button.onPress`).
    /// If the screen's widget generation changed during `f`, the extracted entry is disposed
    /// instead of restored onto the new screen.
    pub fn with_mounted_widget<R>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&mut Self, &mut MountedWidgetEntry) -> R,
    ) -> Option<R> {
        let generation = self.client.screen_state.0.widgets.generation();
        let (mut entry, focused) = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            (
                inner.entries.get_mut(idx)?.take()?,
                inner.focused == Some(idx),
            )
        };
        if entry.disabled || !entry.owner.is_enabled() {
            entry.disabled = true;
            self.client.screen_state.0.widgets.clear_links_for(idx);
            self.client
                .screen_state
                .0
                .widgets
                .inner
                .borrow_mut()
                .entries[idx] = Some(entry);
            return None;
        }
        let entry_id = entry.id;
        let result = f(self, &mut entry);
        let still_same_screen = self.client.screen_state.0.widgets.generation() == generation;
        if still_same_screen {
            let restored = {
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                if inner.order.contains(&entry_id)
                    && inner.entries.get(idx).is_some_and(Option::is_none)
                {
                    if entry.disabled || !entry.owner.is_enabled() {
                        entry.disabled = true;
                        if inner.focused == Some(idx) {
                            inner.focused = None;
                        }
                        if inner.dragging.is_some_and(|(drag_idx, _)| drag_idx == idx) {
                            inner.dragging = None;
                        }
                    }
                    inner.entries[idx] = Some(entry);
                    None
                } else {
                    Some(entry)
                }
            };
            if let Some(mut entry) = restored {
                if focused {
                    entry.apply_focus(false);
                }
                entry.dispose();
            }
        } else {
            // Teardown could not blur this entry while it was out of the table.
            if focused {
                entry.apply_focus(false);
            }
            entry.dispose();
        }
        Some(result)
    }

    /// Draw every visible mounted widget inside the receiving screen's
    /// Pre-guarded contents pass, before Foreground and tooltips
    /// (`Screen.extractRenderState`).
    pub fn draw_mounted_screen_widgets(&self, batch: &mut HudBatch) {
        self.draw_mounted_screen_widgets_at(batch, Instant::now());
    }

    pub fn draw_mounted_screen_widgets_at(&self, batch: &mut HudBatch, now: Instant) {
        widgets::without_engine_overrides(|| {
            self.draw_mounted_screen_widgets_inner(batch, now);
        });
    }

    fn draw_mounted_screen_widgets_inner(&self, batch: &mut HudBatch, now: Instant) {
        let len = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .entries
            .len();
        if len == 0 {
            return;
        }
        let mouse = self.cursor();
        let partial_tick = self.common.clock.fraction() as f32;
        let focused_idx = self.client.screen_state.0.widgets.focused_index();
        let generation = self.client.screen_state.0.widgets.generation();
        let mut next_hover_since = HashMap::new();
        let prev_hover_since = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .mounted_hover_since
            .clone();
        let mut scheduled_tooltip: Option<widgets::PendingWidgetTooltip> = None;
        for idx in 0..len {
            if self.client.screen_state.0.widgets.generation() != generation {
                break;
            }
            let Some(mut entry) = self
                .client
                .screen_state
                .0
                .widgets
                .inner
                .borrow_mut()
                .entries
                .get_mut(idx)
                .and_then(Option::take)
            else {
                continue;
            };
            let focused = focused_idx == Some(idx);
            if !entry.disabled && entry.owner.is_enabled() {
                match &mut entry.kind {
                    MountedWidgetKind::Button {
                        state, narration, ..
                    } => {
                        if state.visible {
                            let mark = widgets::frame_mark();
                            let hovered = point_in_rect(mouse, state.rect);
                            let label: Text =
                                state.message.try_into_engine(&()).unwrap_or_default();
                            let custom_narration = narration.as_ref().and_then(|n| {
                                let t: Text = n.try_into_engine(&()).ok()?;
                                Some(t.plain())
                            });
                            let converted_tip =
                                convert_widget_tooltip(state.tooltip.clone()).flatten();
                            let tooltip_hint = converted_tip.as_ref().map(|t| t.narration.as_str());
                            if hovered || (focused && self.key_focus_driving()) {
                                let started =
                                    prev_hover_since.get(&entry.id).copied().unwrap_or(now);
                                next_hover_since.insert(entry.id, started);
                                if let Some(tip) = converted_tip.clone()
                                    && now.duration_since(started) >= state.tooltip_delay
                                {
                                    scheduled_tooltip = Some(widgets::PendingWidgetTooltip {
                                        tooltip: tip,
                                        rect: state.rect,
                                        focused_only: !hovered,
                                    });
                                }
                            }
                            ui::button_mounted(
                                batch,
                                &self.client.gui.atlas,
                                state.rect,
                                &label,
                                ui::ButtonVisualState {
                                    active: state.active,
                                    hovered,
                                    focused,
                                },
                                ui::ButtonStyle {
                                    alpha: state.alpha,
                                    label_text_color: state.label_text_color,
                                    sprites: state.sprites.as_ref(),
                                    override_render_highlighted_sprite: state
                                        .override_render_highlighted_sprite,
                                    tab_order_group: state.tab_order_group,
                                    tooltip_narration: tooltip_hint,
                                    ..ui::ButtonStyle::default()
                                },
                                |rect| match custom_narration {
                                    Some(title) => Narratable::named(title, rect, state.active),
                                    None => Narratable::button(label.plain(), rect, state.active),
                                },
                            );
                            widgets::mark_mounted(mark, entry.id.raw(), false);
                        }
                    }
                    MountedWidgetKind::TextField {
                        edit_box,
                        hint,
                        active,
                        visible,
                        alpha,
                        tooltip,
                        tooltip_delay,
                        tab_order_group,
                        ..
                    } => {
                        if *visible {
                            let rect = [edit_box.x, edit_box.y, edit_box.width, edit_box.height];
                            let hovered = point_in_rect(mouse, rect);
                            let converted_tip = convert_widget_tooltip(tooltip.clone()).flatten();
                            let tooltip_hint = converted_tip.as_ref().map(|t| t.narration.as_str());
                            if hovered || (focused && self.key_focus_driving()) {
                                let started =
                                    prev_hover_since.get(&entry.id).copied().unwrap_or(now);
                                next_hover_since.insert(entry.id, started);
                                if let Some(tip) = converted_tip.clone()
                                    && now.duration_since(started) >= *tooltip_delay
                                {
                                    scheduled_tooltip = Some(widgets::PendingWidgetTooltip {
                                        tooltip: tip,
                                        rect,
                                        focused_only: !hovered,
                                    });
                                }
                            }
                            edit_box.focused = focused && *active;
                            let mark = widgets::frame_mark();
                            edit_box.draw_mounted_at(
                                batch,
                                &self.client.gui.atlas,
                                now,
                                ui::EditBoxDrawStyle {
                                    active: *active,
                                    alpha: *alpha,
                                    tab_order_group: *tab_order_group,
                                    tooltip_hint,
                                },
                            );
                            widgets::mark_mounted(mark, entry.id.raw(), true);
                            if let Some(hint) = hint
                                && edit_box.value.is_empty()
                                && !edit_box.focused
                            {
                                let color = widgets::scale_abgr_alpha(
                                    if edit_box.editable && *active {
                                        edit_box.text_color
                                    } else {
                                        edit_box.text_color_uneditable
                                    },
                                    *alpha,
                                );
                                if (color >> 24) != 0 {
                                    batch.text_styled(
                                        &self.client.gui.atlas,
                                        hint,
                                        edit_box.text_x(),
                                        edit_box.text_y(),
                                        color,
                                        true,
                                    );
                                }
                            }
                        }
                    }
                    MountedWidgetKind::Custom(widget) => {
                        let paint = batch.paint_checkpoint();
                        let mut custom_tip = None;
                        let outcome = call_owner(
                            &entry.owner,
                            &mut entry.disabled,
                            "screen_widget_render",
                            || {
                                if !widget.visible() {
                                    return;
                                }
                                let bounds = widget.bounds();
                                if !valid_widget_rect(bounds) {
                                    return;
                                }
                                if widget.active() {
                                    widgets::target_as(
                                        bounds,
                                        Kind::Mounted {
                                            id: entry.id.raw(),
                                            field: widget.captures_input(),
                                        },
                                    );
                                }
                                let hovered = point_in_rect(mouse, bounds);
                                if hovered || (focused && self.key_focus_driving()) {
                                    custom_tip =
                                        widget.tooltip().map(|tip| (tip, bounds, !hovered));
                                }
                                let host: &crate::game::ClientGame<'_> = self;
                                let mut draw = GameDrawContext::new(host, batch);
                                widget.render(
                                    host,
                                    &mut draw,
                                    mouse,
                                    hovered,
                                    focused,
                                    partial_tick,
                                );
                            },
                        );
                        if outcome.is_none() {
                            paint.discard(batch);
                        } else if let Some((tip, rect, focused_only)) = custom_tip
                            && let Some(Some(converted)) = convert_widget_tooltip(Some(tip))
                        {
                            scheduled_tooltip = Some(widgets::PendingWidgetTooltip {
                                tooltip: converted,
                                rect,
                                focused_only,
                            });
                        }
                    }
                }
            }
            if self.client.screen_state.0.widgets.generation() == generation {
                if entry.disabled || !entry.owner.is_enabled() {
                    entry.disabled = true;
                    self.client.screen_state.0.widgets.clear_links_for(idx);
                }
                let restored = {
                    let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                    if let Some(slot) = inner.entries.get_mut(idx) {
                        *slot = Some(entry);
                        None
                    } else {
                        Some(entry)
                    }
                };
                if let Some(entry) = restored {
                    entry.dispose();
                }
            } else {
                entry.dispose();
                break;
            }
        }
        if self.client.screen_state.0.widgets.generation() == generation {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            inner.mounted_hover_since = next_hover_since;
            if scheduled_tooltip.is_some() {
                inner.pending_tooltip = scheduled_tooltip;
            }
        }
    }

    /// Notify custom mounted widgets of pointer movement (`GuiEventListener.mouseMoved`).
    pub fn mounted_widget_mouse_moved(&mut self, mouse: [f32; 2]) {
        let len = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .entries
            .len();
        if len == 0 {
            return;
        }
        let generation = self.client.screen_state.0.widgets.generation();
        for idx in 0..len {
            if self.client.screen_state.0.widgets.generation() != generation {
                return;
            }
            let is_custom = matches!(
                self.client
                    .screen_state
                    .0
                    .widgets
                    .inner
                    .borrow()
                    .entries
                    .get(idx)
                    .and_then(Option::as_ref),
                Some(entry)
                    if !entry.disabled
                        && entry.owner.is_enabled()
                        && matches!(entry.kind, MountedWidgetKind::Custom(_))
            );
            if !is_custom {
                continue;
            }
            self.with_mounted_widget(idx, |game, entry| {
                if let MountedWidgetKind::Custom(widget) = &mut entry.kind {
                    call_owner(
                        &entry.owner,
                        &mut entry.disabled,
                        "screen_widget_mouse_moved",
                        || {
                            if widget.visible() && widget.active() {
                                widget.mouse_moved(game, mouse);
                            }
                        },
                    );
                }
            });
        }
    }

    /// Dispatch a mouse click to the first hit mounted child in insertion order.
    /// A hit consumes the click independently of whether the child takes focus
    /// (`ContainerEventHandler.mouseClicked`). A miss leaves focus alone.
    pub fn mounted_widget_mouse_clicked(
        &mut self,
        mouse: [f32; 2],
        button: i32,
        modifiers: u32,
        double_click: bool,
    ) -> bool {
        self.mounted_widget_click(None, mouse, button, modifiers, double_click)
    }

    /// Controller focus names the child; coincident bounds are only a pointer concern.
    pub(in crate::game::client_side) fn activate_mounted_widget(
        &mut self,
        id: ScreenWidgetId,
        (x, y): (f32, f32),
    ) -> bool {
        let generation = self.client.screen_state.0.widgets.generation();
        let mouse = [x, y];
        let modifiers = crate::game::client_side::key_codes::glfw_modifiers(self.client.modifiers);
        let handled = self.mounted_widget_click(Some(id), mouse, 0, modifiers, false);
        if handled && self.client.screen_state.0.widgets.generation() == generation {
            self.mounted_widget_mouse_released(mouse, 0, modifiers);
        }
        handled
    }

    pub(in crate::game) fn mounted_widget_click(
        &mut self,
        id: Option<ScreenWidgetId>,
        mouse: [f32; 2],
        button: i32,
        modifiers: u32,
        double_click: bool,
    ) -> bool {
        let range = if let Some(id) = id {
            let Some(index) = self.client.screen_state.0.widgets.index_of(id) else {
                return false;
            };
            index..index + 1
        } else {
            let len = self
                .client
                .screen_state
                .0
                .widgets
                .inner
                .borrow()
                .entries
                .len();
            0..len
        };
        let hit_rect = |rect| id.is_some() || point_in_rect(mouse, rect);
        let generation = self.client.screen_state.0.widgets.generation();
        for idx in range {
            if self.client.screen_state.0.widgets.generation() != generation {
                return false;
            }
            enum Hit {
                Miss,
                Button,
                TextField,
                Custom,
            }
            let (hit, disabled) = {
                let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                match inner.entries.get_mut(idx).and_then(Option::as_mut) {
                    Some(entry) if !entry.disabled && entry.owner.is_enabled() => match &entry.kind
                    {
                        MountedWidgetKind::Button { state, .. } => {
                            if state.visible && state.active && hit_rect(state.rect) {
                                (Hit::Button, false)
                            } else {
                                (Hit::Miss, false)
                            }
                        }
                        MountedWidgetKind::TextField {
                            edit_box,
                            active,
                            visible,
                            ..
                        } => {
                            let rect = [edit_box.x, edit_box.y, edit_box.width, edit_box.height];
                            if *visible && *active && hit_rect(rect) {
                                (Hit::TextField, false)
                            } else {
                                (Hit::Miss, false)
                            }
                        }
                        MountedWidgetKind::Custom(widget) => {
                            let over = call_owner(
                                &entry.owner,
                                &mut entry.disabled,
                                "screen_widget_hit_test",
                                || {
                                    let rect = widget.bounds();
                                    widget.visible()
                                        && widget.active()
                                        && valid_widget_rect(rect)
                                        && hit_rect(rect)
                                },
                            )
                            .unwrap_or(false);
                            (if over { Hit::Custom } else { Hit::Miss }, entry.disabled)
                        }
                    },
                    _ => (Hit::Miss, false),
                }
            };
            if disabled {
                self.client.screen_state.0.widgets.clear_links_for(idx);
            }
            match hit {
                Hit::Miss => continue,
                Hit::Button => {
                    if button != 0 {
                        return id.is_none();
                    }
                    let entry_id = self
                        .client
                        .screen_state
                        .0
                        .widgets
                        .inner
                        .borrow()
                        .entries
                        .get(idx)
                        .and_then(Option::as_ref)
                        .map(|e| e.id);
                    self.play_audio_button_click();
                    let accepted =
                        self.with_mounted_widget(idx, |game, entry| entry.press_button(game));
                    if accepted == Some(true)
                        && self.client.screen_state.0.widgets.generation() == generation
                        && entry_id.is_some_and(|eid| {
                            self.client.screen_state.0.widgets.index_of(eid) == Some(idx)
                        })
                    {
                        self.set_mounted_widget_focus(Some(idx));
                        self.client
                            .screen_state
                            .0
                            .widgets
                            .inner
                            .borrow_mut()
                            .dragging = Some((idx, button));
                    }
                    return true;
                }
                Hit::TextField => {
                    if button != 0 {
                        return id.is_none();
                    }
                    self.set_mounted_widget_focus(Some(idx));
                    let shift = (modifiers & 1) != 0;
                    {
                        let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                        if let Some(Some(entry)) = inner.entries.get_mut(idx)
                            && let MountedWidgetKind::TextField { edit_box, .. } = &mut entry.kind
                        {
                            edit_box.click(&self.client.gui.atlas, mouse[0], shift, double_click);
                        }
                        inner.dragging = Some((idx, button));
                    }
                    return true;
                }
                Hit::Custom => {
                    let entry_id = self
                        .client
                        .screen_state
                        .0
                        .widgets
                        .inner
                        .borrow()
                        .entries
                        .get(idx)
                        .and_then(Option::as_ref)
                        .map(|e| e.id);
                    let (handled, take_focus) = self
                        .with_mounted_widget(idx, |game, entry| {
                            if let MountedWidgetKind::Custom(widget) = &mut entry.kind {
                                call_owner(
                                    &entry.owner,
                                    &mut entry.disabled,
                                    "screen_widget_mouse_clicked",
                                    || {
                                        let handled = widget.mouse_clicked(
                                            game,
                                            mouse,
                                            button,
                                            modifiers,
                                            double_click,
                                        );
                                        let take_focus =
                                            handled && widget.should_take_focus_after_interaction();
                                        (handled, take_focus)
                                    },
                                )
                                .unwrap_or((false, false))
                            } else {
                                (false, false)
                            }
                        })
                        .unwrap_or((false, false));
                    if handled
                        && self.client.screen_state.0.widgets.generation() == generation
                        && entry_id.is_some_and(|eid| {
                            self.client.screen_state.0.widgets.index_of(eid) == Some(idx)
                        })
                    {
                        if take_focus {
                            self.set_mounted_widget_focus(Some(idx));
                        }
                        if button == 0 {
                            self.client
                                .screen_state
                                .0
                                .widgets
                                .inner
                                .borrow_mut()
                                .dragging = Some((idx, button));
                        }
                    }
                    return id.is_none() || handled;
                }
            }
        }
        false
    }

    /// Dispatch a mouse release to the mounted widget currently being dragged.
    pub fn mounted_widget_mouse_released(
        &mut self,
        mouse: [f32; 2],
        button: i32,
        modifiers: u32,
    ) -> bool {
        let Some((idx, drag_button)) = self.client.screen_state.0.widgets.inner.borrow().dragging
        else {
            return false;
        };
        if drag_button != button {
            return false;
        }
        self.client
            .screen_state
            .0
            .widgets
            .inner
            .borrow_mut()
            .dragging = None;
        if self.client.screen_state.0.widgets.focused_index() != Some(idx) {
            return false;
        }
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::TextField {
                active, visible, ..
            } => *active && *visible,
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_mouse_released",
                || widget.mouse_released(game, mouse, button, modifiers),
            )
            .unwrap_or(false),
            MountedWidgetKind::Button { .. } => true,
        })
        .unwrap_or(false)
    }

    /// Dispatch pointer dragging to the mounted widget currently being dragged.
    pub(in crate::game::client_side) fn mounted_widget_mouse_dragged(
        &mut self,
        mouse: [f32; 2],
        button: i32,
        delta: [f32; 2],
        modifiers: u32,
    ) -> bool {
        let Some((idx, drag_button)) = self.client.screen_state.0.widgets.inner.borrow().dragging
        else {
            return false;
        };
        if drag_button != button || self.client.screen_state.0.widgets.focused_index() != Some(idx)
        {
            return false;
        }
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                ..
            } => {
                if !*active || !*visible {
                    return false;
                }
                edit_box.drag_to(&game.client.gui.atlas, mouse[0]);
                true
            }
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_mouse_dragged",
                || widget.mouse_dragged(game, mouse, button, delta, modifiers),
            )
            .unwrap_or(false),
            MountedWidgetKind::Button { .. } => true,
        })
        .unwrap_or(false)
    }

    /// Dispatch a mouse wheel scroll to visible mounted custom widgets under `mouse`.
    pub(in crate::game::client_side) fn mounted_widget_mouse_scrolled(
        &mut self,
        mouse: [f32; 2],
        scroll: [f64; 2],
        modifiers: u32,
    ) -> bool {
        let len = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow()
            .entries
            .len();
        if len == 0 {
            return false;
        }
        let generation = self.client.screen_state.0.widgets.generation();
        for idx in 0..len {
            if self.client.screen_state.0.widgets.generation() != generation {
                return false;
            }
            let is_custom = matches!(
                self.client.screen_state.0
                    .widgets
                    .inner
                    .borrow()
                    .entries
                    .get(idx)
                    .and_then(Option::as_ref),
                Some(entry)
                    if !entry.disabled
                        && entry.owner.is_enabled()
                        && matches!(entry.kind, MountedWidgetKind::Custom(_))
            );
            if !is_custom {
                continue;
            }
            let handled = self
                .with_mounted_widget(idx, |game, entry| {
                    if let MountedWidgetKind::Custom(widget) = &mut entry.kind {
                        call_owner(
                            &entry.owner,
                            &mut entry.disabled,
                            "screen_widget_mouse_scrolled",
                            || {
                                let rect = widget.bounds();
                                if widget.visible()
                                    && widget.active()
                                    && valid_widget_rect(rect)
                                    && point_in_rect(mouse, rect)
                                {
                                    widget.mouse_scrolled(game, mouse, scroll, modifiers)
                                } else {
                                    false
                                }
                            },
                        )
                        .unwrap_or(false)
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if self.client.screen_state.0.widgets.generation() != generation {
                return handled;
            }
            if handled {
                return true;
            }
        }
        false
    }

    /// Dispatch a key press to the focused mounted widget.
    /// Tab/Shift+Tab and arrow traversal are handled by the unified screen
    /// focus path (`key_focus_key`), and Escape is never swallowed so vanilla
    /// `Screen.keyPressed`'s `shouldCloseOnEsc` always wins.
    pub fn mounted_widget_key_pressed(
        &mut self,
        key: KeyCode,
        repeat: bool,
        modifiers: u32,
    ) -> bool {
        if self.client.screen_state.0.widgets.is_empty() || key == KeyCode::Escape {
            return false;
        }
        let Some(idx) = self.client.screen_state.0.widgets.focused_index() else {
            return false;
        };
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::Button { state, .. } => {
                if !state.visible
                    || !state.active
                    || !matches!(key, KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space)
                {
                    return false;
                }
                game.play_audio_button_click();
                entry.press_button(game);
                true
            }
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                on_change,
                on_submit,
                ..
            } => {
                if !*visible || !*active {
                    return false;
                }
                if matches!(key, KeyCode::Enter | KeyCode::NumpadEnter) {
                    if let Some(callback) = on_submit.as_mut() {
                        let value = edit_box.value.as_str();
                        call_owner(
                            &entry.owner,
                            &mut entry.disabled,
                            "text_field_submit",
                            || callback(value, game),
                        );
                        return true;
                    }
                    return false;
                }
                let shift = (modifiers & 1) != 0;
                let alt = (modifiers & 4) != 0;
                let ctrl_or_cmd = if cfg!(target_os = "macos") {
                    (modifiers & 8) != 0
                } else {
                    (modifiers & 2) != 0
                };
                if ctrl_or_cmd && !shift && !alt {
                    let shortcut = match key {
                        KeyCode::KeyA => Some(EditShortcut::SelectAll),
                        KeyCode::KeyC => Some(EditShortcut::Copy),
                        KeyCode::KeyV => Some(EditShortcut::Paste),
                        KeyCode::KeyX => Some(EditShortcut::Cut),
                        _ => None,
                    };
                    if let Some(shortcut) = shortcut {
                        let changed = edit_box.shortcut(shortcut, &mut game.client.clipboard.0);
                        if changed && let Some(callback) = on_change.as_mut() {
                            let value = edit_box.value.as_str();
                            call_owner(
                                &entry.owner,
                                &mut entry.disabled,
                                "text_field_change",
                                || callback(value, game),
                            );
                        }
                        return true;
                    }
                }
                if key == KeyCode::KeyA {
                    return false;
                }
                let track_change = matches!(key, KeyCode::Backspace | KeyCode::Delete);
                let before = track_change.then(|| edit_box.value.clone());
                let handled = edit_box.key(key, ctrl_or_cmd, shift);
                if handled
                    && let Some(before) = before
                    && edit_box.value != before
                    && let Some(callback) = on_change.as_mut()
                {
                    let value = edit_box.value.as_str();
                    call_owner(
                        &entry.owner,
                        &mut entry.disabled,
                        "text_field_change",
                        || callback(value, game),
                    );
                }
                handled
            }
            MountedWidgetKind::Custom(widget) => {
                let Some(name) = keybinds::key_name(key) else {
                    return false;
                };
                call_owner(
                    &entry.owner,
                    &mut entry.disabled,
                    "screen_widget_key_pressed",
                    || {
                        if widget.visible() && widget.active() {
                            widget.key_pressed(game, name, repeat, modifiers)
                        } else {
                            false
                        }
                    },
                )
                .unwrap_or(false)
            }
        })
        .unwrap_or(false)
    }

    /// Dispatch a key release to the focused mounted custom widget.
    pub fn mounted_widget_key_released(&mut self, key: KeyCode, modifiers: u32) -> bool {
        let Some(idx) = self.client.screen_state.0.widgets.focused_index() else {
            return false;
        };
        let Some(name) = keybinds::key_name(key) else {
            return false;
        };
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_key_released",
                || {
                    if widget.visible() && widget.active() {
                        widget.key_released(game, name, modifiers)
                    } else {
                        false
                    }
                },
            )
            .unwrap_or(false),
            _ => false,
        })
        .unwrap_or(false)
    }

    /// Dispatch a typed or IME-committed character to the focused mounted widget.
    pub fn mounted_widget_char_typed(&mut self, c: char, modifiers: u32) -> bool {
        let Some(idx) = self.client.screen_state.0.widgets.focused_index() else {
            return false;
        };
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                on_change,
                ..
            } => {
                if !*visible || !*active || !edit_box.editable || !is_allowed_chat_character(c) {
                    return false;
                }
                let mut buf = [0u8; 4];
                let text = c.encode_utf8(&mut buf);
                let before = edit_box.value.clone();
                edit_box.insert_text(text);
                if edit_box.value != before
                    && let Some(callback) = on_change.as_mut()
                {
                    let value = edit_box.value.as_str();
                    call_owner(
                        &entry.owner,
                        &mut entry.disabled,
                        "text_field_change",
                        || callback(value, game),
                    );
                }
                true
            }
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_char_typed",
                || {
                    if widget.visible() && widget.active() {
                        widget.char_typed(game, c, modifiers)
                    } else {
                        false
                    }
                },
            )
            .unwrap_or(false),
            MountedWidgetKind::Button { .. } => false,
        })
        .unwrap_or(false)
    }

    /// Whether the focused mounted widget handles IME preedit
    /// (`ContainerEventHandler.preeditUpdated` -> `EditBox.preeditUpdated`).
    /// Only an editable, visible, active built-in `TextField` handles preedit;
    /// custom `ScreenWidget`s have no preedit callback and return `false` even
    /// if they return `true` from `captures_input()`.
    pub(in crate::game::client_side) fn mounted_widget_preedit(
        &self,
        _event: Option<&PreeditEvent>,
    ) -> bool {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        let Some(idx) = inner.focused else {
            return false;
        };
        let Some(Some(entry)) = inner.entries.get(idx) else {
            return false;
        };
        if entry.disabled || !entry.owner.is_enabled() {
            return false;
        }
        matches!(
            &entry.kind,
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                ..
            } if *active && *visible && edit_box.editable
        )
    }

    /// Read and optionally apply a `TextEdit` rewrite from
    /// `screen_char_typed_pre` for a focused mounted built-in `TextField`.
    /// Custom `ScreenWidget`s own their own state and return `None`.
    pub fn mounted_focused_text_edit(&mut self, edited: Option<&TextEdit>) -> Option<TextEdit> {
        let idx = self.client.screen_state.0.widgets.focused_index()?;
        self.with_mounted_widget(idx, |game, entry| match &mut entry.kind {
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                on_change,
                ..
            } if *active && *visible && edit_box.editable => {
                if let Some(edited) = edited {
                    let before = edit_box.value.clone();
                    if edit_box.value != edited.value {
                        edit_box.select_all();
                        edit_box.insert_text(&edited.value);
                    }
                    edit_box.set_cursor(edited.highlight, false);
                    edit_box.set_cursor(edited.cursor, true);
                    if edit_box.value != before
                        && let Some(callback) = on_change.as_mut()
                    {
                        let value = edit_box.value.as_str();
                        call_owner(
                            &entry.owner,
                            &mut entry.disabled,
                            "text_field_change",
                            || callback(value, game),
                        );
                    }
                }
                Some(edit_box.edit.clone())
            }
            _ => None,
        })
        .flatten()
    }

    /// Whether the focused mounted widget captures text input (`Screen.isInputCaptured` /
    /// `EditBox.canConsumeInput`).
    pub fn mounted_widget_captures_input(&self) -> bool {
        let inner = self.client.screen_state.0.widgets.inner.borrow();
        let Some(idx) = inner.focused else {
            return false;
        };
        let Some(Some(entry)) = inner.entries.get(idx) else {
            return false;
        };
        if entry.disabled || !entry.owner.is_enabled() {
            return false;
        }
        match &entry.kind {
            MountedWidgetKind::TextField {
                edit_box,
                active,
                visible,
                ..
            } => *active && *visible && edit_box.editable,
            MountedWidgetKind::Custom(widget) => entry
                .owner
                .call("screen widget", "screen_widget_captures_input", || {
                    widget.visible() && widget.active() && widget.captures_input()
                })
                .unwrap_or(false),
            MountedWidgetKind::Button { .. } => false,
        }
    }
}

#[cfg(feature = "client")]
impl crate::game::ClientGame<'_> {
    fn stage_live_mounted(&mut self, kind: MountedWidgetKind) -> ScreenWidgetId {
        let id = self.alloc_widget_id();
        let mut entry = MountedWidgetEntry {
            id,
            owner: widget_callback_owner(),
            disabled: false,
            kind,
        };
        let rect = match &entry.kind {
            MountedWidgetKind::Button { state, .. } => Some(state.rect),
            MountedWidgetKind::TextField { edit_box, .. } => {
                Some([edit_box.x, edit_box.y, edit_box.width, edit_box.height])
            }
            MountedWidgetKind::Custom(widget) => call_owner(
                &entry.owner,
                &mut entry.disabled,
                "screen_widget_bounds",
                || widget.bounds(),
            ),
        };
        if let Some(rect) = rect
            && valid_widget_rect(rect)
        {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            inner.entries.push(Some(entry));
            inner.order.push(id);
            drop(inner);
            self.sync_children_cache();
        } else {
            entry.dispose();
        }
        id
    }
    fn apply_live_widget_op(&mut self, id: ScreenWidgetId, op: WidgetOp) -> bool {
        let ok = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            if !inner.order.contains(&id) {
                return false;
            }
            if let Some(key) = inner
                .engine_children
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.key.clone())
            {
                let affects_hits = op.affects_hits();
                let ov = inner.engine_overrides.overrides.entry(key).or_default();
                apply_widget_op_to_engine(ov, op);
                if affects_hits {
                    inner.engine_hits =
                        build_engine_hits(&inner.engine_children, &inner.engine_overrides);
                }
                true
            } else if let Some(entry) = inner.entries.iter_mut().flatten().find(|e| e.id == id) {
                apply_widget_op_to_mounted(entry, op)
            } else {
                false
            }
        };
        if ok {
            self.sync_children_cache();
        }
        ok
    }
}

#[cfg(feature = "client")]
impl ScreenWidgets for crate::game::ClientGame<'_> {
    fn children(&self) -> &[ScreenChild] {
        &self.client.screen_state.0.widgets.children_cache
    }

    fn add_button(&mut self, button: Button) -> ScreenWidgetId {
        self.stage_live_mounted(button_to_kind(button))
    }

    fn add_text_field(&mut self, field: TextField) -> ScreenWidgetId {
        self.stage_live_mounted(text_field_to_kind(field))
    }

    fn add_widget(&mut self, widget: Box<dyn ScreenWidget>) -> ScreenWidgetId {
        self.stage_live_mounted(MountedWidgetKind::Custom(widget))
    }

    fn set_initial_focus(&mut self, id: ScreenWidgetId) -> bool {
        let Some(idx) = self.client.screen_state.0.widgets.index_of(id) else {
            return false;
        };
        let can = self
            .client
            .screen_state
            .0
            .widgets
            .inner
            .borrow_mut()
            .entries
            .get_mut(idx)
            .and_then(Option::as_mut)
            .is_some_and(MountedWidgetEntry::can_focus);
        if !can {
            return false;
        }
        self.set_mounted_widget_focus(Some(idx));
        true
    }

    fn remove(&mut self, id: ScreenWidgetId) -> bool {
        let (removed_mounted, was_focused) = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            let Some(order_pos) = inner.order.iter().position(|&cid| cid == id) else {
                return false;
            };
            inner.order.remove(order_pos);
            if let Some(slot_key) = inner
                .engine_children
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.key.clone())
            {
                inner
                    .engine_overrides
                    .overrides
                    .entry(slot_key)
                    .or_default()
                    .removed = true;
                inner.engine_hits =
                    build_engine_hits(&inner.engine_children, &inner.engine_overrides);
                (None, false)
            } else if let Some(idx) = inner
                .entries
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|e| e.id == id))
            {
                let was_focused = inner.focused == Some(idx);
                if was_focused {
                    inner.focused = None;
                }
                if inner.dragging.is_some_and(|(drag_idx, _)| drag_idx == idx) {
                    inner.dragging = None;
                }
                (inner.entries[idx].take(), was_focused)
            } else {
                (None, false)
            }
        };
        if let Some(mut entry) = removed_mounted {
            if was_focused {
                entry.apply_focus(false);
                sunrise_client::preedit::reset();
                self.client.ime_area = None;
            }
            entry.dispose();
        }
        self.sync_children_cache();
        true
    }

    fn clear(&mut self) {
        let (entries, focused) = {
            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
            let MountedWidgetsInner {
                order,
                engine_children,
                engine_overrides,
                engine_hits,
                dragging,
                pending_tooltip,
                entries,
                focused,
                ..
            } = &mut *inner;
            order.clear();
            for slot in engine_children.iter() {
                engine_overrides
                    .overrides
                    .entry(slot.key.clone())
                    .or_default()
                    .removed = true;
            }
            *engine_hits = build_engine_hits(engine_children, engine_overrides);
            *dragging = None;
            *pending_tooltip = None;
            (std::mem::take(entries), focused.take())
        };
        if focused.is_some() {
            sunrise_client::preedit::reset();
            self.client.ime_area = None;
        }
        for (idx, mut entry) in entries
            .into_iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e?)))
        {
            if focused == Some(idx) {
                entry.apply_focus(false);
            }
            entry.dispose();
        }
        self.sync_children_cache();
    }

    fn set_bounds(&mut self, id: ScreenWidgetId, rect: [f32; 4]) -> bool {
        if !valid_widget_rect(rect) || rect[2] < 0.0 || rect[3] < 0.0 {
            return false;
        }
        self.apply_live_widget_op(id, WidgetOp::Bounds(rect))
    }

    fn set_message(&mut self, id: ScreenWidgetId, message: ApiText) -> bool {
        let Ok(engine_text): std::result::Result<Text, _> = message.try_into_engine(&()) else {
            return false;
        };
        self.apply_live_widget_op(id, WidgetOp::Message(message, engine_text))
    }

    fn set_active(&mut self, id: ScreenWidgetId, active: bool) -> bool {
        self.apply_live_widget_op(id, WidgetOp::Active(active))
    }

    fn set_visible(&mut self, id: ScreenWidgetId, visible: bool) -> bool {
        self.apply_live_widget_op(id, WidgetOp::Visible(visible))
    }

    fn set_alpha(&mut self, id: ScreenWidgetId, alpha: f32) -> bool {
        if !alpha.is_finite() {
            return false;
        }
        self.apply_live_widget_op(id, WidgetOp::Alpha(alpha))
    }

    fn set_label_text_color(&mut self, id: ScreenWidgetId, color: Option<u32>) -> bool {
        self.apply_live_widget_op(id, WidgetOp::LabelTextColor(color))
    }

    fn set_tooltip(&mut self, id: ScreenWidgetId, tooltip: Option<WidgetTooltip>) -> bool {
        let Some(engine_tip) = convert_widget_tooltip(tooltip.clone()) else {
            return false;
        };
        self.apply_live_widget_op(id, WidgetOp::Tooltip(tooltip, engine_tip))
    }

    fn set_tooltip_delay(&mut self, id: ScreenWidgetId, delay: Duration) -> bool {
        self.apply_live_widget_op(id, WidgetOp::TooltipDelay(delay))
    }

    fn set_tab_order_group(&mut self, id: ScreenWidgetId, group: i32) -> bool {
        self.apply_live_widget_op(id, WidgetOp::TabOrderGroup(group))
    }

    fn set_sprites(&mut self, id: ScreenWidgetId, sprites: Option<ButtonSprites>) -> bool {
        self.apply_live_widget_op(id, WidgetOp::Sprites(sprites))
    }

    fn set_override_render_highlighted_sprite(
        &mut self,
        id: ScreenWidgetId,
        highlighted: Option<bool>,
    ) -> bool {
        self.apply_live_widget_op(id, WidgetOp::OverrideHighlighted(highlighted))
    }

    fn press(&mut self, id: ScreenWidgetId) -> bool {
        let generation = self.client.screen_state.0.widgets.generation();
        enum TargetKind {
            MountedButton(usize),
            Engine(widgets::EngineWidgetKey, [f32; 4]),
        }
        let target = {
            let inner = self.client.screen_state.0.widgets.inner.borrow();
            if !inner.order.contains(&id) {
                return false;
            }
            if let Some(idx) = inner
                .entries
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|e| e.id == id))
            {
                let Some(Some(entry)) = inner.entries.get(idx) else {
                    return false;
                };
                if entry.disabled || !entry.owner.is_enabled() {
                    return false;
                }
                match &entry.kind {
                    MountedWidgetKind::Button { state, .. } if state.visible && state.active => {
                        Some(TargetKind::MountedButton(idx))
                    }
                    _ => None,
                }
            } else if let Some(slot) = inner.engine_children.iter().find(|s| s.id == id) {
                if inner.engine_overrides.suppress_all {
                    return false;
                }
                let ov = inner.engine_overrides.overrides.get(&slot.key);
                let removed = ov.is_some_and(|o| o.removed);
                let visible = ov.and_then(|o| o.visible).unwrap_or(true);
                let active = ov.and_then(|o| o.active).unwrap_or(slot.default_active);
                if removed || !visible || !active {
                    return false;
                }
                let direct_override_screen = self.title_input_active()
                    || (self.observed_overlay_now().is_none()
                        && ((self.client.screen == super::Screen::Paused
                            && self.client.pause_menu)
                            || self.client.screen == super::Screen::Options));
                let target_rect = if direct_override_screen {
                    ov.and_then(|o| o.rect).unwrap_or(slot.default_rect)
                } else {
                    inner
                        .engine_hits
                        .iter()
                        .find(|h| h.key == slot.key)
                        .map(|h| h.original_rect)
                        .unwrap_or(slot.default_rect)
                };
                Some(TargetKind::Engine(slot.key.clone(), target_rect))
            } else {
                None
            }
        };
        match target {
            None => false,
            Some(TargetKind::MountedButton(idx)) => {
                self.play_audio_button_click();
                let accepted =
                    self.with_mounted_widget(idx, |game, entry| entry.press_button(game));
                if self.client.screen_state.0.widgets.generation() == generation {
                    self.sync_children_cache();
                }
                accepted == Some(true)
            }
            Some(TargetKind::Engine(key, [ox, oy, ow, oh])) => {
                if ow <= 0.0 || oh <= 0.0 {
                    return false;
                }
                let prev_cursor = self.client.cursor;
                self.client.cursor = (ox + ow * 0.5, oy + oh * 0.5);
                let handled = widgets::with_selected_engine_widget(&key, || {
                    self.with_redirecting_engine_click(|game| {
                        game.with_screen_input_overrides(|game| {
                            game.route_screen_button(MouseButton::Left, 0, true, false)
                        })
                    })
                })
                .unwrap_or(false);
                self.settle_title_mode();
                self.client.cursor = prev_cursor;
                if self.client.screen_state.0.widgets.generation() == generation {
                    if handled {
                        let discovered =
                            self.client.screen_state.0.widgets.inner.borrow().discovered;
                        if discovered && !widgets::in_override_pass() {
                            let collected = self.discover_screen_widgets();
                            self.reconcile_engine_children(collected);
                            let mut inner = self.client.screen_state.0.widgets.inner.borrow_mut();
                            inner.engine_hits =
                                build_engine_hits(&inner.engine_children, &inner.engine_overrides);
                        } else if discovered {
                            self.client
                                .screen_state
                                .0
                                .widgets
                                .inner
                                .borrow_mut()
                                .needs_engine_resync = true;
                        }
                    }
                    self.sync_children_cache();
                }
                handled
            }
        }
    }
}
