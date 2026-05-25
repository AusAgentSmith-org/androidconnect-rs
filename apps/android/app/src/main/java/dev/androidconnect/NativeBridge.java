package dev.androidconnect;

import android.Manifest;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;
import android.util.Log;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.Set;
import java.util.concurrent.CopyOnWriteArraySet;

public final class NativeBridge {
    private static final String TAG = "AndroidConnectNative";
    private static final String MIRROR_REQUEST_CHANNEL_ID = "mirror_requests";
    private static final int MIRROR_REQUEST_NOTIFICATION_ID = 18;
    private static final boolean AVAILABLE;
    private static final Handler MAIN_HANDLER = new Handler(Looper.getMainLooper());
    private static final Set<StatusListener> STATUS_LISTENERS = new CopyOnWriteArraySet<>();
    private static volatile Context appContext;
    private static volatile boolean utilitiesPrimedForConnection;

    // Wake + wifi locks held for the duration of an authenticated control connection
    // (screen-off drops the TCP socket on Samsung/Xiaomi without these).
    private static volatile PowerManager.WakeLock connectionWakeLock;
    private static volatile WifiManager.WifiLock connectionWifiLock;
    private static volatile boolean keepConnectionAwake = true;
    private static volatile String pendingMirrorRequestId;

    public interface MirrorRequestListener {
        void onMirrorRequested(String requestId);
    }
    private static volatile MirrorRequestListener mirrorRequestListener;

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
                getFileBrowserRoot(context).getAbsolutePath(),
                pairingCode
        );
        if (!connected) {
            DeviceStateMonitor.stopForConnection();
        }
        return connected;
    }

    /**
     * Returns the root directory used for the file browser.
     * Uses external storage when MANAGE_EXTERNAL_STORAGE (API 30+) or
     * READ_EXTERNAL_STORAGE (older) is granted; falls back to app-internal files dir.
     */
    public static java.io.File getFileBrowserRoot(Context context) {
        if (Build.VERSION.SDK_INT >= 30) {
            if (Environment.isExternalStorageManager()) {
                return Environment.getExternalStorageDirectory();
            }
        } else {
            if (context.checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE)
                    == PackageManager.PERMISSION_GRANTED) {
                return Environment.getExternalStorageDirectory();
            }
        }
        return context.getFilesDir();
    }

    public static String fileBrowserRootLabel(Context context) {
        java.io.File root = getFileBrowserRoot(context);
        if (root.equals(Environment.getExternalStorageDirectory())) {
            return "Full external storage";
        }
        return "App storage";
    }

    public static void setMirrorRequestListener(MirrorRequestListener listener) {
        mirrorRequestListener = listener;
        if (listener != null) {
            String requestId = pendingMirrorRequestId;
            pendingMirrorRequestId = null;
            if (requestId != null) {
                MAIN_HANDLER.post(() -> dispatchMirrorRequest(requestId));
            }
        }
    }

    public static void clearPendingMirrorRequest(String requestId) {
        if (requestId == null || requestId.equals(pendingMirrorRequestId)) {
            pendingMirrorRequestId = null;
        }
    }

    public static boolean isKeepConnectionAwakeEnabled() {
        return keepConnectionAwake;
    }

    public static boolean connectionLocksHeld() {
        return (connectionWakeLock != null && connectionWakeLock.isHeld())
                || (connectionWifiLock != null && connectionWifiLock.isHeld());
    }

    public static void setKeepConnectionAwakeEnabled(boolean enabled) {
        keepConnectionAwake = enabled;
        if (!enabled) {
            releaseConnectionLocks();
            return;
        }
        if (isConnectedAndAuthenticated()) {
            acquireConnectionLocks();
        }
    }

    public static void disconnect() {
        utilitiesPrimedForConnection = false;
        SmsBridge.stopObserver();
        DeviceStateMonitor.stopForConnection();
        releaseConnectionLocks();
        if (AVAILABLE) {
            nativeDisconnect();
        }
    }

    private static void acquireConnectionLocks() {
        if (!keepConnectionAwake) {
            return;
        }
        Context ctx = appContext;
        if (ctx == null) return;
        if (connectionWakeLock == null) {
            PowerManager power = (PowerManager) ctx.getSystemService(Context.POWER_SERVICE);
            if (power != null) {
                connectionWakeLock = power.newWakeLock(
                        PowerManager.PARTIAL_WAKE_LOCK, "AndroidConnect::Connection");
                connectionWakeLock.setReferenceCounted(false);
                connectionWakeLock.acquire();
                Log.i(TAG, "acquired connection wake lock");
            }
        }
        if (connectionWifiLock == null) {
            WifiManager wifi = (WifiManager) ctx.getApplicationContext()
                    .getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                int mode = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
                        ? WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                        : WifiManager.WIFI_MODE_FULL_HIGH_PERF;
                connectionWifiLock = wifi.createWifiLock(mode, "AndroidConnect::Connection");
                connectionWifiLock.setReferenceCounted(false);
                connectionWifiLock.acquire();
                Log.i(TAG, "acquired connection wifi lock (mode=" + mode + ")");
            }
        }
    }

    private static void releaseConnectionLocks() {
        if (connectionWakeLock != null) {
            if (connectionWakeLock.isHeld()) connectionWakeLock.release();
            connectionWakeLock = null;
            Log.i(TAG, "released connection wake lock");
        }
        if (connectionWifiLock != null) {
            if (connectionWifiLock.isHeld()) connectionWifiLock.release();
            connectionWifiLock = null;
            Log.i(TAG, "released connection wifi lock");
        }
    }

    private static boolean isConnectedAndAuthenticated() {
        if (!AVAILABLE) {
            return false;
        }
        try {
            JSONObject stats = new JSONObject(nativeStatsJson());
            return stats.optBoolean("connected", false)
                    && stats.optBoolean("input_authenticated", false);
        } catch (JSONException ignored) {
            return false;
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

    public static boolean pushStorageStatusJson(String json) {
        return AVAILABLE && nativePushStorageStatus(json);
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

    public static boolean pushMirrorRequestResult(
            String requestId,
            String state,
            String message
    ) {
        if (!AVAILABLE) {
            return false;
        }
        JSONObject json = new JSONObject();
        try {
            json.put("request_id", requestId == null ? "" : requestId);
            json.put("state", state == null ? "Unavailable" : state);
            json.put("message", message == null ? "" : message);
        } catch (JSONException ignored) {
            return false;
        }
        return nativePushMirrorRequestResult(json.toString());
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
                releaseConnectionLocks();
            }
            return;
        }
        if (utilitiesPrimedForConnection) {
            return;
        }
        utilitiesPrimedForConnection = true;
        acquireConnectionLocks();
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

    static void onMirrorRequest(String requestJson) {
        String requestId = "";
        try {
            JSONObject json = new JSONObject(requestJson == null ? "{}" : requestJson);
            requestId = json.optString("request_id", "");
        } catch (JSONException ignored) {
        }
        if (requestId.isEmpty()) {
            requestId = "mirror-" + System.currentTimeMillis();
        }
        String finalRequestId = requestId;
        MAIN_HANDLER.post(() -> dispatchMirrorRequest(finalRequestId));
    }

    private static void dispatchMirrorRequest(String requestId) {
        MirrorRequestListener l = mirrorRequestListener;
        if (l != null) {
            pushMirrorRequestResult(
                    requestId,
                    "PromptShown",
                    "Screen capture prompt shown on phone.");
            l.onMirrorRequested(requestId);
            return;
        }

        pendingMirrorRequestId = requestId;
        pushMirrorRequestResult(
                requestId,
                "Queued",
                "Open AndroidConnect on your phone to approve screen capture.");
        Context context = appContext;
        if (context != null) {
            showMirrorRequestNotification(context, requestId);
        } else {
            Log.w(TAG, "onMirrorRequest: no app context available");
        }
    }

    static void onRelayOffer(String offerJson) {
        AndroidUtilityBridge.pushRelayDisabledStatus();
    }

    static void onClientRoleUpdate(String updateJson) {
        AndroidUtilityBridge.pushDeviceStatus(appContext);
    }

    private static void showMirrorRequestNotification(Context context, String requestId) {
        NotificationManager manager =
                (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) {
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                && context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            Log.w(TAG, "mirror request notification skipped: POST_NOTIFICATIONS not granted");
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                    MIRROR_REQUEST_CHANNEL_ID,
                    "Mirror requests",
                    NotificationManager.IMPORTANCE_HIGH);
            manager.createNotificationChannel(channel);
        }

        Intent intent = new Intent(context, MainActivity.class);
        intent.setAction(MainActivity.ACTION_MIRROR_REQUEST);
        intent.putExtra(MainActivity.EXTRA_MIRROR_REQUEST_ID, requestId);
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            flags |= PendingIntent.FLAG_IMMUTABLE;
        }
        PendingIntent openApp = PendingIntent.getActivity(
                context,
                MIRROR_REQUEST_NOTIFICATION_ID,
                intent,
                flags);

        Notification.Builder builder = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
                ? new Notification.Builder(context, MIRROR_REQUEST_CHANNEL_ID)
                : new Notification.Builder(context);
        Notification notification = builder
                .setSmallIcon(android.R.drawable.presence_video_online)
                .setContentTitle("Screen mirroring requested")
                .setContentText("Open AndroidConnect to approve screen capture")
                .setContentIntent(openApp)
                .setAutoCancel(true)
                .build();
        try {
            manager.notify(MIRROR_REQUEST_NOTIFICATION_ID, notification);
        } catch (SecurityException error) {
            Log.w(TAG, "mirror request notification failed", error);
        }
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
    private static native boolean nativePushStorageStatus(String statusJson);
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
    private static native boolean nativePushMirrorRequestResult(String resultJson);
    private static native boolean nativePushCallState(String stateJson);
    private static native boolean nativePushRelayStatus(String statusJson);
    private static native String nativeStatsJson();
}
