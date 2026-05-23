package dev.androidconnect;

import android.Manifest;
import android.content.ComponentName;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.Build;
import android.provider.Settings;
import android.text.TextUtils;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Computes the current permission/feature grant state for the user-facing
 * onboarding screen. The repository is intentionally stateless beyond a Context
 * reference so that callers can re-query on {@code onResume} and rely on
 * fresh OS-level reads each time.
 */
public final class PermissionStatusRepository {

    /** Stable identifier for each user-facing permission card. */
    public enum PermissionId {
        NOTIFICATION_LISTENER,
        ACCESSIBILITY,
        SMS,
        CONTACTS,
        STORAGE,
        MICROPHONE,
        BLUETOOTH,
        PHONE,
    }

    /** Outcome categories the UI cares about. */
    public enum State {
        GRANTED,
        NOT_GRANTED,
        NEEDS_SETTINGS_PAGE,
    }

    public static final class PermissionState {
        public final PermissionId id;
        public final String title;
        public final String description;
        public final State state;
        public final boolean required;

        PermissionState(PermissionId id, String title, String description, State state,
                        boolean required) {
            this.id = id;
            this.title = title;
            this.description = description;
            this.state = state;
            this.required = required;
        }

        public String stateLabel() {
            switch (state) {
                case GRANTED:
                    return "granted";
                case NEEDS_SETTINGS_PAGE:
                    return "needs settings page";
                case NOT_GRANTED:
                default:
                    return "not granted";
            }
        }
    }

    private final Context appContext;

    public PermissionStatusRepository(Context context) {
        this.appContext = context.getApplicationContext();
    }

    /** Snapshot the live state for every supported permission card. */
    public Map<PermissionId, PermissionState> snapshot() {
        Map<PermissionId, PermissionState> map = new LinkedHashMap<>();
        map.put(PermissionId.NOTIFICATION_LISTENER, notificationListenerState());
        map.put(PermissionId.ACCESSIBILITY, accessibilityState());
        map.put(PermissionId.SMS, smsState());
        map.put(PermissionId.CONTACTS, contactsState());
        map.put(PermissionId.STORAGE, storageState());
        map.put(PermissionId.MICROPHONE, microphoneState());
        map.put(PermissionId.BLUETOOTH, bluetoothState());
        map.put(PermissionId.PHONE, phoneState());
        return map;
    }

    public PermissionState get(PermissionId id) {
        return snapshot().get(id);
    }

    public boolean allRequiredGranted() {
        for (PermissionState ps : snapshot().values()) {
            if (ps.required && ps.state != State.GRANTED) {
                return false;
            }
        }
        return true;
    }

    /** Runtime permission strings for cards that use ActivityCompat.requestPermissions. */
    public static String[] runtimePermissionsFor(PermissionId id) {
        switch (id) {
            case SMS:
                return new String[] {
                        Manifest.permission.RECEIVE_SMS,
                        Manifest.permission.READ_SMS,
                        Manifest.permission.SEND_SMS,
                };
            case CONTACTS:
                return new String[] { Manifest.permission.READ_CONTACTS };
            case STORAGE:
                if (Build.VERSION.SDK_INT >= 33) {
                    return new String[] {
                            Manifest.permission.READ_MEDIA_IMAGES,
                            Manifest.permission.READ_MEDIA_VIDEO,
                    };
                }
                return new String[] { Manifest.permission.READ_EXTERNAL_STORAGE };
            case MICROPHONE:
                return new String[] { Manifest.permission.RECORD_AUDIO };
            case BLUETOOTH:
                if (Build.VERSION.SDK_INT >= 31) {
                    return new String[] { Manifest.permission.BLUETOOTH_CONNECT };
                }
                return new String[0];
            case PHONE:
                return new String[] {
                        Manifest.permission.READ_PHONE_STATE,
                        Manifest.permission.CALL_PHONE,
                };
            case NOTIFICATION_LISTENER:
            case ACCESSIBILITY:
            default:
                return new String[0];
        }
    }

