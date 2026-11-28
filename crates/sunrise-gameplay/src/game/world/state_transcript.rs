//! Per-step entity state, turn and departure transcript to `SUNRISE_STATE_TRANSCRIPT`.
use crate::game::ServerGame;
use std::{
    cell::{Cell, RefCell},
    fmt::Write as _,
    fs::File,
    io::{BufWriter, Write as _},
    path::Path,
};
use sunrise_core::{geometry::Vec3, nbt::Tag, random::LegacyRandom, version::VANILLA_VERSION};
use sunrise_gameplay_common::game::{
    Dim, commands_common::Target, entity::Entity as _, frames_common, native_hooks::mob_kind,
};
use sunrise_sim::{
    join_seq::{JoinSeq, Joined as _},
    lan::playerdata::ints_uuid,
    mobs::leash,
};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

struct TurnRecord {
    id: u32,
    ty: String,
    vehicle: Option<u32>,
}

struct DepartureRecord {
    id: u32,
    ty: String,
    reason: Option<&'static str>,
}

struct Buffer {
    target_dim: Dim,
    turns: Vec<TurnRecord>,
    departures: Vec<DepartureRecord>,
}

impl Buffer {
    const fn new() -> Self {
        Self {
            target_dim: Dim::Overworld,
            turns: Vec::new(),
            departures: Vec::new(),
        }
    }
}

thread_local! {
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static BUFFER: RefCell<Buffer> = const { RefCell::new(Buffer::new()) };
}

#[inline(always)]
pub(in crate::game) fn is_recording() -> bool {
    RECORDING.with(Cell::get)
}

pub(in crate::game) fn record_turn(dim: Dim, id: u32, ty: impl AsRef<str>, vehicle: Option<u32>) {
    BUFFER.with(|cell| {
        let mut buf = cell.borrow_mut();
        if buf.target_dim != dim {
            return;
        }
        buf.turns.push(TurnRecord {
            id,
            ty: namespaced(ty.as_ref()),
            vehicle,
        });
    });
}

pub(in crate::game) fn record_departure(
    dim: Dim,
    id: u32,
    ty: impl AsRef<str>,
    reason: Option<&'static str>,
) {
    BUFFER.with(|cell| {
        let mut buf = cell.borrow_mut();
        if buf.target_dim != dim {
            return;
        }
        if let Some(existing) = buf.departures.iter_mut().find(|d| d.id == id) {
            if existing.reason.is_none() {
                existing.reason = reason;
            }
            return;
        }
        buf.departures.push(DepartureRecord {
            id,
            ty: namespaced(ty.as_ref()),
            reason,
        });
    });
}

pub(in crate::game) struct TickGuard;

impl Drop for TickGuard {
    fn drop(&mut self) {
        RECORDING.with(|flag| flag.set(false));
    }
}

pub(in crate::game) struct TranscriptWriter {
    out: BufWriter<File>,
    enabled: bool,
    step: u64,
    ordinal: u32,
    points: u64,
    rows: u64,
}

impl TranscriptWriter {
    pub(in crate::game) fn open(script: &Path) -> sunrise_game_data::Result<Self> {
        let path = std::env::var_os("SUNRISE_STATE_TRANSCRIPT")
            .ok_or("state_transcript: SUNRISE_STATE_TRANSCRIPT is not set")?;
        let script_bytes = std::fs::read(script)?;
        let script_sha = sha256_hex(&script_bytes);
        let fixture_sha = match std::env::var("SUNRISE_STATE_TRANSCRIPT_FIXTURE_SHA256") {
            Ok(value) => {
                if !is_sha256_hex(&value) {
                    return Err("state_transcript: invalid fixture SHA-256".into());
                }
                value
            }
            Err(_) => EMPTY_SHA256.to_owned(),
        };
        let build = match std::env::var("SUNRISE_STATE_TRANSCRIPT_BUILD") {
            Ok(value) => {
                if value.is_empty() || value.chars().any(char::is_whitespace) {
                    return Err("state_transcript: invalid build identifier".into());
                }
                value
            }
            Err(_) => "sunrise-state-digest".to_owned(),
        };
        let file = File::create(path)?;
        let mut out = BufWriter::new(file);
        writeln!(
            out,
            "H\tformat=sunrise-state-transcript\tversion=1\tengine=sunrise\tscript_sha256={script_sha}\tfixture_sha256={fixture_sha}\tbuild={build}\tgame_version={VANILLA_VERSION}"
        )?;
        Ok(Self {
            out,
            enabled: true,
            step: 0,
            ordinal: 0,
            points: 0,
            rows: 0,
        })
    }

