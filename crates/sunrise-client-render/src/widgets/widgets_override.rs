//! Scoped widget collection and live per-screen override resolution for the mod API's
//! `ScreenWidgets` surface (`ScreenEvent.Init.Pre`, `ScreenEvent.Init.Post`, and
//! `ClientHost::screen_widgets`).
//!
//! Two fast thread-local booleans (`COLLECTING` and `HAS_OVERRIDES`) keep the unmodded draw hot
//! path to two `Cell::get` checks with zero allocations or map lookups.
use super::{CURSOR, Cursor, NARRATING, Narratable, What};
use crate::text::Text;
use mod_api::{ButtonSprites, Identifier, TooltipComponent};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Engine-side validated form of [`mod_api::WidgetTooltip`], stored in override tables and
/// mounted widgets so draw passes never convert API text trees per frame.
#[derive(Clone, Debug)]
pub struct EngineWidgetTooltip {
    pub message: Text,
    pub narration: String,
    pub component: Option<TooltipComponent>,
    pub style: Option<Identifier>,
}

/// Coarse widget category used to disambiguate keys across distinct widget types on one screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetBucket {
    Button,
    Slider,
    Field,
    Checkbox,
}

/// Stable screen-local identity of an engine-drawn widget across discovery, live draws,
/// message/bounds overrides, and `repositionElements` resizes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EngineWidgetKey {
    /// Original (pre-override) logical key: translation key when available, else value-invariant
    /// label prefix, else trimmed plain text (empty for unlabelled widgets).
    pub key: String,
    pub bucket: WidgetBucket,
    /// 0-based occurrence index of `(key, bucket)` within a single screen pass in draw order.
    pub occurrence: u32,
}

/// One engine widget discovered during a collector pass or observed during a live override pass.
#[derive(Clone, Debug)]
pub struct CollectedWidget {
    pub key: EngineWidgetKey,
    pub rect: [f32; 4],
    pub message: Option<Text>,
    pub active: bool,
    pub tab_order_group: i32,
}

/// Mod-applied overrides for one engine widget on the active screen instance.
#[derive(Clone, Debug, Default)]
pub struct EngineWidgetOverride {
    pub removed: bool,
    pub rect: Option<[f32; 4]>,
    pub message: Option<Text>,
    pub active: Option<bool>,
    pub visible: Option<bool>,
    pub alpha: Option<f32>,
    pub label_text_color: Option<Option<u32>>,
    pub tooltip: Option<Option<EngineWidgetTooltip>>,
    pub tooltip_delay: Option<Duration>,
    pub tab_order_group: Option<i32>,
    pub sprites: Option<Option<ButtonSprites>>,
    pub override_render_highlighted_sprite: Option<Option<bool>>,
}

impl EngineWidgetOverride {
    /// Whether this override changes nothing from the engine widget's defaults.
    pub fn is_identity(&self) -> bool {
        !self.removed
            && self.rect.is_none()
            && self.message.is_none()
            && self.active.is_none()
            && self.visible.is_none()
            && self.alpha.is_none()
            && self.label_text_color.is_none()
            && self.tooltip.is_none()
            && self.tooltip_delay.is_none()
            && self.tab_order_group.is_none()
            && self.sprites.is_none()
            && self.override_render_highlighted_sprite.is_none()
    }
}

/// Persistent per-screen override state moved into a draw pass and restored on exit.
#[derive(Clone, Debug, Default)]
pub struct EngineOverrideMaps {
    /// Set when `screen_init_pre` was cancelled: suppresses every engine widget while leaving
    /// background, slots, pages, and mounted mod widgets intact.
    pub suppress_all: bool,
    pub overrides: HashMap<EngineWidgetKey, EngineWidgetOverride>,
    /// When each hovered widget key started hovering, for `set_tooltip_delay`.
    pub hover_since: HashMap<EngineWidgetKey, Instant>,
}

/// Hit-test redirect entry recorded for one engine widget during a live screen draw.
#[derive(Clone, Debug)]
pub struct EngineHitEntry {
    pub key: EngineWidgetKey,
    pub original_rect: [f32; 4],
    pub resolved_rect: [f32; 4],
    /// Whether the widget is visible, active, and not removed/suppressed.
    pub interactive: bool,
    /// Whether a click landing on `original_rect` must be blocked from reaching the engine screen
    /// (unless another interactive widget's `resolved_rect` covers that point).
    pub blocks_original: bool,
}

/// Deferred tooltip scheduled by an overridden engine widget or mounted widget.
#[derive(Clone, Debug)]
pub struct PendingWidgetTooltip {
    pub tooltip: EngineWidgetTooltip,
    pub rect: [f32; 4],
    pub focused_only: bool,
}

/// Summary captured while drawing a screen under [`with_widget_overrides`].
#[derive(Clone, Debug, Default)]
pub struct EngineWidgetPassOutput {
    pub hits: Vec<EngineHitEntry>,
    pub hovered: HashMap<EngineWidgetKey, bool>,
    pub pending_tooltip: Option<PendingWidgetTooltip>,
    pub seen: Vec<CollectedWidget>,
    pub render_vetoed: bool,
}

