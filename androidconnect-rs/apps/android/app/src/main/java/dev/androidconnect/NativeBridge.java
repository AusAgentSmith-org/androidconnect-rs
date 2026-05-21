package dev.androidconnect;

import android.content.Context;
import android.os.Build;

import org.json.JSONException;
import org.json.JSONObject;

public final class NativeBridge {
    private static final boolean AVAILABLE;

    static {
        boolean loaded = false;
        try {
            System.loadLibrary("androidconnect_android_native");
            loaded = true;
        } catch (UnsatisfiedLinkError ignored) {
            loaded = false;
        }
        AVAILABLE = loaded;
    }

    private NativeBridge() {
    }

    public static boolean isAvailable() {
        return AVAILABLE;
    }

    public static boolean connect(Context context, String host, int port, String pairingCode) {
        if (!AVAILABLE) {
            return false;
        }
        return nativeConnect(host, port, buildDeviceName(context), pairingCode);
    }

    public static void disconnect() {
        if (AVAILABLE) {
            nativeDisconnect();
        }
    }

    public static long startSession(Context context) {
        if (!AVAILABLE) {
            return 0;
        }
        return nativeStartSession(buildSessionConfig(context));
    }

    public static void stopSession() {
        if (AVAILABLE) {
            nativeStopSession();
        }
    }

    public static boolean pushVideoFormat(
            int width,
            int height,
            int dpi,
            int frameRate,
            int rotationDegrees
    ) {
        return AVAILABLE && nativePushVideoFormat(width, height, dpi, frameRate, rotationDegrees);
    }

    public static boolean pushVideoFrame(byte[] frame, long presentationTimeUs, int flags) {
        return AVAILABLE && nativePushVideoFrame(frame, presentationTimeUs, flags);
    }

    public static String statsJson() {
        if (!AVAILABLE) {
            return "{\"running\":false,\"native_loaded\":false}";
        }
        return nativeStatsJson();
    }

    private static String buildSessionConfig(Context context) {
        JSONObject json = new JSONObject();
        try {
            json.put("package", context.getPackageName());
            json.put("device", Build.MANUFACTURER + " " + Build.MODEL);
            json.put("sdk", Build.VERSION.SDK_INT);
        } catch (JSONException ignored) {
            return "{}";
        }
        return json.toString();
    }

    private static String buildDeviceName(Context context) {
        return Build.MANUFACTURER + " " + Build.MODEL;
    }

    private static native boolean nativeConnect(
            String host,
            int port,
            String deviceName,
            String pairingCode
    );
    private static native void nativeDisconnect();
    private static native long nativeStartSession(String configJson);
    private static native void nativeStopSession();
    private static native boolean nativePushVideoFormat(
            int width,
            int height,
            int dpi,
            int frameRate,
            int rotationDegrees
    );
    private static native boolean nativePushVideoFrame(
            byte[] frame,
            long presentationTimeUs,
            int flags
    );
    private static native String nativeStatsJson();
}