    private PermissionState notificationListenerState() {
        String enabled = Settings.Secure.getString(
                appContext.getContentResolver(), "enabled_notification_listeners");
        boolean granted = enabled != null
                && enabled.toLowerCase().contains(appContext.getPackageName().toLowerCase());
        return new PermissionState(
                PermissionId.NOTIFICATION_LISTENER,
                "Notification access",
                "Required to mirror notifications and media metadata to the desktop.",
                granted ? State.GRANTED : State.NEEDS_SETTINGS_PAGE,
                true);
    }

    private PermissionState accessibilityState() {
        ComponentName component = new ComponentName(
                appContext, RemoteControlAccessibilityService.class);
        String flattened = component.flattenToString();
        String enabledServices = Settings.Secure.getString(
                appContext.getContentResolver(),
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        boolean granted = !TextUtils.isEmpty(enabledServices)
                && enabledServices.toLowerCase().contains(flattened.toLowerCase());
        return new PermissionState(
                PermissionId.ACCESSIBILITY,
                "Accessibility service",
                "Required so the desktop can dispatch pointer and key input to your phone.",
                granted ? State.GRANTED : State.NEEDS_SETTINGS_PAGE,
                true);
    }

    private PermissionState smsState() {
        boolean granted = hasAll(
                Manifest.permission.RECEIVE_SMS,
                Manifest.permission.READ_SMS,
                Manifest.permission.SEND_SMS);
        return new PermissionState(
                PermissionId.SMS,
                "SMS",
                "Optional. Enables reading and sending SMS from the desktop.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private PermissionState contactsState() {
        boolean granted = hasAll(Manifest.permission.READ_CONTACTS);
        return new PermissionState(
                PermissionId.CONTACTS,
                "Contacts",
                "Optional. Resolves phone numbers to contact names in notifications and SMS.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private PermissionState storageState() {
        boolean granted;
        if (Build.VERSION.SDK_INT >= 33) {
            granted = appContext.checkSelfPermission(Manifest.permission.READ_MEDIA_IMAGES)
                    == PackageManager.PERMISSION_GRANTED
                    || appContext.checkSelfPermission(Manifest.permission.READ_MEDIA_VIDEO)
                    == PackageManager.PERMISSION_GRANTED;
        } else {
            granted = hasAll(Manifest.permission.READ_EXTERNAL_STORAGE);
        }
        return new PermissionState(
                PermissionId.STORAGE,
                "Photos and media",
                "Optional. Lets the desktop browse and share photos and videos.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private PermissionState microphoneState() {
        boolean granted = hasAll(Manifest.permission.RECORD_AUDIO);
        return new PermissionState(
                PermissionId.MICROPHONE,
                "Microphone",
                "Optional. Enables audio-call features that need microphone capture.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private PermissionState bluetoothState() {
        if (Build.VERSION.SDK_INT < 31) {
            return new PermissionState(
                    PermissionId.BLUETOOTH,
                    "Bluetooth",
                    "Granted by default on this Android version.",
                    State.GRANTED,
                    false);
        }
        boolean granted = hasAll(Manifest.permission.BLUETOOTH_CONNECT);
        return new PermissionState(
                PermissionId.BLUETOOTH,
                "Bluetooth",
                "Optional. Reports paired-device count and lets the desktop manage Bluetooth.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private PermissionState phoneState() {
        boolean granted = hasAll(
                Manifest.permission.READ_PHONE_STATE,
                Manifest.permission.CALL_PHONE);
        return new PermissionState(
                PermissionId.PHONE,
                "Phone",
                "Optional. Required to surface call state and dial from the desktop.",
                granted ? State.GRANTED : State.NOT_GRANTED,
                false);
    }

    private boolean hasAll(String... permissions) {
        for (String perm : permissions) {
            if (appContext.checkSelfPermission(perm) != PackageManager.PERMISSION_GRANTED) {
                return false;
            }
        }
        return true;
    }
}