/// Inputs a shared widget drawer passes to [`resolve_engine_widget`].
pub struct EngineWidgetInput<'a> {
    pub bucket: WidgetBucket,
    pub rect: [f32; 4],
    pub label: Option<&'a Text>,
    pub explicit_key: Option<&'a str>,
    pub active: bool,
    pub hovered: bool,
    pub focused: bool,
    pub tab_order_group: i32,
}

/// Resolved properties returned when an engine widget has active mod overrides.
#[derive(Clone, Debug)]
pub struct ResolvedWidgetProps {
    pub rect: [f32; 4],
    pub message: Option<Text>,
    pub active: bool,
    pub hovered: bool,
    pub alpha: f32,
    pub label_text_color: Option<u32>,
    pub tab_order_group: i32,
    pub sprites: Option<ButtonSprites>,
    pub override_render_highlighted_sprite: Option<bool>,
    pub tooltip_narration: Option<String>,
}

/// Outcome of [`resolve_engine_widget`].
#[derive(Debug)]
pub enum ResolvedEngineWidget {
    /// Neither collecting nor overriding this widget: caller draws with its normal arguments.
    Unchanged,
    /// Collecting (widget recorded) or suppressed/removed/hidden: caller skips drawing and side effects.
    Skip,
    /// Draw this engine widget with the resolved overrides.
    Draw(ResolvedWidgetProps),
}

struct CollectorState {
    widgets: Vec<CollectedWidget>,
    counts: HashMap<(String, WidgetBucket), u32>,
}

struct ActiveOverridePass {
    maps: EngineOverrideMaps,
    cursor: (f32, f32),
    now: Instant,
    counts: HashMap<(String, WidgetBucket), u32>,
    next_hover_since: HashMap<EngineWidgetKey, Instant>,
    output: EngineWidgetPassOutput,
}

thread_local! {
    static COLLECTING: Cell<bool> = const { Cell::new(false) };
    static HAS_OVERRIDES: Cell<bool> = const { Cell::new(false) };
    static HAS_INPUT_OVERRIDES: Cell<bool> = const { Cell::new(false) };
    static BLOCKING_ENGINE_WIDGET_CLICK: Cell<bool> = const { Cell::new(false) };
    static BYPASS_OVERRIDES: Cell<bool> = const { Cell::new(false) };
    static COLLECTOR: RefCell<Option<CollectorState>> = const { RefCell::new(None) };
    static ACTIVE_OVERRIDES: RefCell<Option<ActiveOverridePass>> = const { RefCell::new(None) };
    static INPUT_OVERRIDES: RefCell<Option<EngineOverrideMaps>> = const { RefCell::new(None) };
    static LAST_TARGET_META: RefCell<Option<(EngineWidgetKey, i32)>> = const { RefCell::new(None) };
}

/// Take the collector identity and resolved `tab_order_group` of the engine widget most recently
/// resolved by [`resolve_engine_widget`] for the `widgets::target_as` call immediately following it.
pub(super) fn take_last_resolved_target_meta() -> (Option<EngineWidgetKey>, i32) {
    match LAST_TARGET_META.take() {
        Some((key, group)) => (Some(key), group),
        None => (None, 0),
    }
}

/// Whether a widget discovery pass is currently running on this thread.
pub fn is_collecting_widgets() -> bool {
    COLLECTING.get()
}

/// Whether the current container-screen press landed on a suppressed/removed/disabled/moved-away
/// engine widget, so widget-level click handlers must ignore the press while container slots
/// continue normal dispatch.
pub fn is_blocking_engine_widget_click() -> bool {
    BLOCKING_ENGINE_WIDGET_CLICK.get()
}

/// Run `f` with `BLOCKING_ENGINE_WIDGET_CLICK` set, unconditionally restoring the prior flag.
pub fn with_blocking_engine_widget_click<R>(f: impl FnOnce() -> R) -> R {
    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            BLOCKING_ENGINE_WIDGET_CLICK.set(self.0);
        }
    }
    let _guard = Guard(BLOCKING_ENGINE_WIDGET_CLICK.replace(true));
    f()
}

/// Run `f` with `maps` exposed for input override resolution (`resolve_engine_input`),
/// unconditionally restoring `maps` on normal return or unwind.
pub fn with_input_overrides<R>(maps: &mut EngineOverrideMaps, f: impl FnOnce() -> R) -> R {
    struct Guard<'a> {
        dest: &'a mut EngineOverrideMaps,
        prev_has: bool,
        prev_maps: Option<EngineOverrideMaps>,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            HAS_INPUT_OVERRIDES.set(self.prev_has);
            if let Some(current) = INPUT_OVERRIDES.replace(self.prev_maps.take()) {
                *self.dest = current;
            }
        }
    }
    let taken = std::mem::take(maps);
    let _guard = Guard {
        dest: maps,
        prev_has: HAS_INPUT_OVERRIDES.replace(true),
        prev_maps: INPUT_OVERRIDES.replace(Some(taken)),
    };
    f()
}

