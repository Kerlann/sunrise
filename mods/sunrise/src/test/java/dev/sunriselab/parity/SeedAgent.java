package dev.sunriselab.parity;

import java.lang.classfile.ClassFile;
import java.lang.classfile.CodeModel;
import java.lang.classfile.instruction.InvokeInstruction;
import java.lang.classfile.MethodModel;
import java.lang.constant.ClassDesc;
import java.lang.constant.ConstantDescs;
import java.lang.constant.MethodTypeDesc;
import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;

// Pins the source of new random seeds for parity oracles.
public final class SeedAgent {
    private static final String SERVER_LEVEL = "net/minecraft/server/level/ServerLevel";
    private static final String ENTITY_CALLBACKS = SERVER_LEVEL + "$EntityCallbacks";
    private static final ClassDesc ENTITY = ClassDesc.of("net.minecraft.world.entity.Entity");
    private static final ClassDesc RECORDER = ClassDesc.of("dev.sunriselab.parity.StateTranscript");
    private static final MethodTypeDesc ONE_ENTITY = MethodTypeDesc.of(ConstantDescs.CD_void, ENTITY);
    private static final MethodTypeDesc TWO_ENTITIES = MethodTypeDesc.of(ConstantDescs.CD_void, ENTITY, ENTITY);

    private static final String TARGET = "net/minecraft/world/level/levelgen/RandomSupport";
    // RandomSupport.SEED_UNIQUIFIER's start and step.
    private static final ThreadLocal<long[]> UNIQUIFIER = ThreadLocal.withInitial(() -> new long[] {8682522807148012L});
    private static final long STEP = 1181783497276652981L;
    private static volatile boolean installed;

    private static boolean transcriptReady;

    public static void premain(String options, Instrumentation instrumentation) throws ClassNotFoundException {
        boolean transcript = System.getProperty("sunrise.stateTranscript") != null;
        boolean probes = transcript && !"no-state-transcript-probes".equals(options);
        ClassLoader oracleLoader = SeedAgent.class.getClassLoader();
        boolean[] probeClasses = {false, false};
        instrumentation.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(ClassLoader loader, String name, Class<?> redefined, ProtectionDomain domain, byte[] bytes) {
                try {
                    if (TARGET.equals(name)) return rewrite(bytes);
                    if (probes && loader == oracleLoader && SERVER_LEVEL.equals(name)) {
                        byte[] rewritten = rewriteProbes(bytes, Probe.ROOT, Probe.PASSENGER);
                        probeClasses[0] = true;
                        return rewritten;
                    }
                    if (probes && loader == oracleLoader && ENTITY_CALLBACKS.equals(name)) {
                        byte[] rewritten = rewriteProbes(bytes, Probe.DEPARTURE);
                        probeClasses[1] = true;
                        return rewritten;
                    }
                    return null;
                } catch (Throwable error) {
                    // The JVM ignores a transformer's exceptions, so halt before running with missing probes.
                    error.printStackTrace();
                    Runtime.getRuntime().halt(3);
                    return null;
                }
            }
        });
        if (probes) {
            Class.forName(SERVER_LEVEL.replace('/', '.'), false, oracleLoader);
            Class.forName(ENTITY_CALLBACKS.replace('/', '.'), false, oracleLoader);
            if (!probeClasses[0] || !probeClasses[1]) {
                throw new IllegalStateException("Transcript probe classes were not rewritten");
            }
        }
        transcriptReady = transcript;
    }

    public static void requireTranscriptAgent() {
        if (!transcriptReady) {
            throw new IllegalStateException("State transcript needs -javaagent and -Dsunrise.stateTranscript");
        }
    }

    private enum Probe {
        ROOT("tickNonPassenger", ONE_ENTITY, EntityCall.ENTITY, "recordRoot"),
        PASSENGER("tickPassenger", TWO_ENTITIES, EntityCall.ENTITY, "recordPassenger"),
        DEPARTURE("onTickingEnd", ONE_ENTITY, EntityCall.TICK_LIST, "recordDeparture");

        final String method;
        final MethodTypeDesc signature;
        final EntityCall call;
        final String recorder;

        Probe(String method, MethodTypeDesc signature, EntityCall call, String recorder) {
            this.method = method;
            this.signature = signature;
            this.call = call;
            this.recorder = recorder;
        }

        boolean matches(MethodModel model) {
            return model.methodName().equalsString(method) && model.methodType().equalsString(signature.descriptorString());
        }
    }

    private enum EntityCall {
        ENTITY("net/minecraft/world/entity/Entity", "commonTick", "()V"),
        TICK_LIST("net/minecraft/world/level/entity/EntityTickList", "remove", ONE_ENTITY.descriptorString());

        final String owner;
        final String name;
        final String descriptor;

        EntityCall(String owner, String name, String descriptor) {
            this.owner = owner;
            this.name = name;
            this.descriptor = descriptor;
        }

        boolean matches(InvokeInstruction instruction) {
            return instruction.owner().name().equalsString(owner) && instruction.name().equalsString(name)
                && instruction.type().equalsString(descriptor);
        }
    }

    private static byte[] rewriteProbes(byte[] bytes, Probe... probes) {
        ClassFile files = ClassFile.of();
        int[] inserted = new int[probes.length];
        byte[] rewritten = files.transformClass(files.parse(bytes), (builder, element) -> {
            if (element instanceof MethodModel method) {
                for (int i = 0; i < probes.length; i++) {
                    Probe probe = probes[i];
                    if (!probe.matches(method)) continue;
                    int index = i;
                    builder.transformMethod(method, (target, part) -> {
                        if (part instanceof CodeModel code) {
                            target.transformCode(code, (body, instruction) -> {
                                if (instruction instanceof InvokeInstruction invoke && probe.call.matches(invoke)) {
                                    body.aload(1);
                                    if (probe == Probe.PASSENGER) body.aload(2);
                                    body.invokestatic(RECORDER, probe.recorder, probe.signature);
                                    inserted[index]++;
                                }
                                body.with(instruction);
                            });
                        } else {
                            target.with(part);
                        }
                    });
                    return;
                }
            }
            builder.with(element);
        });
        for (int i = 0; i < probes.length; i++) {
            if (inserted[i] != 1) {
                throw new IllegalStateException("Expected one transcript probe in " + probes[i].method + ", inserted " + inserted[i]);
            }
        }
        return rewritten;
    }

    private static byte[] rewrite(byte[] bytes) {
        ClassFile files = ClassFile.of();
        boolean[] found = {false};
        byte[] rewritten = files.transformClass(files.parse(bytes), (builder, element) -> {
            if (element instanceof MethodModel method && method.methodName().equalsString("generateUniqueSeed")
                    && method.methodType().equalsString("()J")) {
                found[0] = true;
                builder.withMethodBody(method.methodName(), method.methodType(), method.flags().flagsMask(), code -> code
                    .invokestatic(ClassDesc.of(SeedAgent.class.getName()), "nextSeed", MethodTypeDesc.of(ConstantDescs.CD_long))
                    .lreturn());
            } else {
                builder.with(element);
            }
        });
        if (!found[0]) {
            throw new IllegalStateException("RandomSupport has no generateUniqueSeed()J");
        }
        installed = true;
        return rewritten;
    }

    public static long nextSeed() {
        long[] uniquifier = UNIQUIFIER.get();
        return uniquifier[0] *= STEP;
    }

    public static void setUniquifier(long value) {
        if (!installed) {
            throw new IllegalStateException("RandomSupport.generateUniqueSeed was not rewritten (no -javaagent?)");
        }
        UNIQUIFIER.get()[0] = value;
    }
}
