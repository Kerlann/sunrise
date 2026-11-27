//! Mounted and engine screen widgets (`ScreenEvent.Init.Pre`, `ScreenEvent.Init.Post`, and
//! `ClientHost::screen_widgets`).
//!
//! Mods inspect, mutate, hide, disable, move, remove, press, and replace widgets across every
//! engine-drawn and mod screen through [`ScreenWidgets`], and mount new buttons ([`Button`]),
//! single-line text fields ([`TextField`]), and custom widgets ([`ScreenWidget`])
//! (`Screen.addRenderableWidget`). The engine owns drawing, hover, focus, Tab traversal,
//! narration, and mouse/keyboard dispatch for mounted widgets and applies mod overrides on top of
//! engine widgets through the single per-screen override table.
//!
//! Built-in buttons and text fields match vanilla's `Button` (`Button.Builder` / `Button.OnPress`)
//! and `EditBox`, sharing the same editing core ([`TextEdit`]) as the game's own fields.
//!
//! ```
//! use mod_api::client::widgets::{Button, TextField};
//! use mod_api::Text;
//! let _name = TextField::new([20.0, 20.0, 150.0, 20.0], 32)
//!     .value("Steve")
//!     .hint(Text::translate("mymod.screen.player_name", vec![]));
//! let done = Button::new(
//!     [20.0, 50.0, 150.0, 20.0],
//!     Text::translate("gui.done", vec![]),
//!     |state, _host| {
//!         state.active = false;
//!     },
//! );
//! assert!(done.state().active);
//! ```

use crate::{ClientHost, DrawContext, Identifier, ItemStack, Text, TooltipComponent};
use std::fmt;
use std::time::Duration;

#[path = "widgets_text_edit.rs"]
mod text_edit;
pub use text_edit::{
    EditKey, TextEdit, byte_index, char_len, filter_text, is_allowed_chat_character,
    java_whitespace, multiline_word_position, utf16_len, utf16_prefix, word_position,
};

/// Custom GUI art for a [`Button`], using the sprite ids accepted by [`DrawContext::sprite`].
#[derive(Clone, Debug, PartialEq, sunrise_mod_api_macros::Wire)]
#[non_exhaustive]
pub struct ButtonSprites {
    /// The normal sprite. If this is selected but missing, uses the vanilla sprite for that state.
    pub normal: String,
    /// The hovered or focused sprite. An absent or unknown id falls back to `normal`.
    pub highlighted: Option<String>,
    /// The inactive sprite, even when highlighted. An absent or unknown id falls back to `normal`.
    pub disabled: Option<String>,
    /// `[x, y]` in GUI pixels, added to the label's draw position, not its hitbox or overflow
    /// clip. If either coordinate is non-finite, the whole offset is ignored.
    pub label_offset: [f32; 2],
}

impl ButtonSprites {
    /// Uses `normal` in every state, with no label offset.
    pub fn new(normal: impl Into<String>) -> Self {
        Self {
            normal: normal.into(),
            highlighted: None,
            disabled: None,
            label_offset: [0.0; 2],
        }
    }
}

/// A tooltip attached to a screen widget (`net.minecraft.client.gui.components.Tooltip`).
///
/// Shown when the widget is hovered or keyboard-focused after its `tooltip_delay` elapses,
/// wrapped at 170 GUI pixels (`Tooltip.MAX_WIDTH`), and reported as the widget's narration hint.
#[derive(Clone, Debug, PartialEq)]
// no wire: may carry an in-process custom TooltipComponent payload.
#[non_exhaustive]
pub struct WidgetTooltip {
    /// The tooltip text (`Tooltip.create(Component)`).
    pub message: Text,
    /// Optional custom narration text (`Tooltip.create(Component, Component)`); defaults to `message`.
    pub narration: Option<Text>,
    /// Optional tooltip component shown beneath the text (`Tooltip.component()`).
    pub component: Option<TooltipComponent>,
    /// Optional tooltip background/frame style id (`Tooltip.style()`).
    pub style: Option<Identifier>,
}

impl WidgetTooltip {
    /// A tooltip showing `message` and narrating the same text.
    pub fn new(message: impl Into<Text>) -> Self {
        Self {
            message: message.into(),
            narration: None,
            component: None,
            style: None,
        }
    }

    /// Set a custom narration hint distinct from the visible tooltip text.
    pub fn narration(mut self, narration: impl Into<Text>) -> Self {
        self.narration = Some(narration.into());
        self
    }

    /// Attach a [`TooltipComponent`] alongside the tooltip text.
    pub fn component(mut self, component: TooltipComponent) -> Self {
        self.component = Some(component);
        self
    }

    /// Select a custom tooltip texture style (`Tooltip.style()`).
    pub fn style(mut self, style: Identifier) -> Self {
        self.style = Some(style);
        self
    }
}