/// Resolve an engine widget's interactive rectangle for input dispatch (mouse click, keyboard
/// Enter/Space activation, controller focus) against the active `EngineOverrideMaps`.
/// Returns `None` when the widget is suppressed, removed, hidden, or inactive (`visible && active`
/// per vanilla `AbstractButton.onClick` / `keyPressed`).
pub fn resolve_engine_input(
    bucket: WidgetBucket,
    label: Option<&Text>,
    explicit_key: Option<&str>,
    occurrence: u32,
    default_rect: [f32; 4],
    default_active: bool,
) -> Option<[f32; 4]> {
    if !HAS_INPUT_OVERRIDES.get() {
        return default_active.then_some(default_rect);
    }
    let key_str = match explicit_key {
        Some(explicit) => explicit.to_owned(),
        None => label.map(stable_key_from_text).unwrap_or_default(),
    };
    let key = EngineWidgetKey {
        key: key_str,
        bucket,
        occurrence,
    };
    INPUT_OVERRIDES.with_borrow(|slot| {
        let Some(maps) = slot.as_ref() else {
            return default_active.then_some(default_rect);
        };
        if maps.suppress_all {
            return None;
        }
        let ov = maps.overrides.get(&key);
        if ov.is_some_and(|o| o.removed || o.visible == Some(false)) {
            return None;
        }
        let active = ov.and_then(|o| o.active).unwrap_or(default_active);
        if !active {
            return None;
        }
        Some(ov.and_then(|o| o.rect).unwrap_or(default_rect))
    })
}

/// Mark the active [`with_widget_overrides`] pass as vetoed by `ScreenEvent.Render.Pre` so this
/// frame's empty draw never clears live widget membership or hit tables.
pub fn mark_render_vetoed() {
    if !HAS_OVERRIDES.get() {
        return;
    }
    ACTIVE_OVERRIDES.with_borrow_mut(|slot| {
        if let Some(pass) = slot.as_mut() {
            pass.output.render_vetoed = true;
        }
    });
}

/// Take the deferred tooltip scheduled by an overridden engine widget during the active
/// [`with_widget_overrides`] pass, if any.
pub fn take_active_pending_tooltip() -> Option<PendingWidgetTooltip> {
    if !HAS_OVERRIDES.get() {
        return None;
    }
    ACTIVE_OVERRIDES.with_borrow_mut(|slot| slot.as_mut()?.output.pending_tooltip.take())
}

/// Stable key extracted from an engine `Text`: translation key when available, else prefix before
/// `": "` (so value-bearing sliders and cycle labels stay stable across value changes), else
/// trimmed plain text.
pub fn stable_key_from_text(text: &Text) -> String {
    if let Some(key) = text.translation_key_hint() {
        return key;
    }
    let plain = text.plain();
    let trimmed = plain.trim();
    if let Some((prefix, _)) = trimmed.split_once(": ") {
        prefix.trim().to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn point_in_rect((mx, my): (f32, f32), [x, y, w, h]: [f32; 4]) -> bool {
    w > 0.0 && h > 0.0 && mx >= x && my >= y && mx < x + w && my < y + h
}

/// Clear any widgets collected or hits recorded before a covering popup (`widgets::cover_frame`).
pub(super) fn cover_override_frame() {
    if COLLECTING.get() {
        COLLECTOR.with_borrow_mut(|slot| {
            if let Some(state) = slot.as_mut() {
                state.widgets.clear();
                state.counts.clear();
            }
        });
    }
    if HAS_OVERRIDES.get() {
        ACTIVE_OVERRIDES.with_borrow_mut(|slot| {
            if let Some(pass) = slot.as_mut() {
                pass.output.hits.clear();
                pass.output.seen.clear();
                pass.output.pending_tooltip = None;
                pass.counts.clear();
            }
        });
    }
}

/// Run `f` with engine widget collection enabled in place of the live widget/focus/narration
/// tables. Live frame tables, cursor, narration flag, and any enclosing override pass are saved
/// and restored unconditionally (including on panic).
pub fn with_widget_collector<R>(f: impl FnOnce() -> R) -> (R, Vec<CollectedWidget>) {
    struct Guard<'a> {
        saved: Option<super::Saved>,
        prev_cursor: Cursor,
        prev_narrating: bool,
        prev_collecting: bool,
        prev_has_overrides: bool,
        prev_bypass: bool,
        prev_collector: Option<CollectorState>,
        out: &'a mut Vec<CollectedWidget>,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            if let Some(finished) = COLLECTOR.replace(self.prev_collector.take()) {
                *self.out = finished.widgets;
            }
            COLLECTING.set(self.prev_collecting);
            HAS_OVERRIDES.set(self.prev_has_overrides);
            BYPASS_OVERRIDES.set(self.prev_bypass);
            NARRATING.set(self.prev_narrating);
            CURSOR.set(self.prev_cursor);
            if let Some(saved) = self.saved.take() {
                super::put(saved);
            }
        }
    }
    let mut collected = Vec::new();
    let result = {
        let _guard = Guard {
            saved: Some(super::take()),
            prev_cursor: CURSOR.replace(Cursor::Arrow),
            prev_narrating: NARRATING.replace(false),
            prev_collecting: COLLECTING.replace(true),
            prev_has_overrides: HAS_OVERRIDES.replace(false),
            prev_bypass: BYPASS_OVERRIDES.replace(false),
            prev_collector: COLLECTOR.replace(Some(CollectorState {
                widgets: Vec::new(),
                counts: HashMap::new(),
            })),
            out: &mut collected,
        };
        f()
    };
    (result, collected)
}

