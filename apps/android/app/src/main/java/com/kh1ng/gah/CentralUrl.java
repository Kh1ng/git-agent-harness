package com.kh1ng.gah;

import java.net.URI;
import java.net.URISyntaxException;
import java.util.Locale;

final class CentralUrl {
    private CentralUrl() {}

    static String normalize(String value) {
        if (value == null || !value.equals(value.trim())) throw new IllegalArgumentException("Remove spaces around the address.");
        try {
            URI uri = new URI(value);
            String scheme = uri.getScheme() == null ? "" : uri.getScheme().toLowerCase(Locale.ROOT);
            if (!(scheme.equals("http") || scheme.equals("https")) || uri.getHost() == null || uri.getRawUserInfo() != null
                    || uri.getRawQuery() != null || uri.getRawFragment() != null || uri.getPort() == 0 || uri.getPort() > 65535) {
                throw new IllegalArgumentException("Use an HTTP or HTTPS server address without credentials, a query, or a fragment.");
            }
            String path = uri.getRawPath();
            if (path == null || path.isEmpty()) path = "/";
            return new URI(scheme, null, uri.getHost().toLowerCase(Locale.ROOT), uri.getPort(), path, null, null).toString();
        } catch (URISyntaxException error) {
            throw new IllegalArgumentException("Enter a valid central server address.");
        }
    }

    static boolean sameOrigin(String trusted, URI candidate) {
        URI base = URI.create(trusted);
        return candidate != null && base.getScheme().equalsIgnoreCase(candidate.getScheme())
                && base.getHost().equalsIgnoreCase(candidate.getHost()) && effectivePort(base) == effectivePort(candidate);
    }

    private static int effectivePort(URI uri) {
        if (uri.getPort() != -1) return uri.getPort();
        return "https".equalsIgnoreCase(uri.getScheme()) ? 443 : 80;
    }
}
