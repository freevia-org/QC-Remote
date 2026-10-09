package org.freevia.qcremote;

import java.net.URI;
import java.util.Locale;

/** The relay hosts this app will hand a device credential to.
 *
 * The endpoint arrives from JavaScript, and the WebView is not an
 * authorization boundary: anything that can call the plugin could otherwise
 * point pairing, and with it the long-lived device credential and every
 * remote command, at a host of its choosing. The native side therefore
 * accepts only the production relay, plus loopback in debug builds for local
 * relay development. Every form must still be a bare HTTPS origin, because the
 * relay socket is derived from it by swapping the scheme to wss. */
final class RelayEndpointPolicy {
    static final String PRODUCTION_HOST = "qcrc.freevia.org";

    private RelayEndpointPolicy() {}

    /** The canonical endpoint, or null when it is not a permitted relay. */
    static String normalize(String raw, boolean allowLoopback) {
        String value = raw == null ? "" : raw.trim().replaceAll("/+$", "");
        final URI uri;
        try { uri = URI.create(value); } catch (Exception ignored) { return null; }
        if (!"https".equalsIgnoreCase(uri.getScheme()) || uri.getHost() == null || uri.getHost().isEmpty()
            || uri.getRawUserInfo() != null || uri.getRawQuery() != null || uri.getRawFragment() != null
            || uri.getRawPath() != null && !uri.getRawPath().isEmpty() && !"/".equals(uri.getRawPath())) return null;
        String host = uri.getHost().toLowerCase(Locale.ROOT);
        if (PRODUCTION_HOST.equals(host)) {
            if (uri.getPort() != -1 && uri.getPort() != 443) return null;
            return "https://" + PRODUCTION_HOST;
        }
        if (allowLoopback && isLoopback(host)) return value;
        return null;
    }

    static boolean isLoopback(String host) {
        return "localhost".equals(host) || "127.0.0.1".equals(host) || "[::1]".equals(host) || "::1".equals(host);
    }
}