    pub(in crate::game) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(in crate::game) fn begin_tick(&mut self, game: &ServerGame<'_>) -> Option<TickGuard> {
        if !self.enabled {
            return None;
        }
        let target_dim = owner_dim(game).unwrap_or(game.common.dim);
        self.step += 1;
        self.ordinal = 0;
        BUFFER.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.target_dim = target_dim;
            buf.turns.clear();
            buf.departures.clear();
        });
        RECORDING.with(|flag| flag.set(true));
        Some(TickGuard)
    }

    pub(in crate::game) fn record_point(
        &mut self,
        game: &ServerGame<'_>,
        op: usize,
        is_tick: bool,
    ) -> sunrise_game_data::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let expected_dim = owner_dim(game).unwrap_or(game.common.dim);
        if game.common.dim != expected_dim {
            return Err("state_transcript: active level does not match player level".into());
        }
        let members = collect_members(game);
        let phase = if is_tick { "after_tick" } else { "after_op" };
        let ord = self.ordinal;
        self.ordinal += 1;
        self.write_row(&format!(
            "P\tstep={}\tphase={phase}\tord={ord}\top={op}\tgame_time={}\tdim={}",
            self.step,
            game.common.world.game_time,
            game.common.dim.id(),
        ))?;
        self.write_row(&format!(
            "L\tlevel_random={}\trand_value={}\tlast_entity_id={}\tmembers={}",
            fmt_random(&game.common.world.random),
            game.common.block_ticks.random_tick_value,
            game.server.next_entity as i32,
            members.len(),
        ))?;
        let mut line = String::with_capacity(320);
        for (i, member) in members.iter().enumerate() {
            line.clear();
            member.format_into(i, &mut line);
            self.write_row(&line)?;
        }
        if is_tick {
            let (turns, departures) = BUFFER.with(|cell| {
                let mut buf = cell.borrow_mut();
                (
                    std::mem::take(&mut buf.turns),
                    std::mem::take(&mut buf.departures),
                )
            });
            for (n, turn) in turns.iter().enumerate() {
                let (role, vehicle) = match turn.vehicle {
                    Some(v) => ("passenger", v.to_string()),
                    None => ("root", "-".to_owned()),
                };
                self.write_row(&format!(
                    "T\tn={n}\tid={}\ttype={}\trole={role}\tvehicle={vehicle}",
                    turn.id, turn.ty,
                ))?;
            }
            for (n, departure) in departures.iter().enumerate() {
                let reason = departure.reason.unwrap_or("?");
                self.write_row(&format!(
                    "D\tn={n}\tid={}\ttype={}\treason={reason}",
                    departure.id, departure.ty,
                ))?;
            }
        }
        self.points += 1;
        self.out.flush()?;
        Ok(())
    }

    pub(in crate::game) fn complete(&mut self) -> sunrise_game_data::Result<()> {
        writeln!(self.out, "Z\tpoints={}\trows={}", self.points, self.rows)?;
        self.out.flush()?;
        Ok(())
    }

    fn write_row(&mut self, row: &str) -> std::io::Result<()> {
        self.out.write_all(row.as_bytes())?;
        self.out.write_all(b"\n")?;
        self.rows += 1;
        Ok(())
    }
}

enum Health {
    NotLiving,
    Living(f32),
    Unmodeled,
}

struct Member {
    order: (u8, u64, u32),
    id: u32,
    ty: String,
    pos: Vec3,
    motion: Option<Vec3>,
    rot: Option<[f32; 2]>,
    ground: Option<bool>,
    tick_count: Option<i32>,
    fall_distance: Option<f64>,
    fire: Option<i32>,
    air: Option<i32>,
    invulnerable_time: Option<i32>,
    no_gravity: Option<bool>,
    vehicle: Option<u32>,
    passengers: Vec<u32>,
    random: Option<(u64, Option<f64>)>,
    health: Health,
    uuid: Option<u128>,
}

