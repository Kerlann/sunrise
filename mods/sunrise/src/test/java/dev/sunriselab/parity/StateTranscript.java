package dev.sunriselab.parity;

import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.DigestInputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.HexFormat;
import java.util.List;
import java.util.Locale;
import java.util.StringJoiner;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.players.PlayerList;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.entity.EntityTickList;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.MarsagliaPolarGaussian;
import net.minecraft.world.phys.Vec3;

public final class StateTranscript implements AutoCloseable {
    private static final HexFormat HEX = HexFormat.of();
    private static final ThreadLocal<StateTranscript> ACTIVE_TICK = new ThreadLocal<>();
    private static final Field TICK_LIST = field(ServerLevel.class, "entityTickList");
    private static final Field RAND_VALUE = field(Level.class, "randValue");
    private static final Field ENTITY_COUNTER = field(ServerLevel.class, "ENTITY_COUNTER");
    private static final Field ENTITY_RANDOM = field(Entity.class, "random");
    private static final Field SEED = field(LegacyRandomSource.class, "seed");
    private static final Field GAUSSIAN_SOURCE = field(LegacyRandomSource.class, "gaussianSource");
    private static final Field HAS_GAUSSIAN = field(MarsagliaPolarGaussian.class, "haveNextNextGaussian");
    private static final Field GAUSSIAN = field(MarsagliaPolarGaussian.class, "nextNextGaussian");

    private final BufferedWriter output;
    private final ServerLevel level;
    private final List<Turn> turns = new ArrayList<>();
    private final List<Departure> departures = new ArrayList<>();
    private long step;
    private int ordinal;
    private int points;
    private long rows;
    boolean enabled = true;

    private record Turn(int id, String type, String role, String vehicle) {}
    private record Departure(int id, String type, Entity.RemovalReason reason) {}

    StateTranscript(ServerLevel level, Path script, PlayerList players) throws IOException, ReflectiveOperationException {
        Class<?> agent = Class.forName("dev.sunriselab.parity.SeedAgent");
        agent.getMethod("requireTranscriptAgent").invoke(null);
        Path path = Path.of(System.getProperty("sunrise.stateTranscript"));
        String scriptHash;
        try (InputStream input = Files.newInputStream(script)) {
            scriptHash = sha256(input);
        }
        String fixtureHash = System.getProperty("sunrise.stateTranscriptFixtureSha256", sha256(InputStream.nullInputStream()));
        if (!fixtureHash.matches("[0-9a-f]{64}")) throw new IllegalArgumentException("Invalid transcript fixture SHA-256");
        String build = System.getProperty("sunrise.stateTranscriptBuild");
        if (build == null) build = defaultBuild(agent);
        if (build.isBlank() || build.chars().anyMatch(Character::isWhitespace))
            throw new IllegalArgumentException("Invalid transcript build identifier");
        this.level = level;
        output = Files.newBufferedWriter(path);
        output.write("H\tformat=sunrise-state-transcript\tversion=1\tengine=vanilla\tscript_sha256=" + scriptHash
            + "\tfixture_sha256=" + fixtureHash + "\tbuild=" + build + "\tgame_version=26.3"
            + "\tview_distance=" + players.getViewDistance() + "\tsimulation_distance=" + players.getSimulationDistance()
            + "\n");
    }

    private static String defaultBuild(Class<?> agent) throws IOException {
        StringJoiner build = new StringJoiner("-");
        for (Class<?> cls : List.of(ServerLevel.class, agent)) {
            var location = cls.getProtectionDomain().getCodeSource().getLocation();
            try (InputStream input = location.openStream()) {
                build.add(sha256(input));
            }
        }
        hashClasses(build, VanillaCommandExecOracle.class);
        hashClasses(build, StateTranscript.class);
        return build.toString();
    }

    private static void hashClasses(StringJoiner build, Class<?> cls) throws IOException {
        String resource = "/" + cls.getName().replace('.', '/') + ".class";
        try (InputStream input = cls.getResourceAsStream(resource)) {
            if (input == null) throw new IOException("Missing transcript build resource: " + resource);
            build.add(sha256(input));
        }
        Class<?>[] nested = cls.getDeclaredClasses();
        Arrays.sort(nested, Comparator.comparing(Class::getName));
        for (Class<?> child : nested) hashClasses(build, child);
    }

    private static String sha256(InputStream input) throws IOException {
        try {
            MessageDigest hash = MessageDigest.getInstance("SHA-256");
            new DigestInputStream(input, hash).transferTo(OutputStream.nullOutputStream());
            return HEX.formatHex(hash.digest());
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException(error);
        }
    }

    static void syncRandValue(ServerLevel level, int value) throws IllegalAccessException {
        RAND_VALUE.setInt(level, value);
    }

    static void syncPlayerSeed(Entity player, long seed) throws IllegalAccessException {
        ((RandomSource) ENTITY_RANDOM.get(player)).setSeed(seed);
    }

    private static Field field(Class<?> owner, String name) {
        try {
            Field result = owner.getDeclaredField(name);
            result.setAccessible(true);
            return result;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(error);
        }
    }

