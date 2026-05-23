package dev.androidconnect;

import android.app.Notification;
import android.app.PendingIntent;
import android.app.RemoteInput;
import android.content.ComponentName;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.media.MediaMetadata;
import android.media.session.MediaController;
import android.media.session.MediaSessionManager;
import android.media.session.PlaybackState;
import android.os.Bundle;
import android.service.notification.NotificationListenerService;
import android.service.notification.StatusBarNotification;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;

public final class AndroidConnectNotificationService extends NotificationListenerService {
    private static final String FILTER_PREFS = "androidconnect_notification_filters";
    private static final String FILTER_KEY_PREFIX = "package.";
    private static volatile AndroidConnectNotificationService activeService;

    @Override
    public void onListenerConnected() {
        activeService = this;
        AndroidUtilityBridge.rememberContext(this);
        refreshMediaStatus();
    }

    @Override
    public void onListenerDisconnected() {
        if (activeService == this) {
            activeService = null;
        }
    }

    @Override
    public void onNotificationPosted(StatusBarNotification sbn) {
        AndroidUtilityBridge.rememberContext(this);
        if (sbn != null && !isPackageEnabled(sbn.getPackageName())) {
            return;
        }
        pushNotificationPosted(sbn);
        refreshMediaStatus();
    }

    @Override
    public void onNotificationRemoved(StatusBarNotification sbn) {
        if (sbn != null) {
            NativeBridge.pushNotificationRemoved(sbn.getKey());
        }
        refreshMediaStatus();
    }

    public static void performNotificationAction(String actionJson) {
        AndroidConnectNotificationService service = activeService;
        if (service == null) {
            return;
        }
        try {
            JSONObject json = new JSONObject(actionJson);
            String notificationId = json.optString("notification_id", "");
            String actionId = json.optString("action_id", "");
            String replyText = json.optString("reply_text", null);
            service.performNotificationAction(notificationId, actionId, replyText);
        } catch (JSONException ignored) {
        }
    }

    public static void applyNotificationFilterUpdate(String filterJson) {
        AndroidConnectNotificationService service = activeService;
        if (service == null) {
            return;
        }
        try {
            JSONObject json = new JSONObject(filterJson);
            String packageName = json.optString("package_name", "").trim();
            if (packageName.isEmpty()) {
                return;
            }
            boolean enabled = json.optBoolean("enabled", true);
            service.setPackageEnabled(packageName, enabled);
        } catch (JSONException ignored) {
        }
    }

