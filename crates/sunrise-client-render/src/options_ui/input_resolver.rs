//! Single Options input resolver at the Options entry (`click_with_outcome`, `drag`,
//! `release_with_outcome`, and shared Tab-group ordering).
use super::*;

/// Active pointer-gesture owner kept from press through drag and release.
#[derive(Clone, Debug)]
pub(super) struct ActivePointerTarget {
    pub(super) page_key: u64,
    pub(super) key: crate::widgets::EngineWidgetKey,
    pub(super) original_rect: [f32; 4],
    pub(super) default_active: bool,
    pub(super) size: (f32, f32),
    pub(super) right: bool,
}

/// Resolution of an initial pointer press against the receiving page's current widgets and
/// active mod overrides.
pub(super) enum PointerPressRoute {
    PassThrough,
    Block,
    Select {
        target: ActivePointerTarget,
        mapped: (f32, f32),
    },
}

/// Shared Tab / Shift+Tab index step in stable `tab_order_group` order for both standard Options
/// pages and hosted `settings_shell` pages.
pub(crate) fn tab_step_grouped(
    groups: &[i32],
    current: Option<usize>,
    shift: bool,
) -> Option<usize> {
    if groups.is_empty() {
        return None;
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&i| groups[i]);
    let at = current.and_then(|cur| order.iter().position(|&i| i == cur));
    let next_pos = match at {
        None => {
            if shift {
                order.len() - 1
            } else {
                0
            }
        }
        Some(p) => {
            if shift {
                (p + order.len() - 1) % order.len()
            } else {
                (p + 1) % order.len()
            }
        }
    };
    Some(order[next_pos])
}

fn map_rect_point(
    pt: (f32, f32),
    [rx, ry, rw, rh]: [f32; 4],
    [ox, oy, ow, oh]: [f32; 4],
) -> (f32, f32) {
    if rw <= 0.0 || rh <= 0.0 {
        return pt;
    }
    let u = (pt.0 - rx) / rw;
    let v = (pt.1 - ry) / rh;
    (ox + u * ow, oy + v * oh)
}

impl OptionsUi {
    /// Run the existing read-only widget discovery pass for the current Options page at `size`.
    pub(super) fn collect_page_widgets(
        &mut self,
        s: &Settings,
        atlas: &HudAtlas,
        size: (f32, f32),
    ) -> Vec<crate::widgets::CollectedWidget> {
        let mut batch = HudBatch::new(
            [
                size.0.max(1.0).round() as u32,
                size.1.max(1.0).round() as u32,
            ],
            s.gui_scale_inputs(),
        );
        batch.width = size.0;
        batch.height = size.1;
        let ((), collected) = crate::widgets::with_widget_collector(|| {
            self.draw_contents(s, &mut batch, atlas, (-1.0, -1.0));
        });
        collected
    }

    /// Single fresh lookup shared by press, resize-fallback drag, and release: collects the current
    /// page's live widgets and partitions them into eligible
    /// `(key, original_rect, resolved_rect, default_active)` entries and blocked original hitboxes.
    fn resolve_page_widgets(
        &mut self,
        s: &Settings,
        atlas: &HudAtlas,
        size: (f32, f32),
    ) -> (
        Vec<(crate::widgets::EngineWidgetKey, [f32; 4], [f32; 4], bool)>,
        Vec<[f32; 4]>,
    ) {
        let collected = self.collect_page_widgets(s, atlas, size);
        let mut eligible = Vec::new();
        let mut blocked_originals = Vec::new();
        for item in collected {
            match crate::widgets::resolve_engine_key_input(&item.key, item.rect, item.active) {
                Some(resolved) => {
                    if resolved != item.rect {
                        blocked_originals.push(item.rect);
                    }
                    eligible.push((item.key, item.rect, resolved, item.active));
                }
                None => {
                    blocked_originals.push(item.rect);
                }
            }
        }
        (eligible, blocked_originals)
    }

    /// Re-verify an active pointer target against the current page via [`Self::resolve_page_widgets`].
    fn lookup_active_pointer(
        &mut self,
        s: &Settings,
        atlas: &HudAtlas,
        size: (f32, f32),
        target: &ActivePointerTarget,
    ) -> Option<([f32; 4], [f32; 4])> {
        if target.page_key != self.sub_page_key() {
            return None;
        }
        let (eligible, _) = self.resolve_page_widgets(s, atlas, size);
        eligible
            .into_iter()
            .find(|(key, _, _, _)| *key == target.key)
            .map(|(_, orig, res, _)| (orig, res))
    }

