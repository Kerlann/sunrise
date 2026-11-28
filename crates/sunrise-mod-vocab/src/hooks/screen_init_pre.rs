//! The `screen_init_pre` hook: declarations the mod API generator reads (see `hooks/mod.rs`).

pub trait Hooks {
    /// Before a screen builds its default widgets (`ScreenEvent.Init.Pre`, vanilla's
    /// `Screen.init` / `Screen.rebuildWidgets`). Use `event.widgets` to mount widgets before the
    /// screen's own controls or remove widgets added by earlier `screen_init_pre` listeners.
    /// Cancelling suppresses the screen's own widgets only; the background, slots and pages still
    /// draw. `screen_init_post` still fires afterward with any widgets added here. A panicking
    /// listener publishes no edits and leaves cancellation unchanged.
    #[hook(
        receiver = "read",
        dispatch = "flat",
        side = "client",
        contract = "cancel",
        out = "canceled",
        staged_surface = "widgets",
        neoforge = "ScreenEvent.Init.Pre",
        event_per_listener = true,
        cancellable = true
    )]
    fn screen_init_pre(
        screen: &ScreenView<'_>,
        widgets: &mut dyn ScreenWidgets,
        canceled: &mut bool,
    );
}