/// Identifier of a child on the current screen during `Hooks::screen_init_pre`,
/// `Hooks::screen_init_post`, and live [`ClientHost::screen_widgets`] access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
// no wire: screen-local widget handle issued by the engine, never serialized.
// exhaustive: opaque per-screen handle whose inner value is private; mods only compare and pass it back.
pub struct ScreenWidgetId(u64);

impl ScreenWidgetId {
    /// Build a widget identifier from an engine-assigned raw id.
    // hidden: only the engine assigns screen widget identifiers.
    #[doc(hidden)]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Read the engine-assigned raw id.
    // hidden: only the engine maps widget identifiers back to its screen widget tables.
    #[doc(hidden)]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Mutable state of a mounted [`Button`], passed to its [`ButtonPress`] callback.
#[derive(Clone, Debug, PartialEq)]
// no wire: live mounted-button state mutated in-process by its press callback.
#[non_exhaustive]
pub struct ButtonState {
    /// Where it is, `[x, y, width, height]` in GUI pixels (vanilla's are 20 high).
    /// A press callback's non-finite rectangle is ignored, retaining the previous bounds.
    pub rect: [f32; 4],
    /// Its message (`AbstractWidget.getMessage`), centred and scrolled when it does not fit.
    pub message: Text,
    /// An inactive button is greyed out, takes no press and is skipped by Tab.
    pub active: bool,
    /// A hidden button is neither drawn nor interactive.
    pub visible: bool,
    /// Custom art; `None` draws vanilla sprites with no label offset.
    pub sprites: Option<ButtonSprites>,
    /// Opacity multiplier in `0.0..=1.0` (`AbstractWidget.alpha`).
    pub alpha: f32,
    /// Optional ARGB override for the label text colour (NeoForge `AbstractWidget.labelTextColor`).
    pub label_text_color: Option<u32>,
    /// Optional tooltip displayed on hover or keyboard focus (`AbstractWidget.setTooltip`).
    pub tooltip: Option<WidgetTooltip>,
    /// Delay before `tooltip` opens (`AbstractWidget.setTooltipDelay`).
    pub tooltip_delay: Duration,
    /// Tab traversal group (`AbstractWidget.getTabOrderGroup`); lower values come first.
    pub tab_order_group: i32,
    /// Override for drawing the highlighted button sprite (`AbstractButton.setOverrideRenderHighlightedSprite`).
    pub override_render_highlighted_sprite: Option<bool>,
}

/// Press callback for a mounted [`Button`] (`Button.OnPress`).
// no wire: in-process callback invoked by the client when a mounted button is pressed.
pub type ButtonPress = dyn FnMut(&mut ButtonState, &mut dyn ClientHost) + Send + 'static;

/// A push button (`Button`) mounted on a screen through [`ScreenWidgets::add_button`].
// no wire: owns an in-process press callback (`ButtonPress`).
// exhaustive: its fields are private; mods build it with `Button::new` or `Button::inactive`.
pub struct Button {
    state: ButtonState,
    on_press: Option<Box<ButtonPress>>,
    narration: Option<Text>,
}

impl Button {
    /// `Button.SMALL_WIDTH` (120 GUI pixels).
    pub const SMALL_WIDTH: f32 = 120.0;
    /// `Button.DEFAULT_WIDTH` (150 GUI pixels).
    pub const DEFAULT_WIDTH: f32 = 150.0;
    /// `Button.BIG_WIDTH` (200 GUI pixels).
    pub const BIG_WIDTH: f32 = 200.0;
    /// `Button.DEFAULT_HEIGHT` (20 GUI pixels).
    pub const DEFAULT_HEIGHT: f32 = 20.0;
    /// `Button.DEFAULT_SPACING` (8 GUI pixels).
    pub const DEFAULT_SPACING: f32 = 8.0;

    /// An active, visible button with vanilla art and a press callback (`Button.Builder`).
    pub fn new(
        rect: [f32; 4],
        message: impl Into<Text>,
        on_press: impl FnMut(&mut ButtonState, &mut dyn ClientHost) + Send + 'static,
    ) -> Self {
        Self {
            state: ButtonState {
                rect,
                message: message.into(),
                active: true,
                visible: true,
                sprites: None,
                alpha: 1.0,
                label_text_color: None,
                tooltip: None,
                tooltip_delay: Duration::ZERO,
                tab_order_group: 0,
                override_render_highlighted_sprite: None,
            },
            on_press: Some(Box::new(on_press)),
            narration: None,
        }
    }