/// Run `f` with `maps` active for engine widget override lookup and hit-map capture. `maps` is
/// moved into thread-local storage for the duration of `f` and unconditionally restored on exit or
/// unwind (`hover_since` timestamps are committed on normal completion and preserved on unwind).
pub fn with_widget_overrides<R>(
    maps: &mut EngineOverrideMaps,
    cursor: (f32, f32),
    now: Instant,
    f: impl FnOnce() -> R,
) -> (R, EngineWidgetPassOutput) {
    struct Guard<'a> {
        dest: &'a mut EngineOverrideMaps,
        prev_has_overrides: bool,
        prev_pass: Option<ActiveOverridePass>,
        finished: Option<ActiveOverridePass>,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let normal_exit = self.finished.is_some();
            let active = self
                .finished
                .take()
                .or_else(|| ACTIVE_OVERRIDES.replace(self.prev_pass.take()));
            HAS_OVERRIDES.set(self.prev_has_overrides);
            LAST_TARGET_META.set(None);
            if let Some(mut pass) = active {
                if normal_exit && !pass.output.render_vetoed {
                    pass.maps.hover_since = pass.next_hover_since;
                }
                *self.dest = pass.maps;
            }
        }
    }
    let taken = std::mem::take(maps);
    let prev_pass = ACTIVE_OVERRIDES.replace(Some(ActiveOverridePass {
        maps: taken,
        cursor,
        now,
        counts: HashMap::new(),
        next_hover_since: HashMap::new(),
        output: EngineWidgetPassOutput::default(),
    }));
    let prev_has_overrides = HAS_OVERRIDES.replace(true);
    let mut guard = Guard {
        dest: maps,
        prev_has_overrides,
        prev_pass,
        finished: None,
    };
    let result = f();
    let mut finished = ACTIVE_OVERRIDES
        .replace(guard.prev_pass.take())
        .expect("override pass active");
    let output = std::mem::take(&mut finished.output);
    guard.finished = Some(finished);
    drop(guard);
    (result, output)
}

/// Run `f` while bypassing engine widget collection and overrides (used when drawing mounted mod
/// widgets so they never collide with engine widget keys or `screen_init_pre` suppression).
pub fn without_engine_overrides<R>(f: impl FnOnce() -> R) -> R {
    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            BYPASS_OVERRIDES.set(self.0);
        }
    }
    let _guard = Guard(BYPASS_OVERRIDES.replace(true));
    f()
}

