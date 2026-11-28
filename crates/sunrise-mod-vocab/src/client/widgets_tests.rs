use super::*;
use crate::{Host, Identifier};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Default)]
struct FakeHost;
impl Host for FakeHost {}
crate::impl_host_areas!(FakeHost);
impl ClientHost for FakeHost {}
crate::impl_client_host_areas!(FakeHost);

struct StubDraw;
impl DrawContext for StubDraw {
    fn with_scissor(
        &mut self,
        _: [f32; 4],
        _: &mut dyn FnMut(&mut dyn DrawContext),
    ) -> Result<(), crate::GuiClipRefusal> {
        unreachable!("widgets under test do not open a scissor")
    }
}

#[test]
fn button_builder_defaults_modifiers_and_callback_unpack() {
    assert_eq!(Button::SMALL_WIDTH, 120.0);
    assert_eq!(Button::DEFAULT_WIDTH, 150.0);
    assert_eq!(Button::BIG_WIDTH, 200.0);
    assert_eq!(Button::DEFAULT_HEIGHT, 20.0);
    assert_eq!(Button::DEFAULT_SPACING, 8.0);

    let pressed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&pressed);
    let sprites = ButtonSprites {
        highlighted: Some("example:btn_hover".into()),
        disabled: Some("example:btn_disabled".into()),
        label_offset: [2.0, -1.0],
        ..ButtonSprites::new("example:btn")
    };
    let tooltip = WidgetTooltip::new(Text::literal("Saves work"))
        .narration(Text::literal("Save button tooltip"))
        .style(Identifier::parse("example:tip").unwrap());
    let button = Button::new(
        [10.0, 20.0, 120.0, 20.0],
        Text::literal("Save"),
        move |state, _host| {
            state.message = Text::literal("Saved");
            state.active = false;
            seen.store(true, Ordering::Relaxed);
        },
    )
    .active(false)
    .visible(false)
    .sprites(sprites.clone())
    .alpha(0.5)
    .label_text_color(0xFF00_FF00)
    .tooltip(tooltip.clone())
    .tooltip_delay(Duration::from_millis(250))
    .tab_order_group(-2)
    .override_render_highlighted_sprite(true)
    .narration(Text::literal("Custom save narration"));

    assert_eq!(button.state().rect, [10.0, 20.0, 120.0, 20.0]);
    assert_eq!(button.state().message, Text::literal("Save"));
    assert!(!button.state().active);
    assert!(!button.state().visible);
    assert_eq!(button.state().sprites, Some(sprites));
    assert_eq!(button.state().alpha, 0.5);
    assert_eq!(button.state().label_text_color, Some(0xFF00_FF00));
    assert_eq!(button.state().tooltip, Some(tooltip));
    assert_eq!(button.state().tooltip_delay, Duration::from_millis(250));
    assert_eq!(button.state().tab_order_group, -2);
    assert_eq!(
        button.state().override_render_highlighted_sprite,
        Some(true)
    );

    let (mut state, on_press, narration) = button.into_parts();
    assert_eq!(narration, Some(Text::literal("Custom save narration")));
    let mut host = FakeHost;
    on_press.expect("Button::new installs a press callback")(&mut state, &mut host);
    assert!(pressed.load(Ordering::Relaxed));
    assert_eq!(state.message, Text::literal("Saved"));
    assert!(!state.active);

    let inactive = Button::inactive([0.0, 0.0, 80.0, 20.0], Text::literal("Locked"));
    assert!(!inactive.state().active);
    assert!(inactive.state().visible);
    assert_eq!(inactive.state().sprites, None);
    assert_eq!(inactive.state().alpha, 1.0);
    let (inactive_state, inactive_cb, inactive_narration) = inactive.into_parts();
    assert!(!inactive_state.active);
    assert!(inactive_cb.is_none());
    assert!(inactive_narration.is_none());
}

