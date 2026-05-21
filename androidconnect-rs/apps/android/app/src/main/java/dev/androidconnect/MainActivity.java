package dev.androidconnect;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.media.projection.MediaProjectionManager;
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
    private static final int DEFAULT_PORT = 48172;
    private static final long STATUS_REFRESH_MS = 1_000L;

    private TextView statusView;
    private EditText hostField;
    private EditText portField;
    private EditText pairingField;
    private final Handler statusHandler = new Handler(Looper.getMainLooper());
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
        updateStatus();
    }

    @Override
    protected void onResume() {
        super.onResume();
        updateStatus();
        statusHandler.removeCallbacks(statusRefresh);
        statusHandler.postDelayed(statusRefresh, STATUS_REFRESH_MS);
    }

    @Override
    protected void onPause() {
        statusHandler.removeCallbacks(statusRefresh);
        super.onPause();
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
        pairingField.setHint("Pairing code from desktop");
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
        if (pairingCode.isEmpty()) {
            showError("Enter the pairing code shown by the desktop viewer.");
            return;
        }

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

    private void requestNotificationsIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestPermissions(new String[] { Manifest.permission.POST_NOTIFICATIONS },
                    REQUEST_NOTIFICATIONS);
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
            boolean inputAuthenticated = json.optBoolean("input_authenticated", false);
            long encodedFrames = json.optLong("encoded_frames", 0);
            long sentBytes = json.optLong("sent_bytes", 0);
            String connectedTo = json.optString("connected_to", "");
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
            } else {
                status.append("disconnected");
            }

            status.append("\nPairing: ");
            if (inputAuthenticated) {
                status.append("authenticated");
            } else if (connected) {
                status.append("pending");
            } else {
                status.append("not connected");
            }

            status.append("\nInput service: ");
            status.append(inputState);
            status.append("\nSent: ");
            status.append(formatBytes(sentBytes));

            if (!lastError.isEmpty()) {
                status.append("\nLast error: ");
                status.append(lastError);
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