    /// An inactive, visible button with no press callback and vanilla art.
    pub fn inactive(rect: [f32; 4], message: impl Into<Text>) -> Self {
        Self {
            state: ButtonState {
                rect,
                message: message.into(),
                active: false,
                visible: true,
                sprites: None,
                alpha: 1.0,
                label_text_color: None,
                tooltip: None,
                tooltip_delay: Duration::ZERO,
                tab_order_group: 0,
                override_render_highlighted_sprite: None,
            },
            on_press: None,
            narration: None,
        }
    }

    /// Set whether the button accepts presses and Tab focus.
    pub fn active(mut self, active: bool) -> Self {
        self.state.active = active;
        self
    }

    /// Set whether the button is drawn and interactive.
    pub fn visible(mut self, visible: bool) -> Self {
        self.state.visible = visible;
        self
    }

    /// Use custom GUI art instead of vanilla's button sprites.
    pub fn sprites(mut self, sprites: ButtonSprites) -> Self {
        self.state.sprites = Some(sprites);
        self
    }

    /// Set the opacity multiplier (`AbstractWidget.setAlpha`).
    pub fn alpha(mut self, alpha: f32) -> Self {
        if alpha.is_finite() {
            self.state.alpha = alpha;
        }
        self
    }

    /// Set a custom ARGB label text colour (NeoForge `AbstractWidget.setLabelTextColor`).
    pub fn label_text_color(mut self, argb: u32) -> Self {
        self.state.label_text_color = Some(argb);
        self
    }

    /// Attach a tooltip shown on hover or keyboard focus (`Button.Builder.tooltip`).
    pub fn tooltip(mut self, tooltip: WidgetTooltip) -> Self {
        self.state.tooltip = Some(tooltip);
        self
    }

    /// Set the hover/focus delay before the tooltip opens (`AbstractWidget.setTooltipDelay`).
    pub fn tooltip_delay(mut self, delay: Duration) -> Self {
        self.state.tooltip_delay = delay;
        self
    }

    /// Set the Tab traversal order group (`AbstractWidget.setTabOrderGroup`).
    pub fn tab_order_group(mut self, group: i32) -> Self {
        self.state.tab_order_group = group;
        self
    }

    /// Force or suppress the highlighted button sprite (`AbstractButton.setOverrideRenderHighlightedSprite`).
    pub fn override_render_highlighted_sprite(mut self, highlighted: bool) -> Self {
        self.state.override_render_highlighted_sprite = Some(highlighted);
        self
    }

    /// Override the narration message (`Button.Builder.createNarration`).
    pub fn narration(mut self, narration: impl Into<Text>) -> Self {
        self.narration = Some(narration.into());
        self
    }

    /// Inspect the button's configured state before mounting it.
    pub fn state(&self) -> &ButtonState {
        &self.state
    }

    /// Unpack the button into its state, press callback, and custom narration for the engine's widget store.
    // hidden: only the engine's screen widget store unpacks a `Button` when mounted.
    #[doc(hidden)]
    pub fn into_parts(self) -> (ButtonState, Option<Box<ButtonPress>>, Option<Text>) {
        (self.state, self.on_press, self.narration)
    }
}

impl fmt::Debug for Button {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Button")
            .field("state", &self.state)
            .field("has_on_press", &self.on_press.is_some())
            .field("narration", &self.narration)
            .finish()
    }
}

/// Change or submit callback for a mounted [`TextField`].
// no wire: in-process callback invoked by the client when a mounted text field changes or submits.
pub type TextFieldCallback = dyn FnMut(&str, &mut dyn ClientHost) + Send + 'static;

/// Extracted fields of a [`TextField`] for the engine's screen widget store.
// no wire: engine-only handoff record carrying in-process callbacks.
// hidden: only the engine's screen widget store unpacks a `TextField` when mounted.
#[doc(hidden)]
#[non_exhaustive]
pub struct TextFieldParts {
    /// Where it is, `[x, y, width, height]` in GUI pixels (vanilla's are 20 high).
    pub rect: [f32; 4],
    /// Initial text content.
    pub value: String,
    /// Optional hint text shown when the field is empty (`EditBox.setHint`).
    pub hint: Option<Text>,
    /// Maximum number of UTF-16 code units accepted (`EditBox.setMaxLength`, `insertText`).
    pub max_length: usize,
    /// Whether the player may edit the text (`EditBox.setEditable`).
    pub editable: bool,
    /// Whether the field is active and accepts Tab focus.
    pub active: bool,
    /// Whether the field is drawn and interactive.
    pub visible: bool,
    /// Whether the vanilla `widget/text_field` frame is drawn (`EditBox.setBordered`).
    pub bordered: bool,
    /// Opacity multiplier in `0.0..=1.0` (`AbstractWidget.alpha`).
    pub alpha: f32,
    /// Optional tooltip displayed on hover or keyboard focus (`AbstractWidget.setTooltip`).
    pub tooltip: Option<WidgetTooltip>,
    /// Delay before `tooltip` opens (`AbstractWidget.setTooltipDelay`).
    pub tooltip_delay: Duration,
    /// Tab traversal order group (`AbstractWidget.getTabOrderGroup`).
    pub tab_order_group: i32,
    /// Called whenever the field's text changes (`EditBox.setResponder`).
    pub on_change: Option<Box<TextFieldCallback>>,
    /// Called when Enter is pressed while the field is focused.
    pub on_submit: Option<Box<TextFieldCallback>>,
}