    /// Resolve a pointer press at `(x, y)` against the current page's live layout and overrides.
    pub(super) fn resolve_pointer_press(
        &mut self,
        s: &Settings,
        atlas: &HudAtlas,
        (x, y): (f32, f32),
        size: (f32, f32),
        right: bool,
    ) -> PointerPressRoute {
        let (eligible, blocked_originals) = self.resolve_page_widgets(s, atlas, size);
        // An eligible widget covering (x, y) wins even if another widget was hidden or moved away
        // from this rectangle; moved widgets take priority over unmoved ones underneath.
        let hit = eligible
            .iter()
            .rev()
            .find(|(_, orig, res, _)| {
                res != orig && ui::hovered((x, y), res[0], res[1], res[2], res[3])
            })
            .or_else(|| {
                eligible
                    .iter()
                    .rev()
                    .find(|(_, _, res, _)| ui::hovered((x, y), res[0], res[1], res[2], res[3]))
            });
        if let Some((key, original_rect, resolved_rect, default_active)) = hit {
            let mapped = if resolved_rect != original_rect {
                map_rect_point((x, y), *resolved_rect, *original_rect)
            } else {
                (x, y)
            };
            return PointerPressRoute::Select {
                target: ActivePointerTarget {
                    page_key: self.sub_page_key(),
                    key: key.clone(),
                    original_rect: *original_rect,
                    default_active: *default_active,
                    size,
                    right,
                },
                mapped,
            };
        }
        if blocked_originals
            .iter()
            .any(|rect| ui::hovered((x, y), rect[0], rect[1], rect[2], rect[3]))
        {
            return PointerPressRoute::Block;
        }
        PointerPressRoute::PassThrough
    }

    /// Clear any active drag across all Options pages without firing release actions.
    pub(super) fn cancel_active_drag(&mut self) {
        self.active_pointer = None;
        self.dragging = None;
        self.scrollbar_drag_y = None;
        if let Some(shell) = self.shell_mut() {
            shell.cancel_drag();
        }
        if let Some(c) = &mut self.calibration {
            let _ = c.release();
        }
        if let Some(p) = &mut self.mods_screen {
            p.cancel_drag();
        }
        if let Some(p) = &mut self.pack_screen {
            p.cancel_drag();
        }
    }

    /// Resolve a drag against the active pointer target; cancels without action if the widget
    /// disappeared or became ineligible mid-drag.
    pub(super) fn drag_resolved(
        &mut self,
        s: &mut Settings,
        atlas: &HudAtlas,
        (x, y): (f32, f32),
        size: (f32, f32),
        framebuffer: [i32; 2],
        right: bool,
    ) -> Option<(bool, Vec<Event>)> {
        if !crate::widgets::has_input_overrides() {
            return None;
        }
        let target = self.active_pointer.clone()?;
        if target.right != right || target.page_key != self.sub_page_key() {
            self.cancel_active_drag();
            return Some((false, Vec::new()));
        }
        let geom = if target.size == size {
            crate::widgets::resolve_captured_engine_key_drag(
                &target.key,
                target.original_rect,
                target.default_active,
            )
        } else {
            self.lookup_active_pointer(s, atlas, size, &target)
        };
        let Some((original_rect, resolved_rect)) = geom else {
            self.cancel_active_drag();
            return Some((false, Vec::new()));
        };
        let mapped = if resolved_rect != original_rect {
            map_rect_point((x, y), resolved_rect, original_rect)
        } else {
            (x, y)
        };
        let result = crate::widgets::with_selected_engine_widget(&target.key, || {
            self.drag_inner(s, atlas, mapped, size, framebuffer, right)
        });
        Some(result)
    }

    /// Resolve a release against the active pointer target via [`Self::resolve_page_widgets`];
    /// cancels without firing release events if the widget disappeared or became ineligible.
    pub(super) fn release_resolved(
        &mut self,
        s: &Settings,
        atlas: &HudAtlas,
        size: (f32, f32),
    ) -> Option<(Vec<Event>, bool)> {
        let target = self.active_pointer.take()?;
        if !crate::widgets::has_input_overrides() {
            return None;
        }
        if self
            .lookup_active_pointer(s, atlas, size, &target)
            .is_none()
        {
            self.cancel_active_drag();
            return Some((Vec::new(), false));
        }
        let result =
            crate::widgets::with_selected_engine_widget(&target.key, || self.release_inner());
        Some(result)
    }
}
