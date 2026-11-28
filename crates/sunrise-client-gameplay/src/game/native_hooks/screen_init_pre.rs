use mod_api::{ScreenView, ScreenWidgets};

/// The hook points. Each does nothing in Sunrise.
#[cfg(feature = "client")]
impl crate::game::ClientGame<'_> {
    /// Before a screen builds its default widgets (`ScreenEvent.Init.Pre`).
    pub(in crate::game) fn native_screen_init_pre(
        &self,
        screen: &ScreenView<'_>,
        stage_listener: impl FnMut(&mut dyn FnMut(&mut dyn ScreenWidgets) -> bool),
        canceled: &mut bool,
    ) {
        self.fire_screen_init_pre(screen, stage_listener, canceled);
    }
}