/// A single-line text field (`EditBox`) mounted on a screen through [`ScreenWidgets::add_text_field`].
// no wire: owns in-process change and submit callbacks (`TextFieldCallback`).
// exhaustive: its fields are private; mods build it with `TextField::new`.
pub struct TextField {
    rect: [f32; 4],
    value: String,
    hint: Option<Text>,
    max_length: usize,
    editable: bool,
    active: bool,
    visible: bool,
    bordered: bool,
    alpha: f32,
    tooltip: Option<WidgetTooltip>,
    tooltip_delay: Duration,
    tab_order_group: i32,
    on_change: Option<Box<TextFieldCallback>>,
    on_submit: Option<Box<TextFieldCallback>>,
}

impl TextField {
    /// An empty, editable, active, visible, bordered text field taking up to `max_length`
    /// UTF-16 code units (`EditBox.setMaxLength` and `EditBox.insertText`).
    pub fn new(rect: [f32; 4], max_length: usize) -> Self {
        Self {
            rect,
            value: String::new(),
            hint: None,
            max_length,
            editable: true,
            active: true,
            visible: true,
            bordered: true,
            alpha: 1.0,
            tooltip: None,
            tooltip_delay: Duration::ZERO,
            tab_order_group: 0,
            on_change: None,
            on_submit: None,
        }
    }

    /// Set the initial text content (`EditBox.setValue`).
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Set hint text drawn while the field is empty (`EditBox.setHint`).
    pub fn hint(mut self, hint: impl Into<Text>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Set whether typing and edits can modify the field's text (`EditBox.setEditable`).
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Set whether the field is active and accepts Tab focus.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set whether the field is drawn and interactive.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Set whether the vanilla `widget/text_field` frame is drawn (`EditBox.setBordered`).
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Set the opacity multiplier (`AbstractWidget.setAlpha`).
    pub fn alpha(mut self, alpha: f32) -> Self {
        if alpha.is_finite() {
            self.alpha = alpha;
        }
        self
    }

    /// Attach a tooltip shown on hover or keyboard focus (`AbstractWidget.setTooltip`).
    pub fn tooltip(mut self, tooltip: WidgetTooltip) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    /// Set the hover/focus delay before the tooltip opens (`AbstractWidget.setTooltipDelay`).
    pub fn tooltip_delay(mut self, delay: Duration) -> Self {
        self.tooltip_delay = delay;
        self
    }

    /// Set the Tab traversal order group (`AbstractWidget.setTabOrderGroup`).
    pub fn tab_order_group(mut self, group: i32) -> Self {
        self.tab_order_group = group;
        self
    }

    /// Install a callback invoked whenever the field's text changes (`EditBox.setResponder`).
    pub fn on_change(
        mut self,
        callback: impl FnMut(&str, &mut dyn ClientHost) + Send + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(callback));
        self
    }

    /// Install a callback invoked when Enter is pressed while the field is focused.
    pub fn on_submit(
        mut self,
        callback: impl FnMut(&str, &mut dyn ClientHost) + Send + 'static,
    ) -> Self {
        self.on_submit = Some(Box::new(callback));
        self
    }

    /// Unpack the text field into its fields and callbacks for the engine's widget store.
    // hidden: only the engine's screen widget store unpacks a `TextField` when mounted.
    #[doc(hidden)]
    pub fn into_parts(self) -> TextFieldParts {
        TextFieldParts {
            rect: self.rect,
            value: self.value,
            hint: self.hint,
            max_length: self.max_length,
            editable: self.editable,
            active: self.active,
            visible: self.visible,
            bordered: self.bordered,
            alpha: self.alpha,
            tooltip: self.tooltip,
            tooltip_delay: self.tooltip_delay,
            tab_order_group: self.tab_order_group,
            on_change: self.on_change,
            on_submit: self.on_submit,
        }
    }
}

impl fmt::Debug for TextField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextField")
            .field("rect", &self.rect)
            .field("value", &self.value)
            .field("hint", &self.hint)
            .field("max_length", &self.max_length)
            .field("editable", &self.editable)
            .field("active", &self.active)
            .field("visible", &self.visible)
            .field("bordered", &self.bordered)
            .field("alpha", &self.alpha)
            .field("tooltip", &self.tooltip)
            .field("tooltip_delay", &self.tooltip_delay)
            .field("tab_order_group", &self.tab_order_group)
            .field("has_on_change", &self.on_change.is_some())
            .field("has_on_submit", &self.on_submit.is_some())
            .finish()
    }
}