impl Member {
    fn format_into(&self, i: usize, out: &mut String) {
        let _ = write!(out, "M\ti={i}\tid={}\ttype={}", self.id, self.ty);
        let _ = write!(
            out,
            "\tpos={:016x},{:016x},{:016x}",
            self.pos.x.to_bits(),
            self.pos.y.to_bits(),
            self.pos.z.to_bits(),
        );
        match self.motion {
            Some(v) => {
                let _ = write!(
                    out,
                    "\tmotion={:016x},{:016x},{:016x}",
                    v.x.to_bits(),
                    v.y.to_bits(),
                    v.z.to_bits(),
                );
            }
            None => out.push_str("\tmotion=?unmodeled"),
        }
        match self.rot {
            Some([yaw, pitch]) => {
                let _ = write!(out, "\trot={:08x},{:08x}", yaw.to_bits(), pitch.to_bits());
            }
            None => out.push_str("\trot=?unmodeled"),
        }
        match self.ground {
            Some(g) => {
                let _ = write!(out, "\tground={}", u8::from(g));
            }
            None => out.push_str("\tground=?unmodeled"),
        }
        match self.tick_count {
            Some(tc) => {
                let _ = write!(out, "\ttick_count={tc}");
            }
            None => out.push_str("\ttick_count=?unmodeled"),
        }
        match self.fall_distance {
            Some(fd) => {
                let _ = write!(out, "\tfall_distance={:016x}", fd.to_bits());
            }
            None => out.push_str("\tfall_distance=?unmodeled"),
        }
        match self.fire {
            Some(f) => {
                let _ = write!(out, "\tfire={f}");
            }
            None => out.push_str("\tfire=?unmodeled"),
        }
        match self.air {
            Some(a) => {
                let _ = write!(out, "\tair={a}");
            }
            None => out.push_str("\tair=?unmodeled"),
        }
        match self.invulnerable_time {
            Some(it) => {
                let _ = write!(out, "\tinvulnerable_time={it}");
            }
            None => out.push_str("\tinvulnerable_time=?unmodeled"),
        }
        match self.no_gravity {
            Some(ng) => {
                let _ = write!(out, "\tno_gravity={}", u8::from(ng));
            }
            None => out.push_str("\tno_gravity=?unmodeled"),
        }
        match self.vehicle {
            Some(v) => {
                let _ = write!(out, "\tvehicle={v}");
            }
            None => out.push_str("\tvehicle=-"),
        }
        out.push_str("\tpassengers=");
        if self.passengers.is_empty() {
            out.push('-');
        } else {
            for (idx, p) in self.passengers.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{p}");
            }
        }
        match self.random {
            Some((state, gaussian)) => {
                let _ = write!(out, "\trandom={state:012x},");
                match gaussian {
                    Some(g) => {
                        let _ = write!(out, "{:016x}", g.to_bits());
                    }
                    None => out.push('-'),
                }
            }
            None => out.push_str("\trandom=?unmodeled"),
        }
        match self.health {
            Health::NotLiving => out.push_str("\thealth=-"),
            Health::Living(h) => {
                let _ = write!(out, "\thealth={:08x}", h.to_bits());
            }
            Health::Unmodeled => out.push_str("\thealth=?unmodeled"),
        }
        match self.uuid {
            Some(u) => {
                let _ = write!(out, "\tuuid={}", fmt_uuid(u));
            }
            None => out.push_str("\tuuid=?absent"),
        }
    }
}

fn joined_order(seq: JoinSeq, id: u32) -> (u8, u64, u32) {
    if seq.is_joined() {
        (1, seq.get(), id)
    } else {
        (2, 0, id)
    }
}

fn owner_dim(game: &ServerGame<'_>) -> Option<Dim> {
    let owner = game.server.serving.as_ref()?.owner;
    let host = game.server.lan.as_ref()?;
    let guest = host
        .hub
        .guests
        .iter()
        .find(|g| g.uuid == owner && g.placed)?;
    Some(Dim::from_id(&guest.dimension))
}

fn local_player_here(game: &ServerGame<'_>) -> bool {
    game.common
        .resident_entities
        .local_player
        .location()
        .is_some_and(|location| location.owner.dimension == game.common.dim.key())
}

