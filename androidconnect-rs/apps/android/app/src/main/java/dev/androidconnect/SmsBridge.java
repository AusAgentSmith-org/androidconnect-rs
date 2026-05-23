package dev.androidconnect;

import android.Manifest;
import android.content.Context;
import android.content.pm.PackageManager;
import android.database.ContentObserver;
import android.database.Cursor;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.provider.ContactsContract;
import android.provider.Telephony;
import android.telephony.SmsManager;
import android.util.Log;

import androidx.core.content.ContextCompat;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Bridges between the SMS provider on Android and the AndroidConnect protocol's Message*
 * payloads. Surfaces:
 * <ul>
 *     <li>{@link #pushThreadList(Context)} → MessageThreadList</li>
 *     <li>{@link #pushThreadDetail(Context, String, String, int)} → MessageThreadDetail (in
 *         response to a desktop MessageThreadOpen)</li>
 *     <li>{@link #sendMessage(Context, String)} → MessageSendResponse (in response to a desktop
 *         MessageSendRequest)</li>
 * </ul>
 *
 * Read access requires READ_SMS; sending requires SEND_SMS. When permissions are missing the
 * thread list and detail are pushed with FeatureStatus = PermissionRequired so the desktop UI
 * can show the onboarding CTA.
 */
final class SmsBridge {
    private static final String TAG = "SmsBridge";
    private static final int DEFAULT_DETAIL_LIMIT = 100;
    private static final ExecutorService EXEC = Executors.newSingleThreadExecutor();
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private static volatile ContentObserver smsObserver;
    private static volatile Context observerContext;
    private static volatile long lastKnownMaxSmsId = -1L;

    private SmsBridge() {}

    static void pushThreadList(Context context) {
        EXEC.execute(() -> {
            try {
                if (!canReadSms(context)) {
                    NativeBridge.pushMessageThreadListJson(
                            permissionRequiredThreadList("READ_SMS permission required."));
                    return;
                }
                JSONArray threads = readThreadSummaries(context);
                JSONObject json = new JSONObject();
                json.put("threads", threads);
                json.put("status", featureStatus("Messages", "Available", ""));
                NativeBridge.pushMessageThreadListJson(json.toString());
            } catch (SecurityException se) {
                Log.w(TAG, "SecurityException reading SMS threads: " + se.getMessage());
                NativeBridge.pushMessageThreadListJson(
                        permissionRequiredThreadList(se.getMessage()));
            } catch (Exception e) {
                Log.w(TAG, "pushThreadList failed", e);
                NativeBridge.pushMessageThreadListJson(errorThreadList(e.getMessage()));
            }
        });
    }

    static void pushThreadDetail(Context context, String threadId, String requestId, int limit) {
        EXEC.execute(() -> {
            try {
                if (!canReadSms(context)) {
                    NativeBridge.pushMessageThreadDetailJson(permissionRequiredThreadDetail(
                            requestId, threadId, "READ_SMS permission required."));
                    return;
                }
                int effectiveLimit = limit > 0 ? limit : DEFAULT_DETAIL_LIMIT;
                JSONArray messages = readMessagesForThread(context, threadId, effectiveLimit);
                JSONObject json = new JSONObject();
                json.put("request_id", requestId == null ? "" : requestId);
                json.put("thread_id", threadId == null ? "" : threadId);
                json.put("messages", messages);
                json.put("status", featureStatus("Messages", "Available", ""));
                NativeBridge.pushMessageThreadDetailJson(json.toString());
            } catch (SecurityException se) {
                Log.w(TAG, "SecurityException reading SMS thread detail: " + se.getMessage());
                NativeBridge.pushMessageThreadDetailJson(permissionRequiredThreadDetail(
                        requestId, threadId, se.getMessage()));
            } catch (Exception e) {
                Log.w(TAG, "pushThreadDetail failed", e);
                NativeBridge.pushMessageThreadDetailJson(errorThreadDetail(
                        requestId, threadId, e.getMessage()));
            }
        });
    }

    static void sendMessage(Context context, String requestJson) {
        EXEC.execute(() -> {
            String requestId = "";
            String threadId = null;
            try {
                JSONObject json = new JSONObject(requestJson == null ? "{}" : requestJson);
                requestId = json.optString("request_id", "");
                threadId = json.isNull("thread_id") ? null : json.optString("thread_id", null);
                String body = json.optString("body", "");
                JSONArray recipientsArr = json.optJSONArray("recipients");
                List<String> recipients = new ArrayList<>();
                if (recipientsArr != null) {
                    for (int i = 0; i < recipientsArr.length(); i++) {
                        String value = recipientsArr.optString(i, "").trim();
                        if (!value.isEmpty()) {
                            recipients.add(value);
                        }
                    }
                }
                if (threadId != null) {
                    String fromThread = lookupRecipientFromThread(context, threadId);
                    if (fromThread != null && !fromThread.trim().isEmpty()) {
                        recipients.clear();
                        recipients.add(fromThread);
                    }
                }
                if (body.isEmpty() || recipients.isEmpty()) {
                    NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                            requestId, threadId, "Failed", "Missing body or recipients."));
                    return;
                }
                if (!canSendSms(context)) {
                    NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                            requestId, threadId, "PermissionDenied", "SEND_SMS not granted."));
                    return;
                }
                SmsManager manager = SmsManager.getDefault();
                for (String recipient : recipients) {
                    ArrayList<String> parts = manager.divideMessage(body);
                    manager.sendMultipartTextMessage(recipient, null, parts, null, null);
                }
                NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                        requestId, threadId, "Queued", null));
            } catch (JSONException je) {
                Log.w(TAG, "send request JSON malformed", je);
                NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                        requestId, threadId, "Failed", "Malformed request: " + je.getMessage()));
            } catch (SecurityException se) {
                Log.w(TAG, "SecurityException sending SMS", se);
                NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                        requestId, threadId, "PermissionDenied", se.getMessage()));
            } catch (Exception e) {
                Log.w(TAG, "sendMessage failed", e);
                NativeBridge.pushMessageSendResponseJson(buildSendResponse(
                        requestId, threadId, "Failed", e.getMessage()));
            }
        });
    }

    static void handleThreadOpen(Context context, String openJson) {
        EXEC.execute(() -> {
            try {
                JSONObject json = new JSONObject(openJson == null ? "{}" : openJson);
                String requestId = json.optString("request_id", "");
                String threadId = json.optString("thread_id", "");
                int limit = json.optInt("limit", 0);
                MAIN.post(() -> pushThreadDetail(context, threadId, requestId, limit));
            } catch (JSONException je) {
                Log.w(TAG, "thread open JSON malformed", je);
            }
        });
    }

    // ───────────────────────── live observer ─────────────────────────

    /**
     * Register a ContentObserver on the SMS database so new incoming messages are pushed live
     * without requiring the desktop to re-open the thread. Initialises lastKnownMaxSmsId to
     * the current maximum so historical messages are not re-sent.
     */
    static void startObserver(Context context) {
        stopObserver();
        if (context == null || !canReadSms(context)) {
            return;
        }
        long maxId = queryCurrentMaxSmsId(context);
        lastKnownMaxSmsId = maxId;
        observerContext = context;

        ContentObserver observer = new ContentObserver(MAIN) {
            @Override
            public void onChange(boolean selfChange) {
                EXEC.execute(() -> pushNewInboundMessages(context));
            }
        };
        try {
            context.getContentResolver().registerContentObserver(
                    Telephony.Sms.CONTENT_URI, true, observer);
            smsObserver = observer;
            Log.d(TAG, "SMS ContentObserver registered (lastId=" + maxId + ")");
        } catch (Exception e) {
            Log.w(TAG, "registerContentObserver failed: " + e.getMessage());
        }
    }

    static void stopObserver() {
        ContentObserver observer = smsObserver;
        Context ctx = observerContext;
        smsObserver = null;
        observerContext = null;
        lastKnownMaxSmsId = -1L;
        if (observer != null && ctx != null) {
            try {
                ctx.getContentResolver().unregisterContentObserver(observer);
                Log.d(TAG, "SMS ContentObserver unregistered");
            } catch (Exception ignored) {
            }
        }
    }

    private static long queryCurrentMaxSmsId(Context context) {
        try (Cursor cursor = context.getContentResolver().query(
                Telephony.Sms.CONTENT_URI,
                new String[]{Telephony.Sms._ID},
                null, null,
                Telephony.Sms._ID + " DESC LIMIT 1")) {
            if (cursor != null && cursor.moveToFirst()) {
                int col = cursor.getColumnIndex(Telephony.Sms._ID);
                if (col >= 0) {
                    return cursor.getLong(col);
                }
            }
        } catch (SecurityException ignored) {
        }
        return 0L;
    }

    private static void pushNewInboundMessages(Context context) {
        try {
            if (!canReadSms(context)) {
                return;
            }
            long maxId = lastKnownMaxSmsId;
            if (maxId < 0) {
                return;
            }

            String[] projection = {
                    Telephony.Sms._ID,
                    Telephony.Sms.ADDRESS,
                    Telephony.Sms.BODY,
                    Telephony.Sms.DATE,
                    Telephony.Sms.THREAD_ID,
            };
            String selection = Telephony.Sms._ID + " > ? AND "
                    + Telephony.Sms.TYPE + " = " + Telephony.Sms.MESSAGE_TYPE_INBOX;
            String[] args = {String.valueOf(maxId)};

            long newMaxId = maxId;
            boolean hadNew = false;
            try (Cursor cursor = context.getContentResolver().query(
                    Telephony.Sms.CONTENT_URI, projection, selection, args,
                    Telephony.Sms.DATE + " ASC")) {
                if (cursor == null) {
                    return;
                }
                int idCol = cursor.getColumnIndex(Telephony.Sms._ID);
                int addrCol = cursor.getColumnIndex(Telephony.Sms.ADDRESS);
                int bodyCol = cursor.getColumnIndex(Telephony.Sms.BODY);
                int dateCol = cursor.getColumnIndex(Telephony.Sms.DATE);
                int threadCol = cursor.getColumnIndex(Telephony.Sms.THREAD_ID);

                while (cursor.moveToNext()) {
                    long id = idCol >= 0 ? cursor.getLong(idCol) : -1;
                    String address = addrCol >= 0 ? cursor.getString(addrCol) : null;
                    String body = bodyCol >= 0 ? cursor.getString(bodyCol) : "";
                    long date = dateCol >= 0 ? cursor.getLong(dateCol) : 0L;
                    String threadId = threadCol >= 0 ? cursor.getString(threadCol) : "";

                    try {
                        JSONObject event = new JSONObject();
                        event.put("thread_id", threadId == null ? "" : threadId);
                        event.put("sender", address == null ? "" : address);
                        event.put("body", body == null ? "" : body);
                        event.put("timestamp_unix_ms", date);
                        event.put("attachments", new JSONArray());
                        NativeBridge.pushMessageEventJson(event.toString());
                    } catch (JSONException je) {
                        Log.w(TAG, "pushNewInboundMessages: JSON build failed", je);
                    }

                    if (id > newMaxId) {
                        newMaxId = id;
                    }
                    hadNew = true;
                }
            }
            lastKnownMaxSmsId = newMaxId;

            // Refresh thread list so snippet/unread badge updates in the sidebar.
            if (hadNew) {
                pushThreadList(context);
            }
        } catch (SecurityException se) {
            Log.w(TAG, "pushNewInboundMessages SecurityException: " + se.getMessage());
        } catch (Exception e) {
            Log.w(TAG, "pushNewInboundMessages failed", e);
        }
    }

    // ───────────────────────── helpers ─────────────────────────

    private static boolean canReadSms(Context context) {
        return ContextCompat.checkSelfPermission(context, Manifest.permission.READ_SMS)
                == PackageManager.PERMISSION_GRANTED;
    }

    private static boolean canSendSms(Context context) {
        return ContextCompat.checkSelfPermission(context, Manifest.permission.SEND_SMS)
                == PackageManager.PERMISSION_GRANTED;
    }

    private static String lookupContactName(Context context, String phoneNumber) {
        if (phoneNumber == null || phoneNumber.isEmpty()) {
            return null;
        }
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.READ_CONTACTS)
                != PackageManager.PERMISSION_GRANTED) {
            return null;
        }
        Uri uri = Uri.withAppendedPath(
                ContactsContract.PhoneLookup.CONTENT_FILTER_URI,
                Uri.encode(phoneNumber));
        try (Cursor cursor = context.getContentResolver().query(
                uri,
                new String[]{ContactsContract.PhoneLookup.DISPLAY_NAME},
                null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                String name = cursor.getString(0);
                if (name != null && !name.trim().isEmpty()) {
                    return name;
                }
            }
        } catch (RuntimeException ignored) {
        }
        return null;
    }

    private static JSONArray readThreadSummaries(Context context) throws JSONException {
        JSONArray threads = new JSONArray();
        Map<Long, String> addressByThread = new HashMap<>();
        Uri uri = Telephony.Sms.Conversations.CONTENT_URI;
        String[] projection = {
                Telephony.Sms.Conversations.THREAD_ID,
                Telephony.Sms.Conversations.SNIPPET,
                Telephony.Sms.Conversations.MESSAGE_COUNT,
        };
        try (Cursor cursor = context.getContentResolver().query(uri, projection, null, null,
                Telephony.Sms.Conversations.DEFAULT_SORT_ORDER)) {
            if (cursor == null) {
                return threads;
            }
            int threadCol = cursor.getColumnIndex(Telephony.Sms.Conversations.THREAD_ID);
            int snippetCol = cursor.getColumnIndex(Telephony.Sms.Conversations.SNIPPET);
            int countCol = cursor.getColumnIndex(Telephony.Sms.Conversations.MESSAGE_COUNT);
            while (cursor.moveToNext()) {
                long threadId = threadCol >= 0 ? cursor.getLong(threadCol) : -1;
                String snippet = snippetCol >= 0 ? cursor.getString(snippetCol) : null;
                int messageCount = countCol >= 0 ? cursor.getInt(countCol) : 0;
                String address = addressByThread.computeIfAbsent(
                        threadId, id -> lookupRecipientFromThread(context, String.valueOf(id)));
                int unread = readUnreadCount(context, threadId);
                Long timestamp = readLatestTimestamp(context, threadId);
                String displayName;
                if (address == null || address.isEmpty()) {
                    displayName = "(unknown)";
                } else {
                    String contact = lookupContactName(context, address);
                    displayName = contact != null ? contact : address;
                }
                JSONObject t = new JSONObject();
                t.put("thread_id", String.valueOf(threadId));
                t.put("display_name", displayName);
                if (snippet != null) {
                    t.put("last_message", snippet);
                } else {
                    t.put("last_message", JSONObject.NULL);
                }
                if (timestamp != null) {
                    t.put("timestamp_unix_ms", timestamp);
                } else {
                    t.put("timestamp_unix_ms", JSONObject.NULL);
                }
                t.put("unread_count", unread);
                // message_count is read but unused by the desktop summary; kept here for future use.
                t.put("_message_count", messageCount);
                threads.put(t);
            }
        }
        return threads;
    }

    private static JSONArray readMessagesForThread(Context context, String threadId, int limit)
            throws JSONException {
        JSONArray messages = new JSONArray();
        if (threadId == null || threadId.isEmpty()) {
            return messages;
        }
        Uri uri = Telephony.Sms.CONTENT_URI;
        String[] projection = {
                Telephony.Sms._ID,
                Telephony.Sms.ADDRESS,
                Telephony.Sms.BODY,
                Telephony.Sms.DATE,
                Telephony.Sms.TYPE,
                Telephony.Sms.THREAD_ID,
        };
        String selection = Telephony.Sms.THREAD_ID + " = ?";
        String[] args = { threadId };
        String orderBy = Telephony.Sms.DATE + " DESC LIMIT " + Math.max(1, limit);
        try (Cursor cursor = context.getContentResolver().query(uri, projection, selection, args,
                orderBy)) {
            if (cursor == null) {
                return messages;
            }
            int idCol = cursor.getColumnIndex(Telephony.Sms._ID);
            int addrCol = cursor.getColumnIndex(Telephony.Sms.ADDRESS);
            int bodyCol = cursor.getColumnIndex(Telephony.Sms.BODY);
            int dateCol = cursor.getColumnIndex(Telephony.Sms.DATE);
            int typeCol = cursor.getColumnIndex(Telephony.Sms.TYPE);
            while (cursor.moveToNext()) {
                JSONObject entry = new JSONObject();
                long id = idCol >= 0 ? cursor.getLong(idCol) : -1;
                String addr = addrCol >= 0 ? cursor.getString(addrCol) : null;
                String body = bodyCol >= 0 ? cursor.getString(bodyCol) : "";
                long date = dateCol >= 0 ? cursor.getLong(dateCol) : 0L;
                int type = typeCol >= 0 ? cursor.getInt(typeCol) : Telephony.Sms.MESSAGE_TYPE_INBOX;
                boolean outbound = type == Telephony.Sms.MESSAGE_TYPE_SENT
                        || type == Telephony.Sms.MESSAGE_TYPE_OUTBOX
                        || type == Telephony.Sms.MESSAGE_TYPE_QUEUED;
                entry.put("message_id", String.valueOf(id));
                entry.put("thread_id", threadId);
                entry.put("sender", addr == null ? "" : addr);
                entry.put("body", body == null ? "" : body);
                entry.put("timestamp_unix_ms", date);
                entry.put("direction", outbound ? "Outbound" : "Inbound");
                entry.put("attachments", new JSONArray());
                messages.put(entry);
            }
        }
        return messages;
    }

    private static String lookupRecipientFromThread(Context context, String threadId) {
        if (threadId == null || threadId.isEmpty()) {
            return null;
        }
        Uri uri = Telephony.Sms.CONTENT_URI;
        try (Cursor cursor = context.getContentResolver().query(
                uri,
                new String[]{Telephony.Sms.ADDRESS},
                Telephony.Sms.THREAD_ID + " = ?",
                new String[]{threadId},
                Telephony.Sms.DATE + " DESC LIMIT 1")) {
            if (cursor != null && cursor.moveToFirst()) {
                int col = cursor.getColumnIndex(Telephony.Sms.ADDRESS);
                if (col >= 0) {
                    return cursor.getString(col);
                }
            }
        } catch (SecurityException ignored) {
        }
        return null;
    }

    private static int readUnreadCount(Context context, long threadId) {
        try (Cursor cursor = context.getContentResolver().query(
                Telephony.Sms.Inbox.CONTENT_URI,
                new String[]{Telephony.Sms._ID},
                Telephony.Sms.THREAD_ID + " = ? AND " + Telephony.Sms.READ + " = 0",
                new String[]{String.valueOf(threadId)},
                null)) {
            return cursor == null ? 0 : cursor.getCount();
        } catch (SecurityException ignored) {
            return 0;
        }
    }

    private static Long readLatestTimestamp(Context context, long threadId) {
        try (Cursor cursor = context.getContentResolver().query(
                Telephony.Sms.CONTENT_URI,
                new String[]{Telephony.Sms.DATE},
                Telephony.Sms.THREAD_ID + " = ?",
                new String[]{String.valueOf(threadId)},
                Telephony.Sms.DATE + " DESC LIMIT 1")) {
            if (cursor != null && cursor.moveToFirst()) {
                int col = cursor.getColumnIndex(Telephony.Sms.DATE);
                if (col >= 0) {
                    return cursor.getLong(col);
                }
            }
        } catch (SecurityException ignored) {
        }
        return null;
    }

    private static String permissionRequiredThreadList(String message) {
        try {
            JSONObject json = new JSONObject();
            json.put("threads", new JSONArray());
            json.put("status", featureStatus("Messages", "PermissionRequired",
                    message == null ? "Grant SMS access to mirror conversations." : message));
            return json.toString();
        } catch (JSONException je) {
            return "{}";
        }
    }

    private static String errorThreadList(String message) {
        try {
            JSONObject json = new JSONObject();
            json.put("threads", new JSONArray());
            json.put("status", featureStatus("Messages", "Error",
                    message == null ? "Could not read SMS." : message));
            return json.toString();
        } catch (JSONException je) {
            return "{}";
        }
    }

    private static String permissionRequiredThreadDetail(String requestId, String threadId,
            String message) {
        try {
            JSONObject json = new JSONObject();
            json.put("request_id", requestId == null ? "" : requestId);
            json.put("thread_id", threadId == null ? "" : threadId);
            json.put("messages", new JSONArray());
            json.put("status", featureStatus("Messages", "PermissionRequired",
                    message == null ? "Grant SMS access." : message));
            return json.toString();
        } catch (JSONException je) {
            return "{}";
        }
    }

    private static String errorThreadDetail(String requestId, String threadId, String message) {
        try {
            JSONObject json = new JSONObject();
            json.put("request_id", requestId == null ? "" : requestId);
            json.put("thread_id", threadId == null ? "" : threadId);
            json.put("messages", new JSONArray());
            json.put("status", featureStatus("Messages", "Error",
                    message == null ? "Could not read thread." : message));
            return json.toString();
        } catch (JSONException je) {
            return "{}";
        }
    }

    private static String buildSendResponse(String requestId, String threadId, String result,
            String message) {
        try {
            JSONObject json = new JSONObject();
            json.put("request_id", requestId == null ? "" : requestId);
            if (threadId == null) {
                json.put("thread_id", JSONObject.NULL);
            } else {
                json.put("thread_id", threadId);
            }
            json.put("result", result);
            if (message == null) {
                json.put("message", JSONObject.NULL);
            } else {
                json.put("message", message);
            }
            return json.toString();
        } catch (JSONException je) {
            return "{}";
        }
    }

    private static JSONObject featureStatus(String feature, String state, String message)
            throws JSONException {
        JSONObject json = new JSONObject();
        json.put("feature", feature);
        json.put("state", state);
        json.put("message", message == null ? "" : message);
        return json;
    }
}