/// What a context menu entry runs when the player picks it (`ClientHost::open_context_menu`).
// no wire: in-process callback invoked by the client when a context menu action is picked.
pub type ContextMenuAction = dyn FnOnce(&mut dyn ClientHost) + Send + 'static;

/// One row of a context menu: an action, a nested menu, or a separator line.
// no wire: the entries hold the mod's closures and stay in this process.
#[non_exhaustive]
pub enum ContextMenuEntry {
    /// A row the player can pick: the menu closes, then `on_select` runs.
    Action {
        /// The row's text.
        label: Text,
        /// An item drawn before the label.
        icon: Option<ItemStack>,
        /// A disabled row is drawn greyed and can't be picked.
        enabled: bool,
        /// Runs once when the player picks the row, after the menu closed.
        on_select: Box<ContextMenuAction>,
    },
    /// A row that opens `entries` beside it (hover, the right arrow key, or Enter).
    Submenu {
        /// The row's text, drawn with an arrow after it.
        label: Text,
        /// An item drawn before the label.
        icon: Option<ItemStack>,
        /// A disabled row is drawn greyed and doesn't open.
        enabled: bool,
        /// The nested menu's rows; an empty list doesn't open.
        entries: Vec<ContextMenuEntry>,
    },
    /// A thin line between groups of rows.
    Separator,
}

impl ContextMenuEntry {
    /// An enabled action with no icon.
    pub fn action(
        label: impl Into<Text>,
        on_select: impl FnOnce(&mut dyn ClientHost) + Send + 'static,
    ) -> Self {
        Self::Action {
            label: label.into(),
            icon: None,
            enabled: true,
            on_select: Box::new(on_select),
        }
    }
    /// An enabled nested menu with no icon.
    pub fn submenu(label: impl Into<Text>, entries: Vec<ContextMenuEntry>) -> Self {
        Self::Submenu {
            label: label.into(),
            icon: None,
            enabled: true,
            entries,
        }
    }
    /// A separator line.
    pub fn separator() -> Self {
        Self::Separator
    }
    /// The same row with `stack` drawn before its label (nothing for a separator).
    pub fn icon(mut self, stack: ItemStack) -> Self {
        if let Self::Action { icon, .. } | Self::Submenu { icon, .. } = &mut self {
            *icon = Some(stack);
        }
        self
    }
    /// The same row enabled or disabled (nothing for a separator).
    pub fn enabled(mut self, on: bool) -> Self {
        if let Self::Action { enabled, .. } | Self::Submenu { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
}

impl std::fmt::Debug for ContextMenuEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Action { label, enabled, .. } => f
                .debug_struct("Action")
                .field("label", label)
                .field("enabled", enabled)
                .finish_non_exhaustive(),
            Self::Submenu {
                label,
                enabled,
                entries,
                ..
            } => f
                .debug_struct("Submenu")
                .field("label", label)
                .field("enabled", enabled)
                .field("entries", entries)
                .finish_non_exhaustive(),
            Self::Separator => f.write_str("Separator"),
        }
    }
}