fn passengers_of(game: &ServerGame<'_>, id: u32) -> Vec<u32> {
    game.seated_passengers(id)
        .into_iter()
        .map(|(_, rider, _)| rider)
        .collect()
}

fn passive_uuid(
    game: &ServerGame<'_>,
    id: u32,
    target: Option<Target>,
    live: Option<[i32; 4]>,
) -> Option<u128> {
    let words = live
        .or_else(|| game.common.persist.uuid_of(id))
        .or_else(|| {
            let Tag::IntArray(words) = game.saved_entity_compound(target?)?.get("UUID")? else {
                return None;
            };
            words.as_slice().try_into().ok()
        })?;
    Some(ints_uuid(words))
}

fn random_pair(r: &LegacyRandom) -> (u64, Option<f64>) {
    (r.state(), r.cached_gaussian())
}

fn is_known_nonliving_other(kind: &str) -> bool {
    matches!(
        kind,
        "minecraft:leash_knot"
            | "minecraft:eye_of_ender"
            | "minecraft:block_display"
            | "minecraft:item_display"
            | "minecraft:text_display"
            | "minecraft:marker"
            | "minecraft:interaction"
    )
}

pub(in crate::game) fn thrown_potion_type(item: &str) -> &'static str {
    match item {
        "minecraft:lingering_potion" => "minecraft:lingering_potion",
        "minecraft:experience_bottle" => "minecraft:experience_bottle",
        _ => "minecraft:splash_potion",
    }
}