/// Single resolution entry point shared by every engine widget drawer (`ui::button_core`,
/// `ui::slider_ext`, `ui::EditBox`, `graphics_style`, `screen_draw`, `pack_screen`, and plain-text
/// engine controls).
pub fn resolve_engine_widget(input: EngineWidgetInput<'_>) -> ResolvedEngineWidget {
    if BYPASS_OVERRIDES.get() {
        return ResolvedEngineWidget::Unchanged;
    }
    let collecting = COLLECTING.get();
    let overriding = HAS_OVERRIDES.get();
    if !collecting && !overriding {
        return ResolvedEngineWidget::Unchanged;
    }
    let key_str = match input.explicit_key {
        Some(explicit) => explicit.to_owned(),
        None => input.label.map(stable_key_from_text).unwrap_or_default(),
    };
    if collecting {
        COLLECTOR.with_borrow_mut(|slot| {
            let Some(state) = slot.as_mut() else {
                return;
            };
            let occurrence = {
                let count = state
                    .counts
                    .entry((key_str.clone(), input.bucket))
                    .or_insert(0);
                let current = *count;
                *count = current.saturating_add(1);
                current
            };
            let message = input
                .label
                .filter(|t| !t.is_empty() || t.translation_key_hint().is_some())
                .cloned();
            state.widgets.push(CollectedWidget {
                key: EngineWidgetKey {
                    key: key_str,
                    bucket: input.bucket,
                    occurrence,
                },
                rect: input.rect,
                message,
                active: input.active,
                tab_order_group: input.tab_order_group,
            });
        });
        return ResolvedEngineWidget::Skip;
    }

    ACTIVE_OVERRIDES.with_borrow_mut(|slot| {
        let Some(pass) = slot.as_mut() else {
            return ResolvedEngineWidget::Unchanged;
        };
        let occurrence = {
            let count = pass
                .counts
                .entry((key_str.clone(), input.bucket))
                .or_insert(0);
            let current = *count;
            *count = current.saturating_add(1);
            current
        };
        let key = EngineWidgetKey {
            key: key_str,
            bucket: input.bucket,
            occurrence,
        };
        let message = input
            .label
            .filter(|t| !t.is_empty() || t.translation_key_hint().is_some())
            .cloned();
        pass.output.seen.push(CollectedWidget {
            key: key.clone(),
            rect: input.rect,
            message,
            active: input.active,
            tab_order_group: input.tab_order_group,
        });
        if pass.maps.suppress_all {
            pass.output.hits.push(EngineHitEntry {
                key,
                original_rect: input.rect,
                resolved_rect: input.rect,
                interactive: false,
                blocks_original: true,
            });
            return ResolvedEngineWidget::Skip;
        }
        let ov = pass.maps.overrides.get(&key);
        let removed = ov.is_some_and(|o| o.removed);
        let visible = ov.and_then(|o| o.visible).unwrap_or(true);
        if removed || !visible {
            pass.output.hits.push(EngineHitEntry {
                key,
                original_rect: input.rect,
                resolved_rect: input.rect,
                interactive: false,
                blocks_original: true,
            });
            return ResolvedEngineWidget::Skip;
        }
        let rect = ov.and_then(|o| o.rect).unwrap_or(input.rect);
        let active = ov.and_then(|o| o.active).unwrap_or(input.active);
        let moved = ov.and_then(|o| o.rect).is_some();
        let hovered = if moved || (active && !input.active) {
            point_in_rect(pass.cursor, rect)
        } else {
            input.hovered
        };
        pass.output.hovered.insert(key.clone(), hovered);
        let tooltip_narration = if let Some(Some(tip)) = ov.and_then(|o| o.tooltip.as_ref()) {
            let narration = tip.narration.clone();
            if hovered {
                let started = pass.maps.hover_since.get(&key).copied().unwrap_or(pass.now);
                pass.next_hover_since.insert(key.clone(), started);
                let delay = ov.and_then(|o| o.tooltip_delay).unwrap_or(Duration::ZERO);
                if pass.now.duration_since(started) >= delay {
                    pass.output.pending_tooltip = Some(PendingWidgetTooltip {
                        tooltip: tip.clone(),
                        rect,
                        focused_only: false,
                    });
                }
            }
            Some(narration)
        } else {
            if hovered {
                let started = pass.maps.hover_since.get(&key).copied().unwrap_or(pass.now);
                pass.next_hover_since.insert(key.clone(), started);
            }
            None
        };
        let blocks_original = moved || (!active && input.active);
        let resolved_tab_group = ov
            .and_then(|o| o.tab_order_group)
            .unwrap_or(input.tab_order_group);
        LAST_TARGET_META.set(Some((key.clone(), resolved_tab_group)));
        pass.output.hits.push(EngineHitEntry {
            key,
            original_rect: input.rect,
            resolved_rect: rect,
            interactive: active,
            blocks_original,
        });
        let Some(ov) = ov.filter(|o| !o.is_identity()) else {
            return ResolvedEngineWidget::Unchanged;
        };
        ResolvedEngineWidget::Draw(ResolvedWidgetProps {
            rect,
            message: ov.message.clone(),
            active,
            hovered,
            alpha: ov.alpha.unwrap_or(1.0),
            label_text_color: ov.label_text_color.flatten(),
            tab_order_group: resolved_tab_group,
            sprites: ov.sprites.clone().flatten(),
            override_render_highlighted_sprite: ov.override_render_highlighted_sprite.flatten(),
            tooltip_narration,
        })
    })
}

/// Apply resolved mod overrides to a widget's [`Narratable`] record before storing it.
pub fn apply_narration_overrides(
    narratable: &mut Narratable,
    rect: [f32; 4],
    active: bool,
    hovered: bool,
    message: Option<&str>,
    tab_order_group: i32,
    tooltip_hint: Option<String>,
) {
    narratable.rect = rect;
    narratable.hovered = Some(hovered);
    narratable.order = tab_order_group;
    if let Some(hint) = tooltip_hint {
        narratable.hint = Some(hint);
    }
    match &mut narratable.what {
        What::Button {
            message: msg,
            active: a,
        }
        | What::Slider {
            message: msg,
            active: a,
        }
        | What::Cycle {
            message: msg,
            active: a,
            ..
        }
        | What::Checkbox {
            message: msg,
            active: a,
            ..
        }
        | What::EditBox {
            message: msg,
            active: a,
            ..
        } => {
            *a = active;
            if let Some(override_msg) = message {
                *msg = override_msg.to_owned();
            }
        }
        What::Named { title, active: a }
        | What::NamedCycle {
            title, active: a, ..
        } => {
            *a = active;
            if let Some(override_msg) = message {
                *title = override_msg.to_owned();
            }
        }
        What::Text { message: msg } => {
            if let Some(override_msg) = message {
                *msg = override_msg.to_owned();
            }
        }
        What::List { .. }
        | What::ContainerList { .. }
        | What::Recipe { .. }
        | What::Group { .. } => {}
    }
}

/// Multiply the alpha byte of a packed ABGR (`HudBatch`) colour by `alpha` clamped to `0.0..=1.0`.
pub fn scale_abgr_alpha(abgr: u32, alpha: f32) -> u32 {
    if (alpha - 1.0).abs() <= f32::EPSILON {
        return abgr;
    }
    let a = (((abgr >> 24) & 0xFF) as f32 * alpha.clamp(0.0, 1.0))
        .round()
        .clamp(0.0, 255.0) as u32;
    (abgr & 0x00FF_FFFF) | (a << 24)
}

/// Packed ABGR white with opacity `alpha` (`ARGB.white(alpha)`).
pub fn white_abgr_alpha(alpha: f32) -> u32 {
    scale_abgr_alpha(u32::MAX, alpha)
}