/// A custom widget mounted on a screen through [`ScreenWidgets::add_widget`]
/// (`Renderable` + `GuiEventListener`).
///
/// The engine owns focus, hover, Tab traversal, drawing order, and input dispatch for mounted
/// widgets. Override the methods the widget reacts to; every input handler defaults to `false`
/// (unhandled). A pointer press goes only to the first hit child; returning `false` from
/// `mouse_clicked` leaves focus unchanged.
// no wire: in-process custom widget trait object owned by the client screen while it stays open.
pub trait ScreenWidget: Send + 'static {
    /// Where the widget is, `[x, y, width, height]` in GUI pixels.
    fn bounds(&self) -> [f32; 4];

    /// Whether the widget is drawn and receives pointer/keyboard input. Defaults to `true`.
    fn visible(&self) -> bool {
        true
    }

    /// Whether the widget is interactive and eligible for Tab focus. Defaults to `true`.
    fn active(&self) -> bool {
        true
    }

    /// Draw the widget during screen rendering (`Renderable.render`).
    fn render(
        &mut self,
        host: &dyn ClientHost,
        draw: &mut dyn DrawContext,
        mouse: [f32; 2],
        hovered: bool,
        focused: bool,
        partial_tick: f32,
    ) {
        let _ = (host, draw, mouse, hovered, focused, partial_tick);
    }

    /// Observe pointer movement over the screen (`GuiEventListener.mouseMoved`).
    fn mouse_moved(&mut self, host: &mut dyn ClientHost, mouse: [f32; 2]) {
        let _ = (host, mouse);
    }

    /// Handle a mouse button press (`GuiEventListener.mouseClicked`).
    ///
    /// Return `true` when the widget handled the click.
    fn mouse_clicked(
        &mut self,
        host: &mut dyn ClientHost,
        mouse: [f32; 2],
        button: i32,
        modifiers: u32,
        double_click: bool,
    ) -> bool {
        let _ = (host, mouse, button, modifiers, double_click);
        false
    }

    /// Handle a mouse button release (`GuiEventListener.mouseReleased`).
    ///
    /// Return `true` when the widget handled the release.
    fn mouse_released(
        &mut self,
        host: &mut dyn ClientHost,
        mouse: [f32; 2],
        button: i32,
        modifiers: u32,
    ) -> bool {
        let _ = (host, mouse, button, modifiers);
        false
    }

    /// Handle pointer motion while a mouse button is held (`GuiEventListener.mouseDragged`).
    ///
    /// Return `true` when the widget handled the drag.
    fn mouse_dragged(
        &mut self,
        host: &mut dyn ClientHost,
        mouse: [f32; 2],
        button: i32,
        delta: [f32; 2],
        modifiers: u32,
    ) -> bool {
        let _ = (host, mouse, button, delta, modifiers);
        false
    }

    /// Handle a mouse wheel scroll (`GuiEventListener.mouseScrolled`).
    ///
    /// Return `true` when the widget handled the scroll.
    fn mouse_scrolled(
        &mut self,
        host: &mut dyn ClientHost,
        mouse: [f32; 2],
        scroll: [f64; 2],
        modifiers: u32,
    ) -> bool {
        let _ = (host, mouse, scroll, modifiers);
        false
    }

    /// Handle a key press (`GuiEventListener.keyPressed`).
    ///
    /// Return `true` when the widget handled the key.
    fn key_pressed(
        &mut self,
        host: &mut dyn ClientHost,
        key: &str,
        repeat: bool,
        modifiers: u32,
    ) -> bool {
        let _ = (host, key, repeat, modifiers);
        false
    }

    /// Handle a key release (`GuiEventListener.keyReleased`).
    ///
    /// Return `true` when the widget handled the key release.
    fn key_released(&mut self, host: &mut dyn ClientHost, key: &str, modifiers: u32) -> bool {
        let _ = (host, key, modifiers);
        false
    }

    /// Handle a typed character (`GuiEventListener.charTyped`).
    ///
    /// Return `true` when the widget handled the character.
    fn char_typed(&mut self, host: &mut dyn ClientHost, c: char, modifiers: u32) -> bool {
        let _ = (host, c, modifiers);
        false
    }

    /// Whether this widget wants text input focus priority over screen hotkeys (such as `E` to
    /// close inventory screens) while focused. This does not consume IME preedit; custom widgets
    /// receive committed characters via [`Self::char_typed`]. Defaults to `false`.
    fn captures_input(&self) -> bool {
        false
    }

    /// Whether clicking this widget should give it keyboard focus
    /// (`GuiEventListener.shouldTakeFocusAfterInteraction`). Defaults to `true`.
    fn should_take_focus_after_interaction(&self) -> bool {
        true
    }

    /// Tab traversal order group (`TabOrderedElement.getTabOrderGroup`). Defaults to `0`.
    fn tab_order_group(&self) -> i32 {
        0
    }

    /// Optional tooltip displayed while hovered or keyboard-focused. Defaults to `None`.
    fn tooltip(&self) -> Option<WidgetTooltip> {
        None
    }

    /// Called when this widget gains or loses keyboard focus (`GuiEventListener.setFocused`).
    fn set_focused(&mut self, focused: bool) {
        let _ = focused;
    }
}

/// An initialized child listed by [`ScreenWidgets::children`].
#[derive(Clone, Debug, PartialEq)]
// no wire: live screen-local child snapshot containing an opaque ScreenWidgetId.
#[non_exhaustive]
pub struct ScreenChild {
    /// The screen-local handle used to inspect, mutate, press, or remove the child.
    pub id: ScreenWidgetId,
    /// `[x, y, width, height]` in GUI pixels, reflecting any live bounds override.
    pub rect: [f32; 4],
    /// The child's logical message (`AbstractWidget.getMessage`), or `None` when it has none.
    pub message: Option<Text>,
    /// Whether the child accepts input (`AbstractWidget.active`).
    pub active: bool,
    /// Whether the child is drawn and interactive (`AbstractWidget.visible`).
    pub visible: bool,
    /// Whether the pointer hovered this child on its last drawn frame (`AbstractWidget.isHovered`).
    pub hovered: bool,
    /// Opacity multiplier in `0.0..=1.0` (`AbstractWidget.getAlpha`).
    pub alpha: f32,
    /// Optional ARGB label text colour override (NeoForge `AbstractWidget.getLabelTextColor`).
    pub label_text_color: Option<u32>,
    /// Tab traversal order group (`TabOrderedElement.getTabOrderGroup`).
    pub tab_order_group: i32,
    /// Highlighted sprite override (`AbstractButton.setOverrideRenderHighlightedSprite`).
    pub override_render_highlighted_sprite: Option<bool>,
}