#[test]
fn text_field_builder_defaults_modifiers_and_callbacks_unpack() {
    let changed = Arc::new(AtomicBool::new(false));
    let submitted = Arc::new(AtomicBool::new(false));
    let changed_flag = Arc::clone(&changed);
    let submitted_flag = Arc::clone(&submitted);

    let default_parts = TextField::new([4.0, 8.0, 160.0, 20.0], 64).into_parts();
    assert_eq!(default_parts.rect, [4.0, 8.0, 160.0, 20.0]);
    assert!(default_parts.value.is_empty());
    assert_eq!(default_parts.hint, None);
    assert_eq!(default_parts.max_length, 64);
    assert!(default_parts.editable);
    assert!(default_parts.active);
    assert!(default_parts.visible);
    assert!(default_parts.bordered);
    assert_eq!(default_parts.alpha, 1.0);
    assert_eq!(default_parts.tooltip, None);
    assert_eq!(default_parts.tooltip_delay, Duration::ZERO);
    assert_eq!(default_parts.tab_order_group, 0);
    assert!(default_parts.on_change.is_none());
    assert!(default_parts.on_submit.is_none());

    let tip = WidgetTooltip::new(Text::literal("Filter"));
    let parts = TextField::new([12.0, 24.0, 200.0, 20.0], 32)
        .value("hello")
        .hint(Text::literal("Search"))
        .editable(false)
        .active(false)
        .visible(false)
        .bordered(false)
        .alpha(0.75)
        .tooltip(tip.clone())
        .tooltip_delay(Duration::from_millis(100))
        .tab_order_group(3)
        .on_change(move |text, _host| {
            if text == "world" {
                changed_flag.store(true, Ordering::Relaxed);
            }
        })
        .on_submit(move |text, _host| {
            if text == "world" {
                submitted_flag.store(true, Ordering::Relaxed);
            }
        })
        .into_parts();

    assert_eq!(parts.value, "hello");
    assert_eq!(parts.hint, Some(Text::literal("Search")));
    assert!(!parts.editable);
    assert!(!parts.active);
    assert!(!parts.visible);
    assert!(!parts.bordered);
    assert_eq!(parts.alpha, 0.75);
    assert_eq!(parts.tooltip, Some(tip));
    assert_eq!(parts.tooltip_delay, Duration::from_millis(100));
    assert_eq!(parts.tab_order_group, 3);

    let mut host = FakeHost;
    parts.on_change.expect("on_change installed")("world", &mut host);
    parts.on_submit.expect("on_submit installed")("world", &mut host);
    assert!(changed.load(Ordering::Relaxed));
    assert!(submitted.load(Ordering::Relaxed));
}

#[test]
fn screen_child_geometry_and_color_helpers() {
    let mut child = ScreenChild::new(
        ScreenWidgetId::from_raw(7),
        [10.0, 20.0, 100.0, 20.0],
        Some(Text::literal("Play")),
    );
    assert!(child.is_active());
    assert_eq!(child.right(), 110.0);
    assert_eq!(child.bottom(), 40.0);
    assert!(child.is_mouse_over([10.0, 20.0]));
    assert!(!child.is_mouse_over([110.0, 20.0]));
    assert_eq!(child.effective_label_text_color(), 0xFFFF_FFFF);

    child.active = false;
    assert!(!child.is_active());
    assert!(!child.is_mouse_over([50.0, 30.0]));
    assert_eq!(child.effective_label_text_color(), 0xFFA0_A0A0);

    child.label_text_color = Some(0xFF12_3456);
    assert_eq!(child.effective_label_text_color(), 0xFF12_3456);
}

