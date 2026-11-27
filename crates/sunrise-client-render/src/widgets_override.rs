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
    static BYPASS_OVERRIDES: Cell<bool> = const { Cell::new(false) };
    static COLLECTOR: RefCell<Option<CollectorState>> = const { RefCell::new(None) };
    static ACTIVE_OVERRIDES: RefCell<Option<ActiveOverridePass>> = const { RefCell::new(None) };
}

/// Whether a widget discovery pass is currently running on this thread.
pub fn is_collecting_widgets() -> bool {
    COLLECTING.get()
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
    struct Guard {
        saved: Option<super::Saved>,
        prev_cursor: Cursor,
        prev_narrating: bool,
        prev_collecting: bool,
        prev_has_overrides: bool,
        prev_bypass: bool,
        prev_collector: Option<CollectorState>,
        out: Vec<CollectedWidget>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(finished) = COLLECTOR.replace(self.prev_collector.take()) {
                self.out = finished.widgets;
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
    let mut guard = Guard {
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
        out: Vec::new(),
    };
    let result = f();
    let collected = COLLECTOR
        .replace(guard.prev_collector.take())
        .map_or_else(Vec::new, |s| s.widgets);
    // Keep Drop from overwriting `prev_collector` again.
    guard.prev_collector = None;
    let _ = guard.out;
    drop(guard);
    (result, collected)
}

/// Run `f` with `maps` active for engine widget override lookup and hit-map capture. `maps` is
/// moved into thread-local storage for the duration of `f` and unconditionally restored (with
/// updated `hover_since` timestamps) on exit or unwind.
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
            let active = self
                .finished
                .take()
                .or_else(|| ACTIVE_OVERRIDES.replace(self.prev_pass.take()));
            HAS_OVERRIDES.set(self.prev_has_overrides);
            if let Some(mut pass) = active {
                pass.maps.hover_since = pass.next_hover_since;
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
            let message = input.label.cloned().or_else(|| {
                input
                    .explicit_key
                    .filter(|k| !k.is_empty())
                    .map(Text::literal)
            });
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
        let message = input.label.cloned().or_else(|| {
            input
                .explicit_key
                .filter(|k| !k.is_empty())
                .map(Text::literal)
        });
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
            tab_order_group: ov.tab_order_group.unwrap_or(input.tab_order_group),
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

/// Effective label text colour in packed ARGB (`screen_draw::Draw` format).
pub fn label_argb_color(active: bool, override_argb: Option<u32>, alpha: f32) -> i32 {
    let base = override_argb.unwrap_or(if active { 0xFFFF_FFFF } else { 0xFFA0_A0A0 });
    scale_argb_alpha(base, alpha)
}