impl ScreenChild {
    /// Build the value exposed by the engine's registrar with default widget state.
    // hidden: only the engine issues ScreenWidgetId handles for initialized children.
    #[doc(hidden)]
    pub fn new(id: ScreenWidgetId, rect: [f32; 4], message: Option<Text>) -> Self {
        Self {
            id,
            rect,
            message,
            active: true,
            visible: true,
            hovered: false,
            alpha: 1.0,
            label_text_color: None,
            tab_order_group: 0,
            override_render_highlighted_sprite: None,
        }
    }

    /// Whether the child is both visible and active (`AbstractWidget.isActive`).
    pub fn is_active(&self) -> bool {
        self.visible && self.active
    }

    /// Right edge `x + width` in GUI pixels (`AbstractWidget.getRight`).
    pub fn right(&self) -> f32 {
        self.rect[0] + self.rect[2]
    }

    /// Bottom edge `y + height` in GUI pixels (`AbstractWidget.getBottom`).
    pub fn bottom(&self) -> f32 {
        self.rect[1] + self.rect[3]
    }

    /// Whether `mouse` lies inside this child while it is active and visible
    /// (`AbstractWidget.isMouseOver`).
    pub fn is_mouse_over(&self, mouse: [f32; 2]) -> bool {
        let [mx, my] = mouse;
        let [x, y, w, h] = self.rect;
        self.is_active() && w > 0.0 && h > 0.0 && mx >= x && my >= y && mx < x + w && my < y + h
    }

    /// Effective ARGB colour used for the label text: `label_text_color` when set, otherwise
    /// active white (`0xFFFF_FFFF`) or vanilla's inactive light grey (`0xFFA0_A0A0`,
    /// `AbstractWidget.WithInactiveMessage.defaultInactiveMessage`, `AbstractWidget.java:318`).
    pub fn effective_label_text_color(&self) -> u32 {
        self.label_text_color.unwrap_or(if self.active {
            0xFFFF_FFFF
        } else {
            0xFFA0_A0A0
        })
    }
}

/// Screen-scoped widget collection passed to `Hooks::screen_init_pre`, `Hooks::screen_init_post`,
/// and [`ClientHost::screen_widgets`].
///
/// Covers both the game's own controls on every engine-drawn screen and widgets mounted by mods.
/// Edits last for the current screen instance and clear when the screen closes, rebuilds, switches
/// sub-page, or performs a rebuilding resize (a layout-only `repositionElements` resize clears
/// only bounds overrides on the screen's own widgets and preserves other overrides and mounted mod
/// widgets).
///
/// Mounting a widget with non-finite bounds returns an ID that is not present in the screen.
/// Calling any mutator with a stale or unknown `ScreenWidgetId` returns `false` without panicking.
// no wire: borrowed screen widget surface scoped to a hook or live host borrow.
pub trait ScreenWidgets {
    /// Registered children in initialization order (`ScreenEvent.Init.getListenersList` /
    /// `Screen.children`). Includes the screen's own widgets (unless suppressed by cancelling
    /// `screen_init_pre`) and mounted mod widgets.
    fn children(&self) -> &[ScreenChild];

    /// Look up one listed child by its identifier.
    fn child(&self, id: ScreenWidgetId) -> Option<&ScreenChild> {
        self.children().iter().find(|child| child.id == id)
    }

    /// Mount a vanilla-style push button on the screen (`Screen.addRenderableWidget`).
    fn add_button(&mut self, button: Button) -> ScreenWidgetId;

    /// Mount a vanilla-style single-line text field on the screen (`Screen.addRenderableWidget`).
    fn add_text_field(&mut self, field: TextField) -> ScreenWidgetId;

    /// Mount a custom widget on the screen (`Screen.addRenderableWidget`).
    fn add_widget(&mut self, widget: Box<dyn ScreenWidget>) -> ScreenWidgetId;

    /// Ask the screen to focus `id` when initialization completes (`Screen.setInitialFocus`).
    ///
    /// Returns `true` when `id` names a mounted widget that can take focus.
    fn set_initial_focus(&mut self, id: ScreenWidgetId) -> bool;

