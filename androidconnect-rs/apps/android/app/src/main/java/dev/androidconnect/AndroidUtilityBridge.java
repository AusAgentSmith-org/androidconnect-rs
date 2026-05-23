package dev.androidconnect;

import android.Manifest;
import android.app.Activity;
import android.app.NotificationManager;
import android.app.role.RoleManager;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.media.AudioManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.net.wifi.WifiInfo;
import android.net.wifi.WifiManager;
import android.os.BatteryManager;
import android.os.Build;
import android.os.PowerManager;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.provider.Settings;
import android.telecom.TelecomManager;
import android.view.KeyEvent;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

public final class AndroidUtilityBridge {
    private static final int SHARE_BUFFER_BYTES = 64 * 1024;

    private AndroidUtilityBridge() {
    }

    public static void refreshAll(Context context) {
        if (context == null) {
            return;
        }
        pushDeviceStatus(context);
        pushPhotosPermissionStatus();
        pushMessagesPermissionStatus();
        pushCallState(context);
        pushRelayDisabledStatus();
    }

    public static void pushDeviceStatus(Context context) {
        if (context == null) {
            return;
        }
        JSONObject json = new JSONObject();
        try {
            json.put("device_name", Build.MANUFACTURER + " " + Build.MODEL);
            json.put("manufacturer", Build.MANUFACTURER);
            json.put("model", Build.MODEL);
            json.put("android_sdk", Build.VERSION.SDK_INT);
            BatteryState battery = readBatteryState(context);
            putNullable(json, "battery_percent", battery.percent);
            putNullable(json, "charging", battery.charging);
            putNullable(json, "interactive", isInteractive(context));
            json.put("features", buildFeatureStatuses(context));
            JSONObject wifi = readWifiState(context);
            if (wifi != null) {
                json.put("wifi_state", wifi);
            }
            JSONObject bluetooth = readBluetoothState(context);
            if (bluetooth != null) {
                json.put("bluetooth_state", bluetooth);
            }
            JSONObject dnd = readDndState(context);
            if (dnd != null) {
                json.put("dnd_state", dnd);
            }
            JSONObject volume = readVolumeState(context);
            if (volume != null) {
                json.put("volume", volume);
            }
            NativeBridge.pushDeviceStatusJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    public static void pushForegroundClipboard(Context context) {
        if (context == null) {
            return;
        }
        ClipboardManager clipboard =
                (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard == null || !clipboard.hasPrimaryClip()) {
            return;
        }
        ClipData clip = clipboard.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            return;
        }
        for (int i = 0; i < clip.getItemCount(); i++) {
            ClipData.Item item = clip.getItemAt(i);
            CharSequence text = item.coerceToText(context);
            if (text != null && text.length() > 0) {
                NativeBridge.pushClipboardText(text.toString());
                continue;
            }
            Uri uri = item.getUri();
            if (uri != null) {
                copyAndPushUri(context, uri, "clipboard");
            }
        }
    }

    public static void applyClipboardText(Context context, String text) {
        ClipboardManager clipboard =
                (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard == null) {
            return;
        }
        clipboard.setPrimaryClip(ClipData.newPlainText("AndroidConnect", text == null ? "" : text));
        Toast.makeText(context, "Clipboard updated from desktop", Toast.LENGTH_SHORT).show();
    }

    public static void applyMediaControl(Context context, String action) {
        AudioManager audio = (AudioManager) context.getSystemService(Context.AUDIO_SERVICE);
        if (audio == null) {
            return;
        }
        int keyCode = mediaKeyCode(action);
        if (keyCode == 0) {
            return;
        }
        long now = android.os.SystemClock.uptimeMillis();
        audio.dispatchMediaKeyEvent(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, 0));
        audio.dispatchMediaKeyEvent(new KeyEvent(now, now, KeyEvent.ACTION_UP, keyCode, 0));
    }

    public static void applyAudioControl(Context context, String command) {
        Toast.makeText(
                context,
                "Audio forwarding requires a supported capture path and is not active",
                Toast.LENGTH_SHORT).show();
        pushDeviceStatus(context);
    }

