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
    private static volatile Context appContext;
    private static volatile boolean utilitiesPrimedForConnection;

    // Scale factors from video space → physical display space, used by input dispatch.
    static volatile float inputScaleX = 1.0f;
    static volatile float inputScaleY = 1.0f;

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
        rememberContext(context);
        utilitiesPrimedForConnection = false;
        boolean connected = nativeConnect(
                host,
                port,
                buildDeviceName(context),
                context.getFilesDir().getAbsolutePath(),
                pairingCode
        );
        if (!connected) {
            DeviceStateMonitor.stopForConnection();
        }
        return connected;
    }

    public static void disconnect() {
        utilitiesPrimedForConnection = false;
        SmsBridge.stopObserver();
        DeviceStateMonitor.stopForConnection();
        if (AVAILABLE) {
            nativeDisconnect();
        }
    }

    public static long startSession(Context context) {
        if (!AVAILABLE) {
            return 0;
        }
        rememberContext(context);
        DeviceStateMonitor.startForSession(context);
        return nativeStartSession(buildSessionConfig(context));
    }

    public static void refreshUtilities(Context context) {
        rememberContext(context);
        AndroidUtilityBridge.refreshAll(context);
    }

    public static void stopSession() {
        DeviceStateMonitor.stopForSession();
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

    public static boolean pushDeviceStatusJson(String json) {
        return AVAILABLE && nativePushDeviceStatus(json);
    }

    public static boolean pushMediaStatusJson(String json) {
        return AVAILABLE && nativePushMediaStatus(json);
    }

    public static boolean pushNotificationPostedJson(String json) {
        return AVAILABLE && nativePushNotificationPosted(json);
    }

    public static boolean pushNotificationRemoved(String notificationId) {
        return AVAILABLE && nativePushNotificationRemoved(notificationId);
    }

    public static boolean pushClipboardText(String text) {
        return AVAILABLE && nativePushClipboardText(text);
    }

    public static boolean pushSharedFile(String fileName, String mimeType, String path, long size) {
        return AVAILABLE && nativePushSharedFile(fileName, mimeType, path, size);
    }

    public static boolean pushSharedFileForRequest(
            String fileName,
            String mimeType,
            String path,
            long size,
            String requestedPath
    ) {
        return AVAILABLE && nativePushSharedFileForRequest(
                fileName,
                mimeType,
                path,
                size,
                requestedPath
        );
    }

    public static boolean pushPhotoAssetListJson(String json) {
        return AVAILABLE && nativePushPhotoAssetList(json);
    }

    public static boolean pushMessageThreadListJson(String json) {
        return AVAILABLE && nativePushMessageThreadList(json);
    }

    public static boolean pushMessageThreadDetailJson(String json) {
        return AVAILABLE && nativePushMessageThreadDetail(json);
    }

    public static boolean pushMessageEventJson(String json) {
        return AVAILABLE && nativePushMessageEvent(json);
    }

    public static boolean pushMessageSendResponseJson(String json) {
        return AVAILABLE && nativePushMessageSendResponse(json);
    }

    public static boolean pushCallStateJson(String json) {
        return AVAILABLE && nativePushCallState(json);
    }

    public static boolean pushRelayStatusJson(String json) {
        return AVAILABLE && nativePushRelayStatus(json);
    }

    static void onNativeStatusChanged() {
        maybePrimeUtilitiesAfterAuth();
        if (STATUS_LISTENERS.isEmpty()) {
            return;
        }
        MAIN_HANDLER.post(() -> {
            for (StatusListener listener : STATUS_LISTENERS) {
                listener.onNativeStatusChanged();
            }
        });
    }

    private static void maybePrimeUtilitiesAfterAuth() {
        if (!AVAILABLE) {
            return;
        }
        Context context = appContext;
        if (context == null) {
            return;
        }
        boolean connected;
        boolean authenticated;
        try {
            JSONObject stats = new JSONObject(nativeStatsJson());
            connected = stats.optBoolean("connected", false);
            authenticated = stats.optBoolean("input_authenticated", false);
        } catch (JSONException ignored) {
            return;
        }
        if (!connected || !authenticated) {
            utilitiesPrimedForConnection = false;
            if (!connected) {
                SmsBridge.stopObserver();
                DeviceStateMonitor.stopForConnection();
            }
            return;
        }
        if (utilitiesPrimedForConnection) {
            return;
        }
        utilitiesPrimedForConnection = true;
        MAIN_HANDLER.post(() -> {
            DeviceStateMonitor.startForConnection(context);
            AndroidUtilityBridge.refreshAll(context);
            SmsBridge.startObserver(context);
        });
    }

    static void onMediaControl(String action) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.applyMediaControl(context, action);
        }
    }

    static void onClipboardTextFromDesktop(String text) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.applyClipboardText(context, text);
        }
    }

    static void onAudioControl(String command) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.applyAudioControl(context, command);
        }
    }

    static void onAppWindowOpen(String packageName, String activityName) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.openApp(context, packageName);
        }
    }

    static void onCallAction(String action, String phoneNumber) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.applyCallAction(context, action, phoneNumber);
        }
    }

    static void onMessageSendRequest(String requestJson) {
        Context context = appContext;
        if (context != null) {
            SmsBridge.sendMessage(context, requestJson);
        }
    }

    static void onMessageThreadOpen(String openJson) {
        Context context = appContext;
        if (context != null) {
            SmsBridge.handleThreadOpen(context, openJson);
        }
    }

    static void onFileTransferRequest(String requestJson) {
        Context context = appContext;
        if (context != null) {
            AndroidUtilityBridge.handleFileTransferRequest(context, requestJson);
        }
    }

    static void onNotificationAction(String actionJson) {
        AndroidConnectNotificationService.performNotificationAction(actionJson);
    }

    static void onNotificationFilterUpdate(String filterJson) {
        AndroidConnectNotificationService.applyNotificationFilterUpdate(filterJson);
    }

    static void onPhotoAssetTransfer(String transferJson) {
        AndroidUtilityBridge.pushPhotosPermissionStatus();
    }

    static void onRelayOffer(String offerJson) {
        AndroidUtilityBridge.pushRelayDisabledStatus();
    }

    static void onClientRoleUpdate(String updateJson) {
        AndroidUtilityBridge.pushDeviceStatus(appContext);
    }

    private static void rememberContext(Context context) {
        if (context != null) {
            appContext = context.getApplicationContext();
            AndroidUtilityBridge.rememberContext(appContext);
        }
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
            String storageDir,
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
    private static native boolean nativePushDeviceStatus(String statusJson);
    private static native boolean nativePushMediaStatus(String statusJson);
    private static native boolean nativePushNotificationPosted(String notificationJson);
    private static native boolean nativePushNotificationRemoved(String notificationId);
    private static native boolean nativePushClipboardText(String text);
    private static native boolean nativePushSharedFile(
            String fileName,
            String mimeType,
            String path,
            long sizeBytes
    );
    private static native boolean nativePushSharedFileForRequest(
            String fileName,
            String mimeType,
            String path,
            long sizeBytes,
            String requestedPath
    );
    private static native boolean nativePushPhotoAssetList(String listJson);
    private static native boolean nativePushMessageThreadList(String listJson);
    private static native boolean nativePushMessageThreadDetail(String detailJson);
    private static native boolean nativePushMessageEvent(String eventJson);
    private static native boolean nativePushMessageSendResponse(String responseJson);
    private static native boolean nativePushCallState(String stateJson);
    private static native boolean nativePushRelayStatus(String statusJson);
    private static native String nativeStatsJson();
}