/// Effective label text colour in packed ABGR (`HudBatch` format): optional ARGB override else
/// vanilla white (`0xFFFF_FFFF`) when active or grey (`0xFFA0_A0A0`) when inactive, scaled by
/// `alpha`.
pub fn label_abgr_color(active: bool, override_argb: Option<u32>, alpha: f32) -> u32 {
    let base_argb = override_argb.unwrap_or(if active { 0xFFFF_FFFF } else { 0xFFA0_A0A0 });
    scale_abgr_alpha(crate::hud_layout::abgr(base_argb), alpha)
}

/// Multiply the alpha byte of a packed ARGB (`screen_draw::Draw`) colour by `alpha` clamped to
/// `0.0..=1.0`.
pub fn scale_argb_alpha(argb: u32, alpha: f32) -> i32 {
    if (alpha - 1.0).abs() <= f32::EPSILON {
        return argb as i32;
    }
    let a = (((argb >> 24) & 0xFF) as f32 * alpha.clamp(0.0, 1.0))
        .round()
        .clamp(0.0, 255.0) as u32;
    ((argb & 0x00FF_FFFF) | (a << 24)) as i32
}

/// Whether the current thread is inside an active [`with_widget_overrides`] draw pass.
#[inline]
pub fn in_override_pass() -> bool {
    HAS_OVERRIDES.get()
}