    public static void openApp(Context context, String packageName) {
        if (packageName == null || packageName.trim().isEmpty()) {
            return;
        }
        Intent launch = context.getPackageManager().getLaunchIntentForPackage(packageName);
        if (launch == null) {
            Toast.makeText(context, "App is not launchable: " + packageName, Toast.LENGTH_SHORT)
                    .show();
            return;
        }
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        context.startActivity(launch);
    }

    public static void applyCallAction(Context context, String action, String phoneNumber) {
        if (!"Dial".equals(action)) {
            pushCallState(context);
            return;
        }
        Intent intent = new Intent(Intent.ACTION_DIAL);
        if (phoneNumber != null && !phoneNumber.trim().isEmpty()) {
            intent.setData(Uri.parse("tel:" + Uri.encode(phoneNumber.trim())));
        }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        context.startActivity(intent);
    }

    public static void handleShareIntent(Context context, Intent intent) {
        if (context == null || intent == null) {
            return;
        }
        String action = intent.getAction();
        if (Intent.ACTION_SEND.equals(action)) {
            CharSequence text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT);
            if (text != null && text.length() > 0) {
                NativeBridge.pushClipboardText(text.toString());
            }
            Uri stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (stream != null) {
                copyAndPushUri(context, stream, "share");
            }
        } else if (Intent.ACTION_SEND_MULTIPLE.equals(action)) {
            java.util.ArrayList<Uri> streams = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (streams != null) {
                for (Uri stream : streams) {
                    if (stream != null) {
                        copyAndPushUri(context, stream, "share");
                    }
                }
            }
        }
    }

    public static void handlePickedUri(Context context, Uri uri) {
        if (context != null && uri != null) {
            copyAndPushUri(context, uri, "picked");
        }
    }

    public static void pushPhotosPermissionStatus() {
        Context context = context();
        if (context == null) {
            return;
        }
        if (!canReadPhotos(context)) {
            pushPhotoStatus("PermissionRequired", "Photo and video media permission is required");
            return;
        }
        JSONObject json = new JSONObject();
        JSONArray assets = new JSONArray();
        try {
            appendMediaAssets(context, MediaStore.Images.Media.EXTERNAL_CONTENT_URI, "image", assets);
            appendMediaAssets(context, MediaStore.Video.Media.EXTERNAL_CONTENT_URI, "video", assets);
            json.put("request_id", "android-media-refresh");
            json.put("assets", assets);
            json.put("status", featureStatus("Photos", "Available", ""));
            NativeBridge.pushPhotoAssetListJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    public static void pushMessagesPermissionStatus() {
        JSONObject json = new JSONObject();
        try {
            json.put("threads", new JSONArray());
            json.put("status", featureStatus(
                    "Messages",
                    "Unsupported",
                    "SMS/MMS sending requires Android role and policy review; RCS is provider limited"));
            NativeBridge.pushMessageThreadListJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    public static void pushCallState(Context context) {
        JSONObject json = new JSONObject();
        try {
            putNullable(json, "call_id", null);
            json.put("state", "Idle");
            putNullable(json, "display_name", null);
            putNullable(json, "phone_number", null);
            JSONArray actions = new JSONArray();
            actions.put("Dial");
            json.put("supported_actions", actions);
            String state = canUseCallSurfaces(context) ? "Available" : "PermissionRequired";
            String message = canUseCallSurfaces(context)
                    ? ""
                    : "Call actions require phone role and runtime permissions";
            json.put("status", featureStatus("Calls", state, message));
            NativeBridge.pushCallStateJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    public static void pushRelayDisabledStatus() {
        JSONObject json = new JSONObject();
        try {
            json.put("enabled", false);
            json.put("connected", false);
            json.put("remote", false);
            json.put("status", featureStatus(
                    "Relay",
                    "Disabled",
                    "Remote relay/NAT traversal is opt-in and no relay endpoint is configured"));
            NativeBridge.pushRelayStatusJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    private static JSONArray buildFeatureStatuses(Context context) throws JSONException {
        JSONArray features = new JSONArray();
        features.put(featureStatus("DeviceStatus", "Available", ""));
        features.put(featureStatus("MediaControls",
                notificationListenerEnabled(context) ? "Available" : "PermissionRequired",
                notificationListenerEnabled(context)
                        ? ""
                        : "Notification listener access enables active media-session metadata"));
        features.put(featureStatus("ClipboardText", "Available",
                "Foreground explicit clipboard sync only"));
        features.put(featureStatus("ClipboardImage", "PermissionRequired",
                "Image clipboard uses explicit share/content URI access when Android exposes it"));
        features.put(featureStatus("FileTransfer", "Available",
                "Explicit share and app-private received files are supported"));
        features.put(featureStatus("FileBrowser", "Available",
                "Remote browsing is limited to AndroidConnect app storage until a document tree is granted"));
        features.put(featureStatus("Notifications",
                notificationListenerEnabled(context) ? "Available" : "PermissionRequired",
                notificationListenerEnabled(context)
                        ? ""
                        : "Enable AndroidConnect in notification listener settings"));
        features.put(featureStatus("AudioForwarding", "Disabled",
                "Audio forwarding needs a supported playback capture path and separate user enablement"));
        features.put(featureStatus("AppWindows",
                RemoteControlAccessibilityService.isRunning() ? "Available" : "PermissionRequired",
                RemoteControlAccessibilityService.isRunning()
                        ? "Launch-app requests are supported; independent app windows remain device/API limited"
                        : "Accessibility control must be enabled for app-focused input"));
        features.put(featureStatus("Messages", "Unsupported",
                "SMS/MMS/RCS require Android role/provider support and are not enabled by default"));
        features.put(featureStatus("Calls", canUseCallSurfaces(context) ? "Available" : "PermissionRequired",
                canUseCallSurfaces(context) ? "" : "Phone permissions and roles are required"));
        features.put(featureStatus("Photos",
                canReadPhotos(context) ? "Available" : "PermissionRequired",
                canReadPhotos(context) ? "" : "Photo and video media permission is required"));
        features.put(featureStatus("Relay", "Disabled",
                "LAN/manual connection remains the default"));
        features.put(featureStatus("MultiClient", "Disabled",
                "Current Android transport exposes one active desktop session with trusted-desktop inventory"));
        return features;
    }

    private static JSONObject featureStatus(String feature, String state, String message)
            throws JSONException {
        JSONObject json = new JSONObject();
        json.put("feature", feature);
        json.put("state", state);
        json.put("message", message == null ? "" : message);
        return json;
    }

    private static void pushPhotoStatus(String state, String message) {
        JSONObject json = new JSONObject();
        try {
            json.put("request_id", "android-media-refresh");
            json.put("assets", new JSONArray());
            json.put("status", featureStatus("Photos", state, message));
            NativeBridge.pushPhotoAssetListJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    private static BatteryState readBatteryState(Context context) {
        Intent battery = context.registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        if (battery == null) {
            return new BatteryState(null, null);
        }
        int level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
        int scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
        Integer percent = null;
        if (level >= 0 && scale > 0) {
            percent = Math.round(level * 100f / scale);
        }
        int status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
        Boolean charging = status == BatteryManager.BATTERY_STATUS_CHARGING
                || status == BatteryManager.BATTERY_STATUS_FULL;
        return new BatteryState(percent, charging);
    }

    private static Boolean isInteractive(Context context) {
        PowerManager power = (PowerManager) context.getSystemService(Context.POWER_SERVICE);
        return power == null ? null : power.isInteractive();
    }

    private static JSONObject readWifiState(Context context) {
        try {
            ConnectivityManager cm =
                    (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
            boolean connected = false;
            if (cm != null) {
                Network active = cm.getActiveNetwork();
                if (active != null) {
                    NetworkCapabilities caps = cm.getNetworkCapabilities(active);
                    connected = caps != null
                            && caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI);
                }
            }

            JSONObject json = new JSONObject();
            json.put("connected", connected);

            String ssid = null;
            Integer signal = null;
            WifiManager wifi = (WifiManager)
                    context.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wifi != null && connected) {
                WifiInfo info = wifi.getConnectionInfo();
                if (info != null) {
                    String rawSsid = info.getSSID();
                    if (rawSsid != null
                            && !rawSsid.isEmpty()
                            && !"<unknown ssid>".equalsIgnoreCase(rawSsid)) {
                        // Strip the surrounding quotes Android adds.
                        if (rawSsid.startsWith("\"") && rawSsid.endsWith("\"")
                                && rawSsid.length() >= 2) {
                            ssid = rawSsid.substring(1, rawSsid.length() - 1);
                        } else {
                            ssid = rawSsid;
                        }
                    }
                    int rssi = info.getRssi();
                    if (rssi != Integer.MIN_VALUE) {
                        signal = rssi;
                    }
                }
            }
            putNullable(json, "ssid", ssid);
            putNullable(json, "signal_strength", signal);
            return json;
        } catch (JSONException error) {
            return null;
        } catch (SecurityException ignored) {
            return null;
        }
    }

    private static JSONObject readBluetoothState(Context context) {
        try {
            BluetoothAdapter adapter;
            if (Build.VERSION.SDK_INT >= 18) {
                BluetoothManager manager =
                        (BluetoothManager) context.getSystemService(Context.BLUETOOTH_SERVICE);
                adapter = manager == null ? BluetoothAdapter.getDefaultAdapter() : manager.getAdapter();
            } else {
                adapter = BluetoothAdapter.getDefaultAdapter();
            }
            if (adapter == null) {
                return null;
            }
            boolean enabled;
            try {
                enabled = adapter.isEnabled();
            } catch (SecurityException ignored) {
                enabled = false;
            }

            int connected = 0;
            if (enabled && hasBluetoothConnectPermission(context)) {
                try {
                    BluetoothManager manager = (BluetoothManager)
                            context.getSystemService(Context.BLUETOOTH_SERVICE);
                    if (manager != null) {
                        connected = manager.getConnectedDevices(BluetoothProfile.GATT).size();
                    }
                } catch (SecurityException ignored) {
                }
            }

            JSONObject json = new JSONObject();
            json.put("enabled", enabled);
            json.put("connected_devices", Math.max(0, Math.min(connected, 255)));
            return json;
        } catch (JSONException error) {
            return null;
        }
    }

    private static JSONObject readDndState(Context context) {
        try {
            NotificationManager nm =
                    (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
            if (nm == null) {
                return null;
            }
            int filter;
            try {
                filter = nm.getCurrentInterruptionFilter();
            } catch (SecurityException ignored) {
                return null;
            }
            String mode;
            switch (filter) {
                case NotificationManager.INTERRUPTION_FILTER_NONE:
                    mode = "TotalSilence";
                    break;
                case NotificationManager.INTERRUPTION_FILTER_ALARMS:
                    mode = "Alarms";
                    break;
                case NotificationManager.INTERRUPTION_FILTER_PRIORITY:
                    mode = "Priority";
                    break;
                case NotificationManager.INTERRUPTION_FILTER_ALL:
                case NotificationManager.INTERRUPTION_FILTER_UNKNOWN:
                default:
                    mode = "Off";
                    break;
            }
            JSONObject json = new JSONObject();
            json.put("enabled", filter != NotificationManager.INTERRUPTION_FILTER_ALL
                    && filter != NotificationManager.INTERRUPTION_FILTER_UNKNOWN);
            json.put("mode", mode);
            return json;
        } catch (JSONException error) {
            return null;
        }
    }

    private static JSONObject readVolumeState(Context context) {
        try {
            AudioManager audio = (AudioManager) context.getSystemService(Context.AUDIO_SERVICE);
            if (audio == null) {
                return null;
            }
            int musicMax = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC);
            int musicCur = audio.getStreamVolume(AudioManager.STREAM_MUSIC);
            int mediaPercent = musicMax <= 0 ? 0 : Math.round(musicCur * 100f / musicMax);

            Integer ringPercent = null;
            try {
                int ringMax = audio.getStreamMaxVolume(AudioManager.STREAM_RING);
                int ringCur = audio.getStreamVolume(AudioManager.STREAM_RING);
                if (ringMax > 0) {
                    ringPercent = Math.round(ringCur * 100f / ringMax);
                }
            } catch (RuntimeException ignored) {
            }

            JSONObject json = new JSONObject();
            json.put("media_percent", clampPercent(mediaPercent));
            putNullable(json, "ring_percent", ringPercent == null ? null : clampPercent(ringPercent));
            json.put("max_percent", 100);
            return json;
        } catch (JSONException error) {
            return null;
        }
    }

    private static int clampPercent(int value) {
        if (value < 0) return 0;
        if (value > 255) return 255;
        return value;
    }

    private static boolean hasBluetoothConnectPermission(Context context) {
        if (Build.VERSION.SDK_INT < 31) {
            return true;
        }
        return context.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT)
                == PackageManager.PERMISSION_GRANTED;
    }

    private static boolean notificationListenerEnabled(Context context) {
        String enabled = Settings.Secure.getString(
                context.getContentResolver(),
                "enabled_notification_listeners");
        return enabled != null && enabled.toLowerCase().contains(context.getPackageName().toLowerCase());
    }

    private static boolean canReadPhotos(Context context) {
        if (Build.VERSION.SDK_INT >= 33) {
            return context.checkSelfPermission(Manifest.permission.READ_MEDIA_IMAGES)
                    == PackageManager.PERMISSION_GRANTED
                    || context.checkSelfPermission(Manifest.permission.READ_MEDIA_VIDEO)
                    == PackageManager.PERMISSION_GRANTED;
        }
        return context.checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE)
                == PackageManager.PERMISSION_GRANTED;
    }

    private static boolean canUseCallSurfaces(Context context) {
        if (context == null) {
            return false;
        }
        if (context.checkSelfPermission(Manifest.permission.READ_PHONE_STATE)
                != PackageManager.PERMISSION_GRANTED) {
            return false;
        }
        if (Build.VERSION.SDK_INT >= 29) {
            RoleManager roles = (RoleManager) context.getSystemService(RoleManager.class);
            return roles != null && roles.isRoleAvailable(RoleManager.ROLE_DIALER);
        }
        TelecomManager telecom = (TelecomManager) context.getSystemService(Context.TELECOM_SERVICE);
        return telecom != null;
    }

    private static int mediaKeyCode(String action) {
        if ("Play".equals(action)) return KeyEvent.KEYCODE_MEDIA_PLAY;
        if ("Pause".equals(action)) return KeyEvent.KEYCODE_MEDIA_PAUSE;
        if ("PlayPause".equals(action)) return KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE;
        if ("Previous".equals(action)) return KeyEvent.KEYCODE_MEDIA_PREVIOUS;
        if ("Next".equals(action)) return KeyEvent.KEYCODE_MEDIA_NEXT;
        if ("Stop".equals(action)) return KeyEvent.KEYCODE_MEDIA_STOP;
        return 0;
    }

    private static void copyAndPushUri(Context context, Uri uri, String source) {
        try {
            SharedContent content = copyUriToCache(context, uri, source);
            NativeBridge.pushSharedFile(
                    content.displayName,
                    content.mimeType == null ? "" : content.mimeType,
                    content.file.getAbsolutePath(),
                    content.sizeBytes);
        } catch (IOException error) {
            Toast.makeText(context, "Share failed: " + error.getMessage(), Toast.LENGTH_SHORT).show();
        }
    }

    private static SharedContent copyUriToCache(Context context, Uri uri, String source)
            throws IOException {
        ContentResolver resolver = context.getContentResolver();
        String displayName = displayNameFor(resolver, uri);
        String mimeType = resolver.getType(uri);
        File dir = new File(context.getCacheDir(), "androidconnect-shares");
        if (!dir.exists() && !dir.mkdirs()) {
            throw new IOException("cannot create share cache");
        }
        File out = uniqueFile(dir, source + "-" + displayName);
        long copied = 0;
        try (InputStream input = resolver.openInputStream(uri);
             FileOutputStream output = new FileOutputStream(out)) {
            if (input == null) {
                throw new IOException("cannot open " + uri);
            }
            byte[] buffer = new byte[SHARE_BUFFER_BYTES];
            int read;
            while ((read = input.read(buffer)) != -1) {
                output.write(buffer, 0, read);
                copied += read;
            }
        }
        return new SharedContent(out, displayName, mimeType, copied);
    }

    private static String displayNameFor(ContentResolver resolver, Uri uri) {
        try (Cursor cursor = resolver.query(
                uri,
                new String[] { OpenableColumns.DISPLAY_NAME },
                null,
                null,
                null)) {
            if (cursor != null && cursor.moveToFirst()) {
                String name = cursor.getString(0);
                if (name != null && !name.trim().isEmpty()) {
                    return sanitizeFileName(name);
                }
            }
        } catch (RuntimeException ignored) {
        }
        String fallback = uri.getLastPathSegment();
        return sanitizeFileName(fallback == null ? "shared-file" : fallback);
    }

    private static File uniqueFile(File dir, String name) {
        File file = new File(dir, sanitizeFileName(name));
        if (!file.exists()) {
            return file;
        }
        for (int i = 1; i < 10_000; i++) {
            File candidate = new File(dir, i + "-" + sanitizeFileName(name));
            if (!candidate.exists()) {
                return candidate;
            }
        }
        return new File(dir, System.currentTimeMillis() + "-" + sanitizeFileName(name));
    }

    private static String sanitizeFileName(String name) {
        String clean = name == null ? "file" : name.replaceAll("[\\\\/:\\u0000-\\u001f]", "_");
        clean = clean.replaceAll("^\\.+", "").trim();
        return clean.isEmpty() ? "file" : clean;
    }

    private static void appendMediaAssets(
            Context context,
            Uri collection,
            String prefix,
            JSONArray assets
    ) throws JSONException {
        String[] projection = new String[] {
                MediaStore.MediaColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.MediaColumns.MIME_TYPE,
                MediaStore.MediaColumns.WIDTH,
                MediaStore.MediaColumns.HEIGHT,
                MediaStore.MediaColumns.SIZE,
                MediaStore.MediaColumns.DATE_ADDED
        };
        String sort = MediaStore.MediaColumns.DATE_ADDED + " DESC";
        try (Cursor cursor = context.getContentResolver().query(
                collection,
                projection,
                null,
                null,
                sort)) {
            if (cursor == null) {
                return;
            }
            int count = 0;
            while (cursor.moveToNext() && count < 100) {
                JSONObject item = new JSONObject();
                long id = cursor.getLong(0);
                item.put("asset_id", prefix + ":" + id);
                item.put("display_name", cursor.getString(1));
                item.put("mime_type", cursor.getString(2));
                putNullable(item, "width", cursor.isNull(3) ? null : cursor.getInt(3));
                putNullable(item, "height", cursor.isNull(4) ? null : cursor.getInt(4));
                putNullable(item, "size_bytes", cursor.isNull(5) ? null : cursor.getLong(5));
                putNullable(item, "created_unix_ms", cursor.isNull(6) ? null : cursor.getLong(6) * 1000L);
                assets.put(item);
                count++;
            }
        } catch (RuntimeException ignored) {
        }
    }

    private static void putNullable(JSONObject json, String key, Object value) throws JSONException {
        json.put(key, value == null ? JSONObject.NULL : value);
    }

    private static Context context() {
        return AppContextHolder.context;
    }

    static void rememberContext(Context context) {
        AppContextHolder.context = context == null ? null : context.getApplicationContext();
    }

    private static final class AppContextHolder {
        private static volatile Context context;
    }

    private static final class BatteryState {
        final Integer percent;
        final Boolean charging;

        BatteryState(Integer percent, Boolean charging) {
            this.percent = percent;
            this.charging = charging;
        }
    }

    private static final class SharedContent {
        final File file;
        final String displayName;
        final String mimeType;
        final long sizeBytes;

        SharedContent(File file, String displayName, String mimeType, long sizeBytes) {
            this.file = file;
            this.displayName = displayName;
            this.mimeType = mimeType;
            this.sizeBytes = sizeBytes;
        }
    }
}
