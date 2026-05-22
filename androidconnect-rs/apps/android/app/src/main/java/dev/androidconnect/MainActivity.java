package dev.androidconnect;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.media.projection.MediaProjectionManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.view.Gravity;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONException;
import org.json.JSONObject;

public final class MainActivity extends Activity {
    private static final int REQUEST_MEDIA_PROJECTION = 1001;
    private static final int REQUEST_NOTIFICATIONS = 1002;
    private static final int REQUEST_PICK_FILE = 1003;
    private static final int REQUEST_MEDIA_READ = 1004;
    private static final int DEFAULT_PORT = 48172;
    private static final long STATUS_REFRESH_MS = 1_000L;

    private TextView statusView;
    private EditText hostField;
    private EditText portField;
    private EditText pairingField;
    private final Handler statusHandler = new Handler(Looper.getMainLooper());
    private final NativeBridge.StatusListener nativeStatusListener =
            new NativeBridge.StatusListener() {
                @Override
                public void onNativeStatusChanged() {
                    updateStatus();
                }
            };
    private final Runnable statusRefresh = new Runnable() {
        @Override
        public void run() {
            updateStatus();
            statusHandler.postDelayed(this, STATUS_REFRESH_MS);
        }
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(createContentView());
        requestNotificationsIfNeeded();
        AndroidUtilityBridge.rememberContext(this);
        AndroidUtilityBridge.handleShareIntent(this, getIntent());
        updateStatus();
    }

    @Override
    protected void onResume() {
        super.onResume();
        NativeBridge.addStatusListener(nativeStatusListener);
        AndroidUtilityBridge.rememberContext(this);
        updateStatus();
        statusHandler.removeCallbacks(statusRefresh);
        statusHandler.postDelayed(statusRefresh, STATUS_REFRESH_MS);
    }

    @Override
    protected void onPause() {
        NativeBridge.removeStatusListener(nativeStatusListener);
        statusHandler.removeCallbacks(statusRefresh);
        super.onPause();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        AndroidUtilityBridge.handleShareIntent(this, intent);
        updateStatus();
    }