#[test]
fn screen_widget_defaults_and_registrar_ids_work() {
    struct MinimalWidget;
    impl ScreenWidget for MinimalWidget {
        fn bounds(&self) -> [f32; 4] {
            [1.0, 2.0, 30.0, 40.0]
        }
    }

    let mut widget = MinimalWidget;
    let mut host = FakeHost;
    let mut draw = StubDraw;
    assert_eq!(widget.bounds(), [1.0, 2.0, 30.0, 40.0]);
    assert!(widget.visible());
    assert!(widget.active());
    assert!(!widget.captures_input());
    assert!(widget.should_take_focus_after_interaction());
    assert_eq!(widget.tab_order_group(), 0);
    assert_eq!(widget.tooltip(), None);
    widget.render(&host, &mut draw, [0.0, 0.0], false, false, 0.5);
    widget.mouse_moved(&mut host, [5.0, 5.0]);
    assert!(!widget.mouse_clicked(&mut host, [5.0, 5.0], 0, 0, false));
    assert!(!widget.mouse_released(&mut host, [5.0, 5.0], 0, 0));
    assert!(!widget.mouse_dragged(&mut host, [5.0, 5.0], 0, [1.0, 0.0], 0));
    assert!(!widget.mouse_scrolled(&mut host, [5.0, 5.0], [0.0, 1.0], 0));
    assert!(!widget.key_pressed(&mut host, "key.keyboard.a", false, 0));
    assert!(!widget.key_released(&mut host, "key.keyboard.a", 0));
    assert!(!widget.char_typed(&mut host, 'a', 0));
    widget.set_focused(true);

    #[derive(Default)]
    struct FakeRegistrar {
        next: u64,
        children: Vec<ScreenChild>,
        initial_focus: Option<ScreenWidgetId>,
    }
    impl ScreenWidgets for FakeRegistrar {
        fn children(&self) -> &[ScreenChild] {
            &self.children
        }

        fn add_button(&mut self, _button: Button) -> ScreenWidgetId {
            self.next += 1;
            let id = ScreenWidgetId::from_raw(self.next);
            self.children.push(ScreenChild::new(id, [0.0; 4], None));
            id
        }
        fn add_text_field(&mut self, _field: TextField) -> ScreenWidgetId {
            self.next += 1;
            let id = ScreenWidgetId::from_raw(self.next);
            self.children.push(ScreenChild::new(id, [0.0; 4], None));
            id
        }
        fn add_widget(&mut self, _widget: Box<dyn ScreenWidget>) -> ScreenWidgetId {
            self.next += 1;
            let id = ScreenWidgetId::from_raw(self.next);
            self.children.push(ScreenChild::new(id, [0.0; 4], None));
            id
        }
        fn set_initial_focus(&mut self, id: ScreenWidgetId) -> bool {
            if self.children.iter().any(|child| child.id == id) {
                self.initial_focus = Some(id);
                true
            } else {
                false
            }
        }
        fn remove(&mut self, id: ScreenWidgetId) -> bool {
            if let Some(pos) = self.children.iter().position(|child| child.id == id) {
                self.children.remove(pos);
                true
            } else {
                false
            }
        }
        fn clear(&mut self) {
            self.children.clear();
            self.initial_focus = None;
        }
        fn set_bounds(&mut self, id: ScreenWidgetId, rect: [f32; 4]) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.rect = rect;
                true
            } else {
                false
            }
        }
        fn set_message(&mut self, id: ScreenWidgetId, message: Text) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.message = Some(message);
                true
            } else {
                false
            }
        }
        fn set_active(&mut self, id: ScreenWidgetId, active: bool) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.active = active;
                true
            } else {
                false
            }
        }
        fn set_visible(&mut self, id: ScreenWidgetId, visible: bool) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.visible = visible;
                true
            } else {
                false
            }
        }
        fn set_alpha(&mut self, id: ScreenWidgetId, alpha: f32) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.alpha = alpha;
                true
            } else {
                false
            }
        }
        fn set_label_text_color(&mut self, id: ScreenWidgetId, color: Option<u32>) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.label_text_color = color;
                true
            } else {
                false
            }
        }
        fn set_tooltip(&mut self, id: ScreenWidgetId, _tooltip: Option<WidgetTooltip>) -> bool {
            self.children.iter().any(|c| c.id == id)
        }
        fn set_tooltip_delay(&mut self, id: ScreenWidgetId, _delay: Duration) -> bool {
            self.children.iter().any(|c| c.id == id)
        }
        fn set_tab_order_group(&mut self, id: ScreenWidgetId, group: i32) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.tab_order_group = group;
                true
            } else {
                false
            }
        }
        fn set_sprites(&mut self, id: ScreenWidgetId, _sprites: Option<ButtonSprites>) -> bool {
            self.children.iter().any(|c| c.id == id)
        }
        fn set_override_render_highlighted_sprite(
            &mut self,
            id: ScreenWidgetId,
            highlighted: Option<bool>,
        ) -> bool {
            if let Some(c) = self.children.iter_mut().find(|c| c.id == id) {
                c.override_render_highlighted_sprite = highlighted;
                true
            } else {
                false
            }
        }
        fn press(&mut self, id: ScreenWidgetId) -> bool {
            self.child(id).is_some_and(ScreenChild::is_active)
        }
    }

    let mut reg = FakeRegistrar::default();
    let b = reg.add_button(Button::inactive([0.0, 0.0, 20.0, 20.0], Text::literal("B")));
    let f = reg.add_text_field(TextField::new([0.0, 24.0, 100.0, 20.0], 16));
    let w = reg.add_widget(Box::new(MinimalWidget));
    assert_eq!(b.raw(), 1);
    assert_eq!(f.raw(), 2);
    assert_eq!(w.raw(), 3);
    assert!(reg.set_position(b, 5.0, 6.0));
    assert!(reg.set_size(b, 40.0, 20.0));
    assert_eq!(reg.child(b).unwrap().rect, [5.0, 6.0, 40.0, 20.0]);
    assert!(reg.press(b));
    assert!(reg.set_initial_focus(f));
    assert_eq!(reg.initial_focus, Some(f));
    assert!(reg.remove(w));
    assert!(!reg.remove(w));
    assert!(!reg.set_initial_focus(w));
    reg.clear();
    assert!(reg.children().is_empty());
}