    /// Remove a listed child (`ScreenEvent.Init.removeListener` / `Screen.removeWidget`),
    /// including any widget drawn by the game or mounted by a mod. Returns `true` if the child
    /// was present.
    fn remove(&mut self, id: ScreenWidgetId) -> bool;

    /// Remove every listed child from the screen (`Screen.clearWidgets`).
    fn clear(&mut self);

    /// Move and resize `id` to `rect` (`[x, y, width, height]` in GUI pixels;
    /// `AbstractWidget.setRectangle` / `setPosition` / `setSize`).
    ///
    /// Rejects non-finite coordinates or negative dimensions (`false`, no change). Sibling
    /// widgets do not re-layout.
    fn set_bounds(&mut self, id: ScreenWidgetId, rect: [f32; 4]) -> bool;

    /// Move `id` to `(x, y)` while keeping its current size (`AbstractWidget.setPosition`).
    fn set_position(&mut self, id: ScreenWidgetId, x: f32, y: f32) -> bool {
        let Some(rect) = self.child(id).map(|child| child.rect) else {
            return false;
        };
        self.set_bounds(id, [x, y, rect[2], rect[3]])
    }

    /// Resize `id` to `(width, height)` while keeping its current position (`AbstractWidget.setSize`).
    fn set_size(&mut self, id: ScreenWidgetId, width: f32, height: f32) -> bool {
        let Some(rect) = self.child(id).map(|child| child.rect) else {
            return false;
        };
        self.set_bounds(id, [rect[0], rect[1], width, height])
    }

    /// Replace the displayed message of `id` (`AbstractWidget.setMessage`).
    ///
    /// Rejects stale ids or malformed text trees (`false`, no change). Does not change `id`.
    fn set_message(&mut self, id: ScreenWidgetId, message: Text) -> bool;

    /// Enable or disable `id` (`AbstractWidget.active`).
    fn set_active(&mut self, id: ScreenWidgetId, active: bool) -> bool;

    /// Show or hide `id` (`AbstractWidget.visible`). Hidden widgets stay in `children()` so a mod
    /// can make them visible again.
    fn set_visible(&mut self, id: ScreenWidgetId, visible: bool) -> bool;

    /// Set the opacity multiplier of `id` (`AbstractWidget.setAlpha`). Rejects non-finite values.
    fn set_alpha(&mut self, id: ScreenWidgetId, alpha: f32) -> bool;

    /// Override the label text colour of `id` (NeoForge `AbstractWidget.setLabelTextColor`).
    /// Pass `None` to restore vanilla's active/inactive default colour.
    fn set_label_text_color(&mut self, id: ScreenWidgetId, color: Option<u32>) -> bool;

    /// Set or clear the tooltip of `id` (`AbstractWidget.setTooltip`).
    fn set_tooltip(&mut self, id: ScreenWidgetId, tooltip: Option<WidgetTooltip>) -> bool;

    /// Set the hover/focus delay before `id`'s tooltip opens (`AbstractWidget.setTooltipDelay`).
    fn set_tooltip_delay(&mut self, id: ScreenWidgetId, delay: Duration) -> bool;

    /// Set the Tab traversal order group of `id` (`AbstractWidget.setTabOrderGroup`).
    fn set_tab_order_group(&mut self, id: ScreenWidgetId, group: i32) -> bool;

    /// Set or clear custom button sprites for `id` (`None` restores vanilla sprites).
    fn set_sprites(&mut self, id: ScreenWidgetId, sprites: Option<ButtonSprites>) -> bool;

    /// Override whether `id` draws its highlighted button sprite
    /// (`AbstractButton.setOverrideRenderHighlightedSprite`).
    ///
    /// `Some(true)` forces the highlighted sprite while active; `Some(false)` suppresses it even
    /// when hovered; `None` restores vanilla hover/focus selection.
    fn set_override_render_highlighted_sprite(
        &mut self,
        id: ScreenWidgetId,
        highlighted: Option<bool>,
    ) -> bool;

    /// Run the screen's own activation for `id` (`AbstractButton.onPress`), through the same path
    /// a pointer click on its resolved rectangle takes.
    ///
    /// Returns `false` when `id` is stale or when the widget is not `visible && active`.
    ///
    /// **Staging limitation:** `Hooks::screen_init_pre` and `Hooks::screen_init_post` receive
    /// `&dyn ClientHost` rather than `&mut dyn ClientHost`, so the staging registrar has no mutable
    /// host handle to run button callbacks or screen click routing and returns `false`; invoke
    /// `press` live through [`ClientHost::screen_widgets`] from a tick, input, or widget callback.
    fn press(&mut self, id: ScreenWidgetId) -> bool;
}

#[cfg(test)]
#[path = "widgets_tests.rs"]
mod tests;