fn collect_members(game: &ServerGame<'_>) -> Vec<Member> {
    let ticking_area = game.entity_ticking_area();
    let in_ticking = |pos: Vec3| ticking_area(pos.x.floor() as i32, pos.z.floor() as i32);
    let removals = &game.common.persist.removals;
    let mut members = Vec::new();

    // Players (always ticking; placed before joined entities in membership order).
    let mut local_player_id = None;
    if let Some(p) = game
        .common
        .player
        .as_ref()
        .filter(|_| local_player_here(game))
    {
        let id = p.entity_id as u32;
        local_player_id = Some(id);
        members.push(Member {
            order: (0, 0, id),
            id,
            ty: "minecraft:player".to_owned(),
            pos: p.position,
            motion: Some(p.velocity),
            rot: Some([game.common.yaw, game.common.pitch]),
            ground: Some(p.ground.on_ground),
            tick_count: Some(p.health.tick_count),
            fall_distance: Some(p.fall_distance),
            fire: Some(p.fire_ticks),
            air: Some(p.air),
            invulnerable_time: Some(p.health.invulnerable_time),
            no_gravity: None,
            vehicle: game.common.riding,
            passengers: passengers_of(game, id),
            random: Some(random_pair(&p.random)),
            health: Health::Living(p.health.health()),
            uuid: game.saved_player_uuid().map(ints_uuid),
        });
    }
    let dim_id = game.common.dim.id();
    if let Some(host) = game.server.lan.as_ref() {
        for g in host
            .hub
            .guests
            .iter()
            .filter(|g| g.placed && g.dimension == dim_id)
        {
            let id = g.entity_id as u32;
            if local_player_id == Some(id) {
                continue;
            }
            let motion_ref = g.motion.as_deref();
            let loaded = g.loaded_body;
            members.push(Member {
                order: (0, 0, id),
                id,
                ty: "minecraft:player".to_owned(),
                pos: Vec3::new(g.position[0], g.position[1], g.position[2]),
                motion: motion_ref.map(|m| m.velocity).or_else(|| {
                    loaded.map(|b| Vec3::new(b.velocity[0], b.velocity[1], b.velocity[2]))
                }),
                rot: Some([g.yaw, g.pitch]),
                ground: Some(motion_ref.map_or(g.on_ground, |m| m.ground.on_ground)),
                tick_count: motion_ref
                    .map(|m| m.health.tick_count)
                    .or(g.carried_tick_count),
                fall_distance: motion_ref
                    .map(|m| m.fall_distance)
                    .or_else(|| loaded.map(|b| b.fall_distance)),
                fire: motion_ref
                    .map(|m| m.fire_ticks)
                    .or_else(|| loaded.map(|b| b.fire_ticks)),
                air: motion_ref.map(|m| m.air).or_else(|| loaded.map(|b| b.air)),
                invulnerable_time: motion_ref
                    .map(|m| m.health.invulnerable_time)
                    .or_else(|| loaded.map(|b| b.invulnerable_time)),
                no_gravity: None,
                vehicle: g.vehicle,
                passengers: passengers_of(game, id),
                random: motion_ref.map(|m| random_pair(&m.random)),
                health: Health::Living(motion_ref.map_or(g.health, |m| m.health.health())),
                uuid: Some(g.uuid),
            });
        }
    }

    // 1. Mobs
    for e in game.common.mobs.iter() {
        if e.removed || removals.contains(&Target::Mob(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: mob_kind(e),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.head_pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.age as i32),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: Some(e.air),
            invulnerable_time: Some(e.invulnerable_time),
            no_gravity: Some(e.no_gravity),
            vehicle: e.extra.mount,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::Living(e.health),
            uuid: passive_uuid(game, e.id, Some(Target::Mob(e.id)), None),
        });
    }

    // 2. Dropped items
    for e in game.common.dropped.iter() {
        if removals.contains(&Target::Item(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:item".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.ticks as i32),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Item(e.id)), None),
        });
    }

    // 3. Boats
    for e in game.common.boats.iter() {
        if removals.contains(&Target::Boat(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: e.native_type().into_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.age),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Boat(e.id)), None),
        });
    }

    // 4. Minecarts
    for e in game.common.carts.iter() {
        if removals.contains(&Target::Cart(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: e.kind.id().to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.tick_count),
            fall_distance: Some(e.fall_distance),
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Cart(e.id)), None),
        });
    }

    // 5. Armor stands
    for e in game.common.armor_stands.iter() {
        if removals.contains(&Target::Stand(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:armor_stand".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.tick_count),
            fall_distance: Some(e.enchantment_fall_distance),
            fire: Some(e.fire_ticks),
            air: Some(e.air),
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: e.vehicle,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::Living(e.health),
            uuid: passive_uuid(game, e.id, Some(Target::Stand(e.id)), None),
        });
    }

    // 6. Primed TNT
    for e in game.common.tnts.iter() {
        if removals.contains(&Target::Tnt(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:tnt".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.ticks as i32),
            fall_distance: Some(e.fall_distance),
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Tnt(e.id)), None),
        });
    }

    // 7. Falling blocks
    for e in game.common.falling.iter() {
        if removals.contains(&Target::Falling(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:falling_block".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.ground.on_ground),
            tick_count: None,
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(
                game,
                e.id,
                Some(Target::Falling(e.id)),
                e.saved.as_ref().map(|s| s.0),
            ),
        });
    }

    // 8. Experience orbs
    for e in game.common.orbs.iter() {
        if removals.contains(&Target::Orb(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:experience_orb".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.ticks as i32),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: Some(e.air),
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Orb(e.id)), None),
        });
    }

    // 9. Projectiles
    for e in game.common.creatures.projectiles.iter() {
        if removals.contains(&Target::Projectile(e.id)) || !in_ticking(e.position) {
            continue;
        }
        let fire = e.arrow.as_ref().map_or(e.fire_ticks, |a| a.fire_ticks);
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: namespaced(e.entity_type()),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.age as i32),
            fall_distance: Some(e.fall_distance()),
            fire: Some(fire),
            air: Some(e.air_supply),
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random.0)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Projectile(e.id)), e.uuid),
        });
    }

    // 10. Firework rockets
    for e in game.common.rockets.iter() {
        if removals.contains(&Target::Rocket(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:firework_rocket".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.ground.on_ground),
            tick_count: None,
            fall_distance: Some(e.fall_distance),
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Rocket(e.id)), None),
        });
    }

    // 11. Thrown tridents
    for e in game.common.tridents.iter() {
        if removals.contains(&Target::Trident(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:trident".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.age as i32),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Trident(e.id)), None),
        });
    }

    // 12. General rows
    for e in game.common.resident_entities.general.rows.iter() {
        if removals.contains(&Target::General(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: namespaced(&e.native_type()),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.on_ground),
            tick_count: Some(e.tick_count),
            fall_distance: Some(e.fall_distance),
            fire: Some(e.base.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.base.no_gravity),
            vehicle: e.mount,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::Unmodeled,
            uuid: Some(ints_uuid(e.uuid)),
        });
    }

    // 13. Paintings
    for e in game.common.paintings.iter() {
        if removals.contains(&Target::Painting(e.id)) {
            continue;
        }
        let (lo, hi) = e.bounds();
        let pos = Vec3::new(
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        );
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:painting".to_owned(),
            pos,
            motion: None,
            rot: Some([f32::from(e.facing) * 90.0, 0.0]),
            ground: None,
            tick_count: Some(e.ticks as i32),
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Painting(e.id)), None),
        });
    }

    // 14. Item frames and glow item frames
    for e in game.common.item_frames.iter() {
        if removals.contains(&Target::Frame(e.id)) {
            continue;
        }
        let c = frames_common::center(e);
        let pos = Vec3::new(c[0], c[1], c[2]);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: e.kind().to_owned(),
            pos,
            motion: None,
            rot: Some(e.entity_rotation()),
            ground: None,
            tick_count: Some(e.ticks as i32),
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Frame(e.id)), None),
        });
    }

    // 15. Cushions
    for e in game.common.cushions.iter() {
        if removals.contains(&Target::Cushion(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:cushion".to_owned(),
            pos: e.position,
            motion: None,
            rot: None,
            ground: None,
            tick_count: None,
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Cushion(e.id)), None),
        });
    }

    // 16. Natural leash knots
    for e in game.common.resident_entities.knots.iter() {
        if removals.contains(&Target::Knot(e.id)) {
            continue;
        }
        let pos = leash::knot_state(e.fence).position;
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:leash_knot".to_owned(),
            pos,
            motion: None,
            rot: None,
            ground: None,
            tick_count: None,
            fall_distance: None,
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Knot(e.id)), None),
        });
    }

    // 17. Other entities (displays, markers, interactions, explicit knots, summoned eyes, ...)
    for e in game.server.others.0.iter() {
        if removals.contains(&Target::Other(e.id)) || !in_ticking(e.position) {
            continue;
        }
        let ty = namespaced(&e.kind);
        let health = if is_known_nonliving_other(&ty) {
            Health::NotLiving
        } else {
            Health::Unmodeled
        };
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty,
            pos: e.position,
            motion: e.eye.map(|eye| eye.motion),
            rot: Some([e.yaw, e.pitch]),
            ground: None,
            tick_count: None,
            fall_distance: None,
            fire: e.eye.map(|eye| eye.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health,
            uuid: passive_uuid(game, e.id, Some(Target::Other(e.id)), None),
        });
    }

    // 18. Evoker fangs
    for e in game.common.fangs.list.iter() {
        if removals.contains(&Target::Fangs(e.id)) {
            continue;
        }
        let pos = Vec3::new(e.pos[0], e.pos[1], e.pos[2]);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:evoker_fangs".to_owned(),
            pos,
            motion: Some(e.motion),
            rot: None,
            ground: None,
            tick_count: None,
            fall_distance: None,
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Fangs(e.id)), None),
        });
    }

    // 19. Lightning bolts
    for e in game.common.bolts.iter() {
        if removals.contains(&Target::Bolt(e.id)) {
            continue;
        }
        let pos = Vec3::new(e.at[0], e.at[1], e.at[2]);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:lightning_bolt".to_owned(),
            pos,
            motion: Some(e.velocity),
            rot: Some(e.rotation),
            ground: Some(e.on_ground),
            tick_count: None,
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: Some(e.air),
            invulnerable_time: Some(e.invulnerable_time),
            no_gravity: Some(e.no_gravity),
            vehicle: e.mount,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Bolt(e.id)), None),
        });
    }

    // 20. Ominous item spawners
    for e in game.common.ominous_spawners.iter() {
        if removals.contains(&Target::Spawner(e.id)) {
            continue;
        }
        let pos = Vec3::new(e.position[0], e.position[1], e.position[2]);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:ominous_item_spawner".to_owned(),
            pos,
            motion: None,
            rot: None,
            ground: None,
            tick_count: Some(e.tick_count as i32),
            fall_distance: None,
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Spawner(e.id)), Some(e.uuid)),
        });
    }

    // 21. End crystals
    for e in game.common.crystals.iter() {
        if removals.contains(&Target::Crystal(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:end_crystal".to_owned(),
            pos: e.position,
            motion: None,
            rot: Some(e.rotation),
            ground: Some(e.fall.0),
            tick_count: None,
            fall_distance: Some(e.fall.1),
            fire: Some(e.fire),
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Crystal(e.id)), None),
        });
    }

    // 22. Ender dragon (parts are children updated by the dragon body, not tick-list members).
    for e in game.common.dragon.iter() {
        if removals.contains(&Target::Dragon(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:ender_dragon".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: None,
            tick_count: Some(e.ticks),
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::Living(e.health),
            uuid: passive_uuid(game, e.id, Some(Target::Dragon(e.id)), None),
        });
    }

    // 23. Area effect clouds (dragon breath and lingering potions)
    for e in game.common.breath_clouds.iter() {
        if removals.contains(&Target::Cloud(e.id)) {
            continue;
        }
        let pos = Vec3::new(e.at[0], e.at[1], e.at[2]);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join_seq(), e.id),
            id: e.id,
            ty: "minecraft:area_effect_cloud".to_owned(),
            pos,
            motion: Some(Vec3::new(e.motion[0], e.motion[1], e.motion[2])),
            rot: None,
            ground: None,
            tick_count: None,
            fall_distance: Some(e.fall_distance),
            fire: Some(e.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Cloud(e.id)), None),
        });
    }

    // 24. Fishing hooks
    for (e, owner) in game.fishing_hooks_here() {
        if owner == Target::Player && !local_player_here(game) {
            continue;
        }
        if removals.contains(&Target::Hook(e.id)) || !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(e.join, e.id),
            id: e.id,
            ty: "minecraft:fishing_bobber".to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: Some([e.yaw, e.pitch]),
            ground: Some(e.ground.on_ground),
            tick_count: Some(e.tick_count),
            fall_distance: None,
            fire: Some(e.base.fire_ticks),
            air: None,
            invulnerable_time: None,
            no_gravity: Some(e.base.no_gravity),
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, Some(Target::Hook(e.id)), Some(e.uuid)),
        });
    }

    // 25. Thrown potions (unjoined)
    for e in game.common.potions.thrown.iter() {
        if !in_ticking(e.position) {
            continue;
        }
        members.push(Member {
            order: joined_order(JoinSeq::UNJOINED, e.id),
            id: e.id,
            ty: thrown_potion_type(&e.stack.item).to_owned(),
            pos: e.position,
            motion: Some(e.velocity),
            rot: None,
            ground: None,
            tick_count: Some(e.age as i32),
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: Some(random_pair(&e.random)),
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, None, None),
        });
    }

    // 26. Thrown eyes of ender (unjoined)
    for e in game.common.end.eyes.iter() {
        let pos = Vec3::new(e.position.x, e.position.y, e.position.z);
        if !in_ticking(pos) {
            continue;
        }
        members.push(Member {
            order: joined_order(JoinSeq::UNJOINED, e.id),
            id: e.id,
            ty: "minecraft:eye_of_ender".to_owned(),
            pos,
            motion: Some(Vec3::new(e.velocity.x, e.velocity.y, e.velocity.z)),
            rot: None,
            ground: None,
            tick_count: None,
            fall_distance: None,
            fire: None,
            air: None,
            invulnerable_time: None,
            no_gravity: None,
            vehicle: None,
            passengers: passengers_of(game, e.id),
            random: None,
            health: Health::NotLiving,
            uuid: passive_uuid(game, e.id, None, None),
        });
    }

    members.sort_by_key(|m| m.order);
    members
}

fn namespaced(key: &str) -> String {
    if key.contains(':') {
        key.to_owned()
    } else {
        format!("minecraft:{key}")
    }
}

fn fmt_random(r: &LegacyRandom) -> String {
    match r.cached_gaussian() {
        Some(g) => format!("{:012x},{:016x}", r.state(), g.to_bits()),
        None => format!("{:012x},-", r.state()),
    }
}

fn fmt_uuid(u: u128) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (u >> 96) as u32,
        ((u >> 80) & 0xffff) as u16,
        ((u >> 64) & 0xffff) as u16,
        ((u >> 48) & 0xffff) as u16,
        (u & 0xffff_ffff_ffff) as u64,
    )
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9 + 63) & !63);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(out, "{word:08x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