    private void pushNotificationPosted(StatusBarNotification sbn) {
        if (sbn == null || sbn.getNotification() == null) {
            return;
        }
        Notification notification = sbn.getNotification();
        Bundle extras = notification.extras;
        JSONObject json = new JSONObject();
        try {
            json.put("notification_id", sbn.getKey());
            json.put("app_package", sbn.getPackageName());
            json.put("app_name", appLabel(sbn.getPackageName()));
            putNullable(json, "title", extras == null ? null : extras.getCharSequence(Notification.EXTRA_TITLE));
            putNullable(json, "text", extras == null ? null : extras.getCharSequence(Notification.EXTRA_TEXT));
            json.put("timestamp_unix_ms", sbn.getPostTime());
            json.put("sensitive", notification.visibility != Notification.VISIBILITY_PUBLIC);
            JSONArray actions = new JSONArray();
            if (notification.actions != null) {
                for (int i = 0; i < notification.actions.length; i++) {
                    Notification.Action action = notification.actions[i];
                    JSONObject actionJson = new JSONObject();
                    actionJson.put("action_id", Integer.toString(i));
                    actionJson.put("title", action.title == null ? "" : action.title.toString());
                    actionJson.put("allows_reply", action.getRemoteInputs() != null
                            && action.getRemoteInputs().length > 0);
                    actions.put(actionJson);
                }
            }
            json.put("actions", actions);
            NativeBridge.pushNotificationPostedJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    private void performNotificationAction(String notificationId, String actionId, String replyText) {
        StatusBarNotification[] notifications = getActiveNotifications();
        if (notifications == null) {
            return;
        }
        for (StatusBarNotification sbn : notifications) {
            if (sbn == null || !sbn.getKey().equals(notificationId)) {
                continue;
            }
            Notification.Action[] actions = sbn.getNotification().actions;
            int index;
            try {
                index = Integer.parseInt(actionId);
            } catch (NumberFormatException ignored) {
                return;
            }
            if (actions == null || index < 0 || index >= actions.length) {
                return;
            }
            try {
                Notification.Action action = actions[index];
                Intent fillIn = new Intent();
                RemoteInput[] inputs = action.getRemoteInputs();
                if (replyText != null && inputs != null && inputs.length > 0) {
                    Bundle results = new Bundle();
                    for (RemoteInput input : inputs) {
                        results.putCharSequence(input.getResultKey(), replyText);
                    }
                    RemoteInput.addResultsToIntent(inputs, fillIn, results);
                }
                action.actionIntent.send(this, 0, fillIn);
            } catch (PendingIntent.CanceledException ignored) {
            }
            return;
        }
    }

    private boolean isPackageEnabled(String packageName) {
        if (packageName == null || packageName.trim().isEmpty()) {
            return true;
        }
        return getSharedPreferences(FILTER_PREFS, MODE_PRIVATE)
                .getBoolean(FILTER_KEY_PREFIX + packageName, true);
    }

    private void setPackageEnabled(String packageName, boolean enabled) {
        SharedPreferences.Editor editor = getSharedPreferences(FILTER_PREFS, MODE_PRIVATE).edit();
        String key = FILTER_KEY_PREFIX + packageName;
        if (enabled) {
            editor.remove(key);
        } else {
            editor.putBoolean(key, false);
        }
        editor.apply();
    }

    private void refreshMediaStatus() {
        MediaSessionManager manager =
                (MediaSessionManager) getSystemService(MEDIA_SESSION_SERVICE);
        if (manager == null) {
            pushNoMedia("Unsupported", "MediaSessionManager is unavailable");
            return;
        }
        List<MediaController> controllers = manager.getActiveSessions(
                new ComponentName(this, AndroidConnectNotificationService.class));
        if (controllers == null || controllers.isEmpty()) {
            pushNoMedia("Available", "No controllable active media session");
            return;
        }
        MediaController controller = controllers.get(0);
        MediaMetadata metadata = controller.getMetadata();
        PlaybackState state = controller.getPlaybackState();
        JSONObject json = new JSONObject();
        try {
            json.put("active", true);
            putNullable(json, "app_package", controller.getPackageName());
            putNullable(json, "app_name", appLabel(controller.getPackageName()));
            putNullable(json, "title", metadata == null ? null
                    : metadata.getString(MediaMetadata.METADATA_KEY_TITLE));
            putNullable(json, "artist", metadata == null ? null
                    : metadata.getString(MediaMetadata.METADATA_KEY_ARTIST));
            putNullable(json, "album", metadata == null ? null
                    : metadata.getString(MediaMetadata.METADATA_KEY_ALBUM));
            json.put("playback_state", playbackStateName(state));
            putNullable(json, "position_ms", state == null ? null : state.getPosition());
            putNullable(json, "duration_ms", metadata == null ? null
                    : metadata.getLong(MediaMetadata.METADATA_KEY_DURATION));
            json.put("supported_actions", supportedMediaActions(state));
            json.put("status", featureStatus("MediaControls", "Available", ""));
            NativeBridge.pushMediaStatusJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    private void pushNoMedia(String state, String message) {
        JSONObject json = new JSONObject();
        try {
            json.put("active", false);
            putNullable(json, "app_package", null);
            putNullable(json, "app_name", null);
            putNullable(json, "title", null);
            putNullable(json, "artist", null);
            putNullable(json, "album", null);
            json.put("playback_state", "None");
            putNullable(json, "position_ms", null);
            putNullable(json, "duration_ms", null);
            json.put("supported_actions", new JSONArray());
            json.put("status", featureStatus("MediaControls", state, message));
            NativeBridge.pushMediaStatusJson(json.toString());
        } catch (JSONException ignored) {
        }
    }

    private JSONArray supportedMediaActions(PlaybackState state) {
        JSONArray actions = new JSONArray();
        if (state == null) {
            return actions;
        }
        long supported = state.getActions();
        if ((supported & PlaybackState.ACTION_PLAY) != 0) actions.put("Play");
        if ((supported & PlaybackState.ACTION_PAUSE) != 0) actions.put("Pause");
        if ((supported & PlaybackState.ACTION_PLAY_PAUSE) != 0) actions.put("PlayPause");
        if ((supported & PlaybackState.ACTION_SKIP_TO_PREVIOUS) != 0) actions.put("Previous");
        if ((supported & PlaybackState.ACTION_SKIP_TO_NEXT) != 0) actions.put("Next");
        if ((supported & PlaybackState.ACTION_STOP) != 0) actions.put("Stop");
        return actions;
    }

    private String playbackStateName(PlaybackState state) {
        if (state == null) {
            return "Unknown";
        }
        switch (state.getState()) {
            case PlaybackState.STATE_STOPPED:
                return "Stopped";
            case PlaybackState.STATE_PAUSED:
                return "Paused";
            case PlaybackState.STATE_PLAYING:
                return "Playing";
            case PlaybackState.STATE_BUFFERING:
            case PlaybackState.STATE_CONNECTING:
                return "Buffering";
            case PlaybackState.STATE_ERROR:
                return "Error";
            default:
                return "Unknown";
        }
    }

    private String appLabel(String packageName) {
        try {
            ApplicationInfo info = getPackageManager().getApplicationInfo(packageName, 0);
            CharSequence label = getPackageManager().getApplicationLabel(info);
            return label == null ? packageName : label.toString();
        } catch (PackageManager.NameNotFoundException error) {
            return packageName;
        }
    }

    private JSONObject featureStatus(String feature, String state, String message)
            throws JSONException {
        JSONObject json = new JSONObject();
        json.put("feature", feature);
        json.put("state", state);
        json.put("message", message == null ? "" : message);
        return json;
    }

    private void putNullable(JSONObject json, String key, Object value) throws JSONException {
        json.put(key, value == null
                ? JSONObject.NULL
                : value instanceof CharSequence ? value.toString() : value);
    }
}
