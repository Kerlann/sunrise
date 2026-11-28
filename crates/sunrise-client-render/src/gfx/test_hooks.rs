//! What tests of the game's shader pack loader and frames read from, or set on, a real renderer
//! (behind the `test-support` feature, which `sunrise-client`'s and `sunrise-client-lib`'s tests
//! and `sunrise-parity-fixtures` enable): one struct on the renderer ([`TestHooks`],
//! `Renderer::test_hooks`), and the read-only views of the renderer and a screenshot they need.

use super::{PendingScreenshot, Renderer};

/// The renderer's test seams, kept apart from its own state.
#[derive(Debug, Default)]
pub struct TestHooks {
    /// Whether the last real frame ran Sunrise's tone curve (`sunrise_tone_curve`).
    pub tone_curve_ran: bool,
    /// How many shader pack builds `load_shader_pack` started.
    pub pack_loads: u64,
    /// Every scaled world target fails, as one out of video memory does, so the world draws at
    /// the window's size without a shader pack's passes (`world_post::Post::prepare`'s
    /// fallback).
    pub fail_scaled_targets: bool,
    /// What the game's last load of the pick logged about its `shaders.properties`.
    pub loaded_log: Vec<String>,
}

impl TestHooks {
    /// Whether this frame's world has no scaled target ([`TestHooks::fail_scaled_targets`]),
    /// checked before `world_post::Post::prepare` would make one: `post`'s target is then
    /// dropped as `prepare`'s out-of-memory fallback drops it, and `prepare` isn't called.
    pub(super) fn fail_scaled_target(&self, post: &mut super::world_post::Post) -> bool {
        if self.fail_scaled_targets {
            post.drop_target();
        }
        self.fail_scaled_targets
    }
}

impl Renderer {
    /// Whether a pack `load_shader_pack` started is still building (`poll_shader_pack` has
    /// nothing to run yet).
    pub fn shader_pack_building(&self) -> bool {
        self.pack_loading
            .as_ref()
            .is_some_and(|l| !l.thread.is_finished())
    }
}

impl PendingScreenshot {
    /// The HDR10 copy's size and pixels (16-bit RGB, big-endian PQ codes), when the frame had
    /// one.
    pub fn hdr_pixels(&self) -> Option<([u32; 2], &[u8])> {
        self.hdr.as_ref().map(|h| (h.size, h.pixels.as_slice()))
    }
}
