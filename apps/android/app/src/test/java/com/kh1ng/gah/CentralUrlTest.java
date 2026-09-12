package com.kh1ng.gah;

import static org.junit.Assert.*;
import java.net.URI;
import org.junit.Test;

public final class CentralUrlTest {
    @Test public void acceptsTailnetHttpAndHttpsOrigins() {
        assertEquals("http://100.64.1.2:3774/", CentralUrl.normalize("http://100.64.1.2:3774"));
        assertEquals("https://central.example.ts.net/", CentralUrl.normalize("https://central.example.ts.net"));
    }

    @Test public void rejectsCredentialsQueriesFragmentsAndWhitespace() {
        for (String value : new String[] {"central.test", " https://central.test", "https://user@central.test", "https://central.test?q=1", "https://central.test/#x"}) {
            assertThrows(IllegalArgumentException.class, () -> CentralUrl.normalize(value));
        }
    }

    @Test public void navigationStaysInsideTheConfiguredOrigin() {
        String central = CentralUrl.normalize("https://central.test/base");
        assertTrue(CentralUrl.sameOrigin(central, URI.create("https://central.test/chat")));
        assertFalse(CentralUrl.sameOrigin(central, URI.create("https://evil.test/chat")));
        assertFalse(CentralUrl.sameOrigin(central, URI.create("http://central.test/chat")));
    }
}