/// Effective label text colour in packed ARGB (`screen_draw::Draw` format).
pub fn label_argb_color(active: bool, override_argb: Option<u32>, alpha: f32) -> i32 {
    let base = override_argb.unwrap_or(if active { 0xFFFF_FFFF } else { 0xFFA0_A0A0 });
    scale_argb_alpha(base, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets;

    #[test]
    fn collector_captures_widgets_and_restores_live_buffers_including_on_panic() {
        let saved = widgets::take();
        widgets::end_frame(9_001);
        widgets::record("LiveButton", [1.0, 2.0, 30.0, 20.0], true);
        widgets::target([1.0, 2.0, 30.0, 20.0]);
        widgets::request_cursor(Cursor::PointingHand);

        let ((), collected) = with_widget_collector(|| {
            assert!(is_collecting_widgets());
            let label = Text::translate("gui.done", &[]);
            let resolved = resolve_engine_widget(EngineWidgetInput {
                bucket: WidgetBucket::Button,
                rect: [10.0, 20.0, 100.0, 20.0],
                label: Some(&label),
                explicit_key: None,
                active: true,
                hovered: false,
                focused: false,
                tab_order_group: 2,
            });
            assert!(matches!(resolved, ResolvedEngineWidget::Skip));
            widgets::record("Scratch", [0.0, 0.0, 10.0, 10.0], true);
            widgets::request_cursor(Cursor::Ibeam);
        });
        assert!(!is_collecting_widgets());
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].key.key, "gui.done");
        assert_eq!(collected[0].key.bucket, WidgetBucket::Button);
        assert_eq!(collected[0].key.occurrence, 0);
        assert_eq!(collected[0].rect, [10.0, 20.0, 100.0, 20.0]);
        assert_eq!(collected[0].tab_order_group, 2);
        assert_eq!(
            collected[0]
                .message
                .as_ref()
                .and_then(Text::translation_key_hint)
                .as_deref(),
            Some("gui.done")
        );

        // Explicit identity keys stay internal and never leak into `message`; empty labels also
        // normalize to `None` while keeping distinct occurrence keys.
        let ((), unlabelled) = with_widget_collector(|| {
            let empty = Text::empty();
            let _ = resolve_engine_widget(EngineWidgetInput {
                bucket: WidgetBucket::Button,
                rect: [1.0, 2.0, 20.0, 18.0],
                label: None,
                explicit_key: Some("recipe_book/button"),
                active: true,
                hovered: false,
                focused: false,
                tab_order_group: 0,
            });
            for idx in 0..2 {
                let _ = resolve_engine_widget(EngineWidgetInput {
                    bucket: WidgetBucket::Button,
                    rect: [10.0 + idx as f32 * 24.0, 2.0, 20.0, 20.0],
                    label: Some(&empty),
                    explicit_key: None,
                    active: true,
                    hovered: false,
                    focused: false,
                    tab_order_group: 0,
                });
            }
        });
        assert_eq!(unlabelled.len(), 3);
        assert_eq!(unlabelled[0].key.key, "recipe_book/button");
        assert!(unlabelled[0].message.is_none());
        assert_eq!(unlabelled[1].key.key, "");
        assert_eq!(unlabelled[1].key.occurrence, 0);
        assert!(unlabelled[1].message.is_none());
        assert_eq!(unlabelled[2].key.key, "");
        assert_eq!(unlabelled[2].key.occurrence, 1);
        assert!(unlabelled[2].message.is_none());

        // Live cursor and widget buffers were restored untouched.
        assert_eq!(widgets::take_cursor(), Cursor::PointingHand);
        widgets::end_frame(9_002);
        let live = widgets::last_labelled();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].label, "LiveButton");

        // Panic inside collector still clears collecting state and restores live buffers.
        widgets::record("BeforePanic", [5.0, 6.0, 40.0, 20.0], true);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = with_widget_collector(|| {
                widgets::record("InsidePanic", [0.0, 0.0, 1.0, 1.0], true);
                panic!("simulated collector failure");
            });
        }));
        assert!(panicked.is_err());
        assert!(!is_collecting_widgets());
        widgets::end_frame(9_003);
        let after = widgets::last_labelled();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].label, "BeforePanic");

        widgets::put(saved);
    }

    #[test]
    fn stable_key_preserves_translation_keys_and_value_prefixes() {
        let done = Text::translate("gui.done", &[]);
        assert_eq!(stable_key_from_text(&done), "gui.done");

        let fov_70 = Text::translate(
            "options.generic_value",
            &[Text::translate("options.fov", &[]), Text::literal("70")],
        );
        let fov_90 = Text::translate(
            "options.generic_value",
            &[Text::translate("options.fov", &[]), Text::literal("90")],
        );
        assert_eq!(stable_key_from_text(&fov_70), stable_key_from_text(&fov_90));
        assert_eq!(
            stable_key_from_text(&fov_70),
            "options.generic_value:options.fov"
        );

        let lit_70 = Text::literal("FOV: 70");
        let lit_90 = Text::literal("FOV: 90");
        assert_eq!(stable_key_from_text(&lit_70), "FOV");
        assert_eq!(stable_key_from_text(&lit_70), stable_key_from_text(&lit_90));
    }

    #[test]
    fn overrides_apply_all_properties_track_occurrences_and_respect_without_overrides() {
        let mut maps = EngineOverrideMaps::default();
        let done_key_0 = EngineWidgetKey {
            key: "gui.done".into(),
            bucket: WidgetBucket::Button,
            occurrence: 0,
        };
        let done_key_1 = EngineWidgetKey {
            key: "gui.done".into(),
            bucket: WidgetBucket::Button,
            occurrence: 1,
        };
        maps.overrides.insert(
            done_key_0.clone(),
            EngineWidgetOverride {
                removed: true,
                ..Default::default()
            },
        );
        let tip = EngineWidgetTooltip {
            message: Text::literal("Tip"),
            narration: "Tip".into(),
            component: None,
            style: None,
        };
        maps.overrides.insert(
            done_key_1.clone(),
            EngineWidgetOverride {
                rect: Some([50.0, 60.0, 120.0, 20.0]),
                message: Some(Text::literal("Apply")),
                active: Some(false),
                alpha: Some(0.5),
                label_text_color: Some(Some(0xFF11_2233)),
                tooltip: Some(Some(tip)),
                tooltip_delay: Some(Duration::from_millis(200)),
                tab_order_group: Some(4),
                override_render_highlighted_sprite: Some(Some(true)),
                ..Default::default()
            },
        );

        let label = Text::translate("gui.done", &[]);
        let t0 = Instant::now();
        let ((), out0) = with_widget_overrides(&mut maps, (60.0, 70.0), t0, || {
            assert!(in_override_pass());
            // Occurrence 0 is removed -> Skip.
            assert!(matches!(
                resolve_engine_widget(EngineWidgetInput {
                    bucket: WidgetBucket::Button,
                    rect: [10.0, 10.0, 80.0, 20.0],
                    label: Some(&label),
                    explicit_key: None,
                    active: true,
                    hovered: false,
                    focused: false,
                    tab_order_group: 0,
                }),
                ResolvedEngineWidget::Skip
            ));
            // Occurrence 1 still resolves to its own key even though occurrence 0 was removed.
            match resolve_engine_widget(EngineWidgetInput {
                bucket: WidgetBucket::Button,
                rect: [10.0, 40.0, 80.0, 20.0],
                label: Some(&label),
                explicit_key: None,
                active: true,
                hovered: false,
                focused: false,
                tab_order_group: 0,
            }) {
                ResolvedEngineWidget::Draw(props) => {
                    assert_eq!(props.rect, [50.0, 60.0, 120.0, 20.0]);
                    assert_eq!(
                        props.message.as_ref().map(Text::plain).as_deref(),
                        Some("Apply")
                    );
                    assert!(!props.active);
                    assert!(props.hovered);
                    assert_eq!(props.alpha, 0.5);
                    assert_eq!(props.label_text_color, Some(0xFF11_2233));
                    assert_eq!(props.tab_order_group, 4);
                    assert_eq!(props.override_render_highlighted_sprite, Some(true));
                    assert_eq!(props.tooltip_narration.as_deref(), Some("Tip"));
                }
                other => panic!("expected Draw, got {other:?}"),
            }
            // Mounted widget rendering bypasses engine overrides while keeping the outer override
            // pass active so nested `with_widget_overrides` calls stay guarded.
            without_engine_overrides(|| {
                assert!(in_override_pass());
                assert!(matches!(
                    resolve_engine_widget(EngineWidgetInput {
                        bucket: WidgetBucket::Button,
                        rect: [10.0, 10.0, 80.0, 20.0],
                        label: Some(&label),
                        explicit_key: None,
                        active: true,
                        hovered: false,
                        focused: false,
                        tab_order_group: 0,
                    }),
                    ResolvedEngineWidget::Unchanged
                ));
            });
        });
        assert!(!in_override_pass());
        // Panic inside an override pass still clears `in_override_pass()` and restores `maps`.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = with_widget_overrides(&mut maps, (0.0, 0.0), t0, || {
                assert!(in_override_pass());
                panic!("simulated override pass failure");
            });
        }));
        assert!(panicked.is_err());
        assert!(!in_override_pass());
        assert!(maps.overrides.contains_key(&done_key_1));
        assert_eq!(maps.hover_since.get(&done_key_1), Some(&t0));
        // Tooltip delay has not elapsed yet on frame 0.
        assert!(out0.pending_tooltip.is_none());
        assert_eq!(out0.hits.len(), 2);
        assert!(out0.hits[0].blocks_original && !out0.hits[0].interactive);
        assert!(out0.hits[1].blocks_original && !out0.hits[1].interactive);
        assert_eq!(out0.hits[1].resolved_rect, [50.0, 60.0, 120.0, 20.0]);

        // After 250 ms hovering, the delayed tooltip schedules.
        let t1 = t0 + Duration::from_millis(250);
        let ((), out1) = with_widget_overrides(&mut maps, (60.0, 70.0), t1, || {
            let _ = resolve_engine_widget(EngineWidgetInput {
                bucket: WidgetBucket::Button,
                rect: [10.0, 10.0, 80.0, 20.0],
                label: Some(&label),
                explicit_key: None,
                active: true,
                hovered: false,
                focused: false,
                tab_order_group: 0,
            });
            let _ = resolve_engine_widget(EngineWidgetInput {
                bucket: WidgetBucket::Button,
                rect: [10.0, 40.0, 80.0, 20.0],
                label: Some(&label),
                explicit_key: None,
                active: true,
                hovered: false,
                focused: false,
                tab_order_group: 0,
            });
        });
        let pending = out1.pending_tooltip.expect("tooltip elapsed after delay");
        assert_eq!(pending.tooltip.message.plain(), "Tip");
        assert_eq!(pending.rect, [50.0, 60.0, 120.0, 20.0]);
    }

    #[test]
    fn nested_widget_collector_preserves_outer_collector_state() {
        let outer_before = Text::literal("OuterBefore");
        let inner_label = Text::literal("Inner");
        let outer_after = Text::literal("OuterAfter");
        let (inner_collected, outer_collected) = with_widget_collector(|| {
            assert!(matches!(
                resolve_engine_widget(EngineWidgetInput {
                    bucket: WidgetBucket::Button,
                    rect: [0.0, 0.0, 40.0, 20.0],
                    label: Some(&outer_before),
                    explicit_key: None,
                    active: true,
                    hovered: false,
                    focused: false,
                    tab_order_group: 0,
                }),
                ResolvedEngineWidget::Skip
            ));
            let ((), inner) = with_widget_collector(|| {
                assert!(matches!(
                    resolve_engine_widget(EngineWidgetInput {
                        bucket: WidgetBucket::Button,
                        rect: [0.0, 24.0, 40.0, 20.0],
                        label: Some(&inner_label),
                        explicit_key: None,
                        active: true,
                        hovered: false,
                        focused: false,
                        tab_order_group: 0,
                    }),
                    ResolvedEngineWidget::Skip
                ));
            });
            assert!(matches!(
                resolve_engine_widget(EngineWidgetInput {
                    bucket: WidgetBucket::Button,
                    rect: [0.0, 48.0, 40.0, 20.0],
                    label: Some(&outer_after),
                    explicit_key: None,
                    active: true,
                    hovered: false,
                    focused: false,
                    tab_order_group: 0,
                }),
                ResolvedEngineWidget::Skip
            ));
            inner
        });
        assert_eq!(
            inner_collected
                .iter()
                .map(|w| w.key.key.as_str())
                .collect::<Vec<_>>(),
            ["Inner"]
        );
        assert_eq!(
            outer_collected
                .iter()
                .map(|w| w.key.key.as_str())
                .collect::<Vec<_>>(),
            ["OuterBefore", "OuterAfter"]
        );
    }

    #[test]
    fn color_and_alpha_helpers_scale_channels_accurately() {
        assert_eq!(scale_abgr_alpha(0xFFFF_FF00, 1.0), 0xFFFF_FF00);
        assert_eq!(scale_abgr_alpha(0xFFFF_FF00, 0.0), 0x00FF_FF00);
        assert_eq!(white_abgr_alpha(0.5) >> 24, 128);
        assert_eq!(label_abgr_color(true, None, 1.0), 0xFFFF_FFFF);
        assert_eq!(label_abgr_color(false, None, 1.0), 0xFFA0_A0A0);
        // ARGB 0xFF112233 -> ABGR 0xFF332211.
        assert_eq!(label_abgr_color(true, Some(0xFF11_2233), 1.0), 0xFF33_2211);
        assert_eq!(scale_argb_alpha(0xFFFF_0000, 0.0) as u32, 0x00FF_0000);
        assert_eq!(
            label_argb_color(true, Some(0xFF11_2233), 1.0) as u32,
            0xFF11_2233
        );
    }
}
