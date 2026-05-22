package dev.androidconnect;

import android.content.Context;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.Set;
import java.util.concurrent.CopyOnWriteArraySet;

public final class NativeBridge {
    private static final boolean AVAILABLE;
    private static final Handler MAIN_HANDLER = new Handler(Looper.getMainLooper());
    private static final Set<StatusListener> STATUS_LISTENERS = new CopyOnWriteArraySet<>();

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

    public interface StatusListener {
        void onNativeStatusChanged();
    }

    public static boolean isAvailable() {
        return AVAILABLE;
    }

    public static void addStatusListener(StatusListener listener) {
        if (listener != null) {
            STATUS_LISTENERS.add(listener);
        }
    }

    public static void removeStatusListener(StatusListener listener) {
        if (listener != null) {
            STATUS_LISTENERS.remove(listener);
        }
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

    static void onNativeStatusChanged() {
        if (STATUS_LISTENERS.isEmpty()) {
            return;
        }
        MAIN_HANDLER.post(() -> {
            for (StatusListener listener : STATUS_LISTENERS) {
                listener.onNativeStatusChanged();
            }
        });
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
