//! The chunk-unload sweep: rows leave with their chunks independently
//! of whether they wrote records to the world snapshot.
use super::*;

impl crate::game::ServerGame<'_> {
    /// Remove entities with the chunks in `keys`, except for a family's
    /// normal exceptions (such as a boat the player is riding).
    pub(super) fn unload_removed_entities(&mut self, keys: &HashSet<(i32, i32)>) {
        let gone = |p: Vec3| keys.contains(&chunk_of(p));
        // Clear just the opt-out for rows that don't use `forget_entity` here.
        let keep_row = |id, at, transient: &mut HashSet<u32>| {
            if gone(at) {
                transient.remove(&id);
                false
            } else {
                true
            }
        };
        let mut removed: Vec<u32> = Vec::new();
        let riding = self.common.riding;
        let ridden_roots: HashSet<_> = self
            .server
            .lan
            .iter()
            .flat_map(|host| host.hub.guests.iter())
            .filter(|guest| guest.placed && guest.dimension == self.common.dim.id())
            .filter_map(|guest| guest.vehicle)
            .chain(riding)
            .filter_map(|vehicle| self.riding_root(vehicle))
            .collect();
        // Resolve every passenger before any family is removed, so the real
        // root's chunk controls the whole tree even across storage families.
        let leaves = |id, at, mount: Option<u32>| {
            let Some(root) = mount.and_then(|vehicle| self.riding_root(vehicle)) else {
                return !ridden_roots.contains(&id) && gone(at);
            };
            !ridden_roots.contains(&root) && gone(self.saved_vehicle_position(root).unwrap_or(at))
        };
        let leaving: HashSet<u32> = self
            .mob_objects()
            .filter(|m| leaves(m.id, m.position, m.extra.mount))
            .map(|m| m.id)
            .chain(
                self.common
                    .resident_entities
                    .general
                    .rows
                    .iter()
                    .filter(|r| leaves(r.id, r.position, r.mount))
                    .map(|r| r.id),
            )
            .collect();
        #[cfg(feature = "state-digest")]
        let dim = self.common.dim;
        self.common.boats.retain(|b| {
            let keep = !gone(b.position) || ridden_roots.contains(&b.id);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    b.id,
                    sunrise_gameplay_common::game::entity::Entity::native_type(b).as_ref(),
                    Some("unloaded_to_chunk")
                );
                removed.push(b.id);
            }
            keep
        });
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for c in self.common.crystals.iter() {
                if gone(c.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        c.id,
                        "minecraft:end_crystal",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .crystals
            .retain(|c| keep_row(c.id, c.position, &mut self.common.persist.transient));
        self.common.ominous_spawners.retain(|s| {
            let keep = !gone(Vec3::new(s.position[0], s.position[1], s.position[2]));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    s.id,
                    "minecraft:ominous_item_spawner",
                    Some("unloaded_to_chunk")
                );
                removed.push(s.id);
            }
            keep
        });
        if self
            .common
            .dragon
            .as_ref()
            .is_some_and(|d| gone(d.position))
        {
            #[cfg(feature = "state-digest")]
            if let Some(d) = self.common.dragon.as_ref() {
                crate::game::record_departure!(
                    dim,
                    d.id,
                    "minecraft:ender_dragon",
                    Some("unloaded_to_chunk")
                );
            }
            self.discard_resident_dragon();
        }
        self.common.carts.retain(|c| {
            let keep = !gone(c.position) || ridden_roots.contains(&c.id);
            if !keep {
                crate::game::record_departure!(dim, c.id, c.kind.id(), Some("unloaded_to_chunk"));
                removed.push(c.id);
            }
            keep
        });
        self.common.fangs.list.retain(|f| {
            let keep = !gone(Vec3::new(f.pos[0], f.pos[1], f.pos[2]));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    f.id,
                    "minecraft:evoker_fangs",
                    Some("unloaded_to_chunk")
                );
                removed.push(f.id);
            }
            keep
        });
        self.server.others.0.retain(|o| {
            let keep = !gone(o.position) || ridden_roots.contains(&o.id);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    o.id,
                    o.kind.as_str(),
                    Some("unloaded_to_chunk")
                );
                removed.push(o.id);
            }
            keep
        });
        self.common.armor_stands.retain(|a| {
            let keep = !gone(a.position) || ridden_roots.contains(&a.id);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    a.id,
                    "minecraft:armor_stand",
                    Some("unloaded_to_chunk")
                );
                removed.push(a.id);
            }
            keep
        });
        self.common.item_frames.retain(|f| {
            let (lo, hi) = f.bounds();
            let keep = !gone(Vec3::new(
                (lo[0] + hi[0]) / 2.0,
                (lo[1] + hi[1]) / 2.0,
                (lo[2] + hi[2]) / 2.0,
            ));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    f.id,
                    if f.glow {
                        "minecraft:glow_item_frame"
                    } else {
                        "minecraft:item_frame"
                    },
                    Some("unloaded_to_chunk")
                );
                removed.push(f.id);
            }
            keep
        });
        self.common.paintings.retain(|p| {
            let keep = !gone(painting_center(p));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    p.id,
                    "minecraft:painting",
                    Some("unloaded_to_chunk")
                );
                removed.push(p.id);
            }
            keep
        });
        self.common.cushions.retain(|c| {
            let keep = !gone(c.position);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    c.id,
                    "minecraft:cushion",
                    Some("unloaded_to_chunk")
                );
                removed.push(c.id);
            }
            keep
        });
        // The potion clouds written above leave with their chunk; a
        // dragon's breath (no potion) was not written and stays.
        self.common.breath_clouds.retain(|c| {
            let keep = c.potion.is_none() || !gone(Vec3::new(c.at[0], c.at[1], c.at[2]));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    c.id,
                    "minecraft:area_effect_cloud",
                    Some("unloaded_to_chunk")
                );
                removed.push(c.id);
            }
            keep
        });
        // Lightning bolts never save (`EntityType.noSave()`); drop them when their chunk unloads.
        self.common.bolts.retain(|b| {
            let keep = !gone(Vec3::new(b.at[0], b.at[1], b.at[2]));
            if !keep {
                crate::game::record_departure!(
                    dim,
                    b.id,
                    "minecraft:lightning_bolt",
                    Some("unloaded_to_chunk")
                );
                removed.push(b.id);
            }
            keep
        });
        self.common.mobs.retain(|m| {
            let keep = !leaving.contains(&m.id);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    m.id,
                    crate::game::native_hooks::mob_kind(m),
                    Some("unloaded_to_chunk")
                );
                removed.push(m.id);
            }
            keep
        });
        self.common.persist.off_level.retain(|id, _| {
            let keep = !leaving.contains(id);
            if !keep {
                removed.push(*id);
            }
            keep
        });
        if !self.common.persist.off_level.is_empty() {
            // A retained boat or cart root is in no section and never saved:
            // it lives only while a passenger or player still rides it.
            let ridden: HashSet<u32> = self
                .mob_objects()
                .filter_map(|m| m.extra.mount)
                .chain(
                    self.common
                        .resident_entities
                        .general
                        .rows
                        .iter()
                        .filter(|row| !leaving.contains(&row.id))
                        .filter_map(|row| row.mount),
                )
                .chain(ridden_roots.iter().copied())
                .collect();
            self.common.persist.off_level.retain(|id, object| {
                let keep = object.as_mob().is_some() || ridden.contains(id);
                if !keep {
                    removed.push(*id);
                }
                keep
            });
        }
        self.common.dropped.retain(|e| {
            let keep = !gone(e.position);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    e.id,
                    "minecraft:item",
                    Some("unloaded_to_chunk")
                );
                removed.push(e.id);
            }
            keep
        });
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for o in self.common.orbs.iter() {
                if gone(o.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        o.id,
                        "minecraft:experience_orb",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .orbs
            .retain(|o| keep_row(o.id, o.position, &mut self.common.persist.transient));
        // Unloaded with their chunk: PersistentEntitySectionManager.stopTracking.
        self.native_leaves_flush();
        // the ones saved with the chunk go with it (a trident too, unsaved).
        self.common.creatures.projectiles.retain(|p| {
            let leaves = entity_nbt::saved_with_chunk(p) || p.arrow.is_some();
            let keep = !leaves || !gone(p.position);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    p.id,
                    p.entity_type(),
                    Some("unloaded_to_chunk")
                );
                removed.push(p.id);
            }
            keep
        });
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for f in self.common.falling.iter() {
                if gone(f.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        f.id,
                        "minecraft:falling_block",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .falling
            .retain(|f| keep_row(f.id, f.position, &mut self.common.persist.transient));
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for r in self.common.rockets.iter() {
                if gone(r.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        r.id,
                        "minecraft:firework_rocket",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .rockets
            .retain(|r| keep_row(r.id, r.position, &mut self.common.persist.transient));
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for t in self.common.tridents.iter() {
                if gone(t.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        t.id,
                        "minecraft:trident",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .tridents
            .retain(|t| keep_row(t.id, t.position, &mut self.common.persist.transient));
        #[cfg(feature = "state-digest")]
        if crate::game::state_transcript::is_recording() {
            for t in self.common.tnts.iter() {
                if gone(t.position) {
                    crate::game::state_transcript::record_departure(
                        dim,
                        t.id,
                        "minecraft:tnt",
                        Some("unloaded_to_chunk"),
                    );
                }
            }
        }
        self.common
            .tnts
            .retain(|t| keep_row(t.id, t.position, &mut self.common.persist.transient));
        self.common.resident_entities.general.rows.retain(|row| {
            let keep = !leaving.contains(&row.id);
            if !keep {
                crate::game::record_departure!(
                    dim,
                    row.id,
                    sunrise_gameplay_common::game::entity::Entity::native_type(row).as_ref(),
                    Some("unloaded_to_chunk")
                );
                removed.push(row.id);
            }
            keep
        });
        self.common
            .persist
            .anon
            .retain(|_, (at, _)| !keys.contains(&chunk_of(*at)));
        for id in removed {
            self.forget_entity(id);
        }
    }
}