    private ScrollView createContentView() {
        int padding = dp(20);

        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setGravity(Gravity.CENTER_HORIZONTAL);
        content.setPadding(padding, padding, padding, padding);

        TextView title = new TextView(this);
        title.setText("AndroidConnect");
        title.setTextSize(26);
        title.setGravity(Gravity.CENTER_HORIZONTAL);
        content.addView(title, matchWrap());

        statusView = new TextView(this);
        statusView.setTextSize(15);
        statusView.setPadding(0, dp(16), 0, dp(20));
        content.addView(statusView, matchWrap());

        hostField = new EditText(this);
        hostField.setHint("Desktop host");
        hostField.setSingleLine(true);
        content.addView(hostField, matchWrap());

        portField = new EditText(this);
        portField.setHint("Desktop port");
        portField.setSingleLine(true);
        portField.setText(String.valueOf(DEFAULT_PORT));
        content.addView(portField, matchWrap());

        pairingField = new EditText(this);
        pairingField.setHint("Pairing code from desktop (first connection)");
        pairingField.setSingleLine(true);
        content.addView(pairingField, matchWrap());

        Button connect = new Button(this);
        connect.setText(getString(R.string.connect_desktop));
        connect.setOnClickListener(view -> connectDesktop());
        content.addView(connect, matchWrap());

        Button disconnect = new Button(this);
        disconnect.setText(getString(R.string.disconnect_desktop));
        disconnect.setOnClickListener(view -> {
            NativeBridge.disconnect();
            updateStatus();
        });
        content.addView(disconnect, matchWrap());

        Button start = new Button(this);
        start.setText(getString(R.string.start_mirroring));
        start.setOnClickListener(view -> requestScreenCapture());
        content.addView(start, matchWrap());

        Button input = new Button(this);
        input.setText(getString(R.string.open_accessibility));
        input.setOnClickListener(view ->
                startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)));
        content.addView(input, matchWrap());

        Button utilityRefresh = new Button(this);
        utilityRefresh.setText(getString(R.string.refresh_utilities));
        utilityRefresh.setOnClickListener(view -> {
            NativeBridge.refreshUtilities(this);
            updateStatus();
        });
        content.addView(utilityRefresh, matchWrap());

        Button notificationAccess = new Button(this);
        notificationAccess.setText(getString(R.string.open_notification_access));
        notificationAccess.setOnClickListener(view ->
                startActivity(new Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)));
        content.addView(notificationAccess, matchWrap());

        Button clipboard = new Button(this);
        clipboard.setText(getString(R.string.sync_clipboard));
        clipboard.setOnClickListener(view -> {
            AndroidUtilityBridge.pushForegroundClipboard(this);
            updateStatus();
        });
        content.addView(clipboard, matchWrap());

        Button pickFile = new Button(this);
        pickFile.setText(getString(R.string.send_file));
        pickFile.setOnClickListener(view -> pickFileForDesktop());
        content.addView(pickFile, matchWrap());

        Button mediaPermission = new Button(this);
        mediaPermission.setText(getString(R.string.grant_media_access));
        mediaPermission.setOnClickListener(view -> requestMediaReadIfNeeded());
        content.addView(mediaPermission, matchWrap());

        Button stop = new Button(this);
        stop.setText(getString(R.string.stop_mirroring));
        stop.setOnClickListener(view -> {
            Intent intent = new Intent(this, MirrorService.class);
            intent.setAction(MirrorService.ACTION_STOP);
            startService(intent);
            updateStatus();
        });
        content.addView(stop, matchWrap());

        ScrollView scroll = new ScrollView(this);
        scroll.addView(content, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));
        return scroll;
    }

    private void connectDesktop() {
        if (!NativeBridge.isAvailable()) {
            showError("Native Rust library is not packaged yet.");
            return;
        }

        String host = hostField.getText().toString().trim();
        if (host.isEmpty()) {
            showError("Enter the desktop host or IP address.");
            return;
        }

        int port;
        try {
            port = Integer.parseInt(portField.getText().toString().trim());
        } catch (NumberFormatException error) {
            showError("Enter a valid desktop port.");
            return;
        }

        String pairingCode = pairingField.getText().toString().trim();
        updateStatus("Connecting to " + host + ":" + port + "...");
        new Thread(() -> {
            boolean connected = NativeBridge.connect(this, host, port, pairingCode);
            runOnUiThread(() -> {
                Toast.makeText(
                        this,
                        connected ? "Desktop connected" : "Desktop connection failed",
                        Toast.LENGTH_SHORT).show();
                updateStatus();
            });
        }, "AndroidConnectDesktopConnect").start();
    }

    private void requestScreenCapture() {
        MediaProjectionManager manager =
                (MediaProjectionManager) getSystemService(MEDIA_PROJECTION_SERVICE);
        if (manager == null) {
            showError("MediaProjection is unavailable on this device.");
            return;
        }
        startActivityForResult(manager.createScreenCaptureIntent(), REQUEST_MEDIA_PROJECTION);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_MEDIA_PROJECTION) {
            if (requestCode == REQUEST_PICK_FILE && resultCode == RESULT_OK && data != null) {
                Uri uri = data.getData();
                AndroidUtilityBridge.handlePickedUri(this, uri);
                updateStatus();
            }
            return;
        }
        if (resultCode != RESULT_OK || data == null) {
            updateStatus();
            return;
        }

        Intent service = new Intent(this, MirrorService.class);
        service.putExtra(MirrorService.EXTRA_RESULT_CODE, resultCode);
        service.putExtra(MirrorService.EXTRA_RESULT_DATA, data);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(service);
        } else {
            startService(service);
        }
        updateStatus();
    }

    private void pickFileForDesktop() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        startActivityForResult(intent, REQUEST_PICK_FILE);
    }

    private void requestNotificationsIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestPermissions(new String[] { Manifest.permission.POST_NOTIFICATIONS },
                    REQUEST_NOTIFICATIONS);
        }
    }

    private void requestMediaReadIfNeeded() {
        if (Build.VERSION.SDK_INT >= 33) {
            requestPermissions(new String[] {
                            Manifest.permission.READ_MEDIA_IMAGES,
                            Manifest.permission.READ_MEDIA_VIDEO
                    },
                    REQUEST_MEDIA_READ);
        } else if (Build.VERSION.SDK_INT >= 23
                && checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE)
                != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { Manifest.permission.READ_EXTERNAL_STORAGE },
                    REQUEST_MEDIA_READ);
        } else {
            AndroidUtilityBridge.pushPhotosPermissionStatus();
        }
    }

    private void updateStatus() {
        updateStatus(null);
    }

    private void updateStatus(String prefix) {
        if (statusView == null) {
            return;
        }

        String status = buildStatusText();
        statusView.setText(prefix == null ? status : prefix + "\n" + status);
    }

    private String buildStatusText() {
        String inputState = RemoteControlAccessibilityService.isRunning()
                ? "enabled"
                : "disabled";
        if (!NativeBridge.isAvailable()) {
            return "Capture: ready\nDesktop: disconnected\nPairing: unavailable"
                    + "\nInput service: " + inputState
                    + "\nRust: native library not packaged yet";
        }

        String stats = NativeBridge.statsJson();
        try {
            JSONObject json = new JSONObject(stats);
            boolean running = json.optBoolean("running", false);
            boolean connected = json.optBoolean("connected", false);
            boolean reconnecting = json.optBoolean("reconnecting", false);
            boolean inputAuthenticated = json.optBoolean("input_authenticated", false);
            long encodedFrames = json.optLong("encoded_frames", 0);
            long sentBytes = json.optLong("sent_bytes", 0);
            long receivedPings = json.optLong("received_pings", 0);
            long sentPongs = json.optLong("sent_pongs", 0);
            long sentUtilities = json.optLong("sent_utility_envelopes", 0);
            long incomingTransfers = json.optLong("incoming_transfer_count", 0);
            long reconnectAttempts = json.optLong("reconnect_attempts", 0);
            String connectedTo = json.optString("connected_to", "");
            String pairedDesktopName = json.optString("paired_desktop_name", "");
            String pairedDesktopId = json.optString("paired_desktop_id", "");
            long trustedDesktopCount = json.optLong("trusted_desktop_count", 0);
            String lastReconnectError = json.optString("last_reconnect_error", "");
            String lastError = json.optString("last_error", "");

            StringBuilder status = new StringBuilder();
            status.append("Capture: ");
            status.append(running ? "running" : "ready");
            status.append(" (");
            status.append(encodedFrames);
            status.append(" frames)");

            status.append("\nDesktop: ");
            if (connected) {
                status.append("connected");
                if (!connectedTo.isEmpty()) {
                    status.append(" to ");
                    status.append(connectedTo);
                }
            } else if (reconnecting) {
                status.append("reconnecting");
                if (reconnectAttempts > 0) {
                    status.append(" (attempt ");
                    status.append(reconnectAttempts);
                    status.append(")");
                }
            } else {
                status.append("disconnected");
            }

            status.append("\nPairing: ");
            if (inputAuthenticated) {
                status.append("authenticated");
                if (!pairedDesktopName.isEmpty()) {
                    status.append(" with ");
                    status.append(pairedDesktopName);
                } else if (!pairedDesktopId.isEmpty()) {
                    status.append(" with ");
                    status.append(pairedDesktopId);
                }
            } else if (connected || reconnecting) {
                status.append("pending");
            } else {
                status.append("not connected");
            }
            status.append(" (");
            status.append(trustedDesktopCount);
            status.append(" trusted)");

            status.append("\nInput service: ");
            status.append(inputState);
            status.append("\nHeartbeat: ");
            status.append(receivedPings);
            status.append(" pings / ");
            status.append(sentPongs);
            status.append(" pongs");
            status.append("\nSent: ");
            status.append(formatBytes(sentBytes));
            status.append(" / ");
            status.append(sentUtilities);
            status.append(" utility messages");
            status.append("\nTransfers: ");
            status.append(incomingTransfers);
            status.append(" active incoming");

            if (!lastError.isEmpty()) {
                status.append("\nLast error: ");
                status.append(lastError);
            }
            if (!lastReconnectError.isEmpty()) {
                status.append("\nReconnect: ");
                status.append(lastReconnectError);
            }

            return status.toString();
        } catch (JSONException error) {
            return "Capture: ready\nDesktop: unknown\nPairing: unknown"
                    + "\nInput service: " + inputState
                    + "\nRust: " + stats;
        }
    }

    private String formatBytes(long bytes) {
        if (bytes < 1024) {
            return bytes + " B";
        }
        long kib = bytes / 1024;
        if (kib < 1024) {
            return kib + " KiB";
        }
        long mib = kib / 1024;
        return mib + " MiB";
    }

    private void showError(String message) {
        new AlertDialog.Builder(this)
                .setTitle(getString(R.string.app_name))
                .setMessage(message)
                .setPositiveButton(android.R.string.ok, null)
                .show();
    }

    private LinearLayout.LayoutParams matchWrap() {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        params.setMargins(0, 0, 0, dp(12));
        return params;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