    void beginTick() {
        if (!enabled) return;
        step++;
        ordinal = 0;
        turns.clear();
        departures.clear();
        ACTIVE_TICK.set(this);
    }

    void endTick() {
        if (enabled) ACTIVE_TICK.remove();
    }

    public static void recordRoot(Entity entity) {
        recordTurn(entity, null, "root");
    }

    public static void recordPassenger(Entity vehicle, Entity entity) {
        recordTurn(entity, vehicle, "passenger");
    }

    private static void recordTurn(Entity entity, Entity vehicle, String role) {
        StateTranscript active = ACTIVE_TICK.get();
        if (active != null && entity.level() == active.level) {
            active.turns.add(new Turn(entity.getId(), type(entity), role, entityId(vehicle)));
        }
    }

    public static void recordDeparture(Entity entity) {
        StateTranscript active = ACTIVE_TICK.get();
        if (active != null && entity.level() == active.level) {
            active.departures.add(new Departure(entity.getId(), type(entity), entity.getRemovalReason()));
        }
    }

    void point(int op, boolean tick) throws IOException, IllegalAccessException {
        if (!enabled) return;
        List<Entity> members = new ArrayList<>();
        ((EntityTickList) TICK_LIST.get(level)).forEach(members::add);
        row("P\tstep=" + step + "\tphase=" + (tick ? "after_tick" : "after_op") + "\tord=" + ordinal++
            + "\top=" + op + "\tgame_time=" + level.getGameTime() + "\tdim=" + level.dimension().identifier());
        row("L\tlevel_random=" + random(level.getRandom()) + "\trand_value=" + RAND_VALUE.getInt(level)
            + "\tlast_entity_id=" + ((AtomicInteger) ENTITY_COUNTER.get(null)).get() + "\tmembers=" + members.size()
            + "\tday_time=" + level.getDefaultClockTime());
        for (int i = 0; i < members.size(); i++) member(i, members.get(i));
        if (tick) {
            for (int i = 0; i < turns.size(); i++) {
                Turn turn = turns.get(i);
                row("T\tn=" + i + "\tid=" + turn.id + "\ttype=" + turn.type + "\trole=" + turn.role + "\tvehicle=" + turn.vehicle);
            }
            for (int i = 0; i < departures.size(); i++) {
                Departure departure = departures.get(i);
                String reason = departure.reason == null ? "?" : departure.reason.name().toLowerCase(Locale.ROOT);
                row("D\tn=" + i + "\tid=" + departure.id + "\ttype=" + departure.type + "\treason=" + reason);
            }
        }
        points++;
    }

    private void member(int i, Entity entity) throws IOException, IllegalAccessException {
        StringJoiner passengers = new StringJoiner(",").setEmptyValue("-");
        for (Entity passenger : entity.getPassengers()) passengers.add(entityId(passenger));
        String health = entity instanceof LivingEntity living ? bits(living.getHealth()) : "-";
        row("M\ti=" + i + "\tid=" + entity.getId() + "\ttype=" + type(entity)
            + "\tpos=" + bits(entity.position()) + "\tmotion=" + bits(entity.getDeltaMovement())
            + "\trot=" + bits(entity.getYRot()) + "," + bits(entity.getXRot())
            + "\tground=" + (entity.onGround() ? 1 : 0) + "\ttick_count=" + entity.tickCount
            + "\tfall_distance=" + bits(entity.fallDistance) + "\tfire=" + entity.getRemainingFireTicks()
            + "\tair=" + entity.getAirSupply() + "\tinvulnerable_time=" + entity.getInvulnerableTime()
            + "\tno_gravity=" + (entity.isNoGravity() ? 1 : 0) + "\tvehicle=" + entityId(entity.getVehicle())
            + "\tpassengers=" + passengers + "\trandom=" + random((RandomSource) ENTITY_RANDOM.get(entity))
            + "\thealth=" + health + "\tuuid=" + entity.getUUID());
    }

    private static String type(Entity entity) {
        return BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString();
    }

    private static String entityId(Entity entity) {
        return entity == null ? "-" : Integer.toString(entity.getId());
    }

    private static String random(RandomSource random) throws IllegalAccessException {
        if (!(random instanceof LegacyRandomSource)) return "?rng:" + random.getClass().getName();
        Object gaussian = GAUSSIAN_SOURCE.get(random);
        String cache = HAS_GAUSSIAN.getBoolean(gaussian) ? bits(GAUSSIAN.getDouble(gaussian)) : "-";
        return HEX.toHexDigits(((AtomicLong) SEED.get(random)).get(), 12) + "," + cache;
    }

    private static String bits(float value) {
        return HEX.toHexDigits(Float.floatToRawIntBits(value));
    }

    private static String bits(double value) {
        return HEX.toHexDigits(Double.doubleToRawLongBits(value));
    }

    private static String bits(Vec3 vector) {
        return bits(vector.x) + "," + bits(vector.y) + "," + bits(vector.z);
    }

    private void row(String value) throws IOException {
        output.write(value);
        output.write('\n');
        rows++;
    }

    void complete() throws IOException {
        output.write("Z\tpoints=" + points + "\trows=" + rows + "\n");
    }

    @Override public void close() throws IOException {
        output.close();
    }
}
