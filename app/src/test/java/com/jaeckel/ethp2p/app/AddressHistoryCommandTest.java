package com.jaeckel.ethp2p.app;

import io.myotis.api.ChainHandle;
import io.myotis.api.VerifiedReads;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Proxy;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The IPC commands of the address history (#899): their JSON reaches the engine whole; and the daemon's start. */
class AddressHistoryCommandTest {

    /** A handle whose reads() records the address history calls and answers {@code answer}. */
    private static CommandHandler handlerRecording(AtomicReference<String> seen, String answer) {
        VerifiedReads reads = (VerifiedReads) Proxy.newProxyInstance(
                VerifiedReads.class.getClassLoader(), new Class<?>[] {VerifiedReads.class},
                (proxy, method, args) -> switch (method.getName()) {
                    case "addressHistoryJson", "verifyAddressHistoryJson" -> {
                        seen.set(method.getName() + " " + args[0] + " " + args[1]);
                        yield answer;
                    }
                    default -> throw new UnsupportedOperationException(method.getName());
                });
        ChainHandle handle = (ChainHandle) Proxy.newProxyInstance(
                ChainHandle.class.getClassLoader(), new Class<?>[] {ChainHandle.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("reads")) return reads;
                    throw new UnsupportedOperationException(method.getName());
                });
        return new CommandHandler(handle, new CountDownLatch(1), null);
    }

    @Test
    void optionsAndEntriesReachTheEngineAsTheyCame() {
        AtomicReference<String> seen = new AtomicReference<>();
        CommandHandler h = handlerRecording(seen, "{\"entries\":[]}");
        String addr = "0x" + "11".repeat(20);
        // Strings inside the options may hold brackets and escaped quotes.
        String opts = "{\"cursor\":\"3:900:2\",\"roles\":[\"from\"],\"note\":\"a } \\\" ]\"}";
        assertEquals("{\"entries\":[]}",
                h.handle("{\"cmd\":\"get-address-history\",\"address\":\"" + addr + "\",\"options\":" + opts + "}"));
        assertEquals("addressHistoryJson " + addr + " " + opts, seen.get());
        // No options: an empty object.
        h.handle("{\"cmd\":\"get-address-history\",\"address\":\"" + addr + "\"}");
        assertEquals("addressHistoryJson " + addr + " {}", seen.get());
        String entries = "[{\"block\":7,\"slot\":1,\"roles\":[\"from\"]}]";
        h.handle("{\"cmd\":\"verify-address-history\",\"address\":\"" + addr + "\",\"entries\":" + entries + "}");
        assertEquals("verifyAddressHistoryJson " + addr + " " + entries, seen.get());
    }

    @Test
    void aValueThatIsNoObjectOrArrayIsRefused() {
        assertThrows(IllegalArgumentException.class,
                () -> CommandHandler.extractRawJsonOr("{\"options\":5}", "options", "{}"));
        assertThrows(IllegalArgumentException.class,
                () -> CommandHandler.extractRawJsonOr("{\"options\":{\"a\":1", "options", "{}"));
        assertTrue(CommandHandler.extractRawJsonOr("{\"x\":1}", "options", "{}").equals("{}"));
    }

    /** The daemon's start names the importer it trusts on every chain, or on each chain. */
    @Test
    void theStartNamesEveryChainsImporter() {
        String a = "0xb11B5F07FEb04076c21092A64eb827f418087241";
        String b = "0xE7795B93916b361F1aF8610A16ef8E1190319335";
        assertEquals(java.util.List.of(
                        "trusting on chain 1 the index of importer " + a,
                        "trusting on chain 100 the index of importer " + b),
                Main.trustedImporters("{\"importers\": {\"100\": \"" + b + "\", \"1\": \"" + a + "\"},"
                        + " \"swarm\": {\"network\": \"mainnet\"}}"));
        assertEquals(java.util.List.of("trusting on every chain the index of importer " + a),
                Main.trustedImporters("{\"importer\":\"" + a + "\"}"));
        assertEquals(java.util.List.of("trusting no importer (every read refused)"),
                Main.trustedImporters("{\"importer\":null}"));
        assertEquals(java.util.List.of("trusting no importer (every read refused)"),
                Main.trustedImporters("{\"importers\":{}}"));
    }
}
