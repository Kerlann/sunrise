//! The `screen_init_post` hooks and helpers: declarations the mod API generator reads (see `hooks/mod.rs`).

pub trait Hooks {
    /// A screen initialized or rebuilt its children (`ScreenEvent.Init.Post`, vanilla's
    /// `Screen.init` / `Screen.rebuildWidgets`), after `screen_init_pre`. Use `event.widgets` to
    /// inspect, mutate, hide, disable, move, remove, or mount buttons, text fields, and custom
    /// widgets on every engine-drawn and mod screen. A layout-only `repositionElements` resize
    /// clears bounds overrides on the screen's own widgets while keeping other overrides and
    /// mounted widgets and does not fire this hook; other resizes and sub-page switches rebuild
    /// and fire `screen_init_pre` then `screen_init_post`. A new chat instance initializes once
    /// even when it replaces one with the same ID and rectangle. A vetoed opening does not
    /// initialize a screen. A panicking listener publishes no edits.
    #[hook(
        receiver = "read",
        dispatch = "flat",
        side = "client",
        version = 5,
        contract = "listen",
        staged_surface = "widgets",
        neoforge = "ScreenEvent.Init.Post",
        event_per_listener = true,
        cancellable = false
    )]
    fn screen_init_post(screen: &ScreenView<'_>, widgets: &mut dyn ScreenWidgets);
}
