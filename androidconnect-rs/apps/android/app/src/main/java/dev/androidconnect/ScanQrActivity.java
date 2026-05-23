package dev.androidconnect;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Base64;
import android.util.Log;
import android.util.Size;
import android.view.Gravity;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import androidx.annotation.OptIn;
import androidx.camera.core.CameraSelector;
import androidx.camera.core.ExperimentalGetImage;
import androidx.camera.core.ImageAnalysis;
import androidx.camera.core.ImageProxy;
import androidx.camera.core.Preview;
import androidx.camera.lifecycle.ProcessCameraProvider;
import androidx.camera.view.PreviewView;
import androidx.lifecycle.LifecycleOwner;
import androidx.lifecycle.LifecycleRegistry;

import com.google.common.util.concurrent.ListenableFuture;
import com.google.mlkit.vision.barcode.BarcodeScanner;
import com.google.mlkit.vision.barcode.BarcodeScannerOptions;
import com.google.mlkit.vision.barcode.BarcodeScanning;
import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.common.InputImage;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Hosts a CameraX preview + ML Kit barcode pipeline. When a QR encoding an
 * {@code androidconnect://pair?...} URI is decoded we extract the pairing
 * fields and pass them back to {@link MainActivity} via Intent extras so the
 * existing pairing flow can pre-fill and auto-submit.
 */
public final class ScanQrActivity extends Activity implements LifecycleOwner {
    private static final String TAG = "ScanQrActivity";
    private static final int REQUEST_CAMERA = 7331;
    private static final String QR_PREFIX = "androidconnect://pair?";

    public static final String EXTRA_BIND = "extra_bind";
    public static final String EXTRA_PAIRING_CODE = "extra_pairing_code";
    public static final String EXTRA_DESKTOP_ID = "extra_desktop_id";
    public static final String EXTRA_DESKTOP_NAME = "extra_desktop_name";

    private final LifecycleRegistry lifecycleRegistry = new LifecycleRegistry(this);
    private final AtomicBoolean handled = new AtomicBoolean(false);
    private PreviewView previewView;
    private TextView statusView;
    private ExecutorService analyzerExecutor;
    private BarcodeScanner scanner;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.CREATED);
        setTitle(getString(R.string.scan_to_connect));

        FrameLayout root = new FrameLayout(this);
        previewView = new PreviewView(this);
        root.addView(previewView, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT));

        LinearLayout statusBar = new LinearLayout(this);
        statusBar.setOrientation(LinearLayout.VERTICAL);
        statusBar.setBackgroundColor(0xCC000000);
        statusBar.setPadding(dp(16), dp(16), dp(16), dp(16));

        statusView = new TextView(this);
        statusView.setTextColor(Color.WHITE);
        statusView.setText("Point the camera at the QR code shown on your desktop.");
        statusBar.addView(statusView, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        FrameLayout.LayoutParams statusParams = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        statusParams.gravity = Gravity.BOTTOM;
        root.addView(statusBar, statusParams);

        setContentView(root);

        analyzerExecutor = Executors.newSingleThreadExecutor();
        scanner = BarcodeScanning.getClient(new BarcodeScannerOptions.Builder()
                .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                .build());

        if (checkSelfPermission(Manifest.permission.CAMERA)
                != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { Manifest.permission.CAMERA }, REQUEST_CAMERA);
        } else {
            bindCamera();
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.STARTED);
    }

    @Override
    protected void onResume() {
        super.onResume();
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.RESUMED);
    }

    @Override
    protected void onPause() {
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.STARTED);
        super.onPause();
    }

    @Override
    protected void onStop() {
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.CREATED);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
        lifecycleRegistry.setCurrentState(androidx.lifecycle.Lifecycle.State.DESTROYED);
        if (analyzerExecutor != null) {
            analyzerExecutor.shutdownNow();
        }
        if (scanner != null) {
            scanner.close();
        }
        super.onDestroy();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
                                            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode != REQUEST_CAMERA) {
            return;
        }
        if (grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
            bindCamera();
        } else {
            Toast.makeText(this, "Camera permission is required to scan QR codes",
                    Toast.LENGTH_LONG).show();
            setResult(RESULT_CANCELED);
            finish();
        }
    }

    @Override
    public androidx.lifecycle.Lifecycle getLifecycle() {
        return lifecycleRegistry;
    }

    private void bindCamera() {
        ListenableFuture<ProcessCameraProvider> future =
                ProcessCameraProvider.getInstance(this);
        future.addListener(() -> {
            try {
                ProcessCameraProvider provider = future.get();
                Preview preview = new Preview.Builder().build();
                preview.setSurfaceProvider(previewView.getSurfaceProvider());

                ImageAnalysis analysis = new ImageAnalysis.Builder()
                        .setTargetResolution(new Size(1280, 720))
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build();
                analysis.setAnalyzer(analyzerExecutor, this::analyze);

                CameraSelector selector = CameraSelector.DEFAULT_BACK_CAMERA;

                provider.unbindAll();
                provider.bindToLifecycle(this, selector, preview, analysis);
            } catch (Exception error) {
                Log.e(TAG, "camera bind failed", error);
                Toast.makeText(this, "Camera unavailable: " + error.getMessage(),
                        Toast.LENGTH_LONG).show();
                setResult(RESULT_CANCELED);
                finish();
            }
        }, mainExecutor);
    }

    private final java.util.concurrent.Executor mainExecutor = new java.util.concurrent.Executor() {
        private final Handler handler = new Handler(Looper.getMainLooper());

        @Override
        public void execute(Runnable runnable) {
            handler.post(runnable);
        }
    };

    @OptIn(markerClass = ExperimentalGetImage.class)
    private void analyze(ImageProxy imageProxy) {
        if (handled.get()) {
            imageProxy.close();
            return;
        }
        android.media.Image image = imageProxy.getImage();
        if (image == null) {
            imageProxy.close();
            return;
        }
        InputImage input = InputImage.fromMediaImage(
                image, imageProxy.getImageInfo().getRotationDegrees());
        scanner.process(input)
                .addOnSuccessListener(this::onBarcodes)
                .addOnFailureListener(error -> Log.w(TAG, "barcode failure", error))
                .addOnCompleteListener(task -> imageProxy.close());
    }

    private void onBarcodes(List<Barcode> barcodes) {
        if (handled.get()) {
            return;
        }
        for (Barcode barcode : barcodes) {
            String raw = barcode.getRawValue();
            if (raw == null || !raw.startsWith(QR_PREFIX)) {
                continue;
            }
            if (handlePayload(raw)) {
                return;
            }
        }
    }

    private boolean handlePayload(String uri) {
        try {
            String query = uri.substring(QR_PREFIX.length());
            String data = null;
            for (String pair : query.split("&")) {
                int eq = pair.indexOf('=');
                if (eq <= 0) {
                    continue;
                }
                String key = pair.substring(0, eq);
                String value = pair.substring(eq + 1);
                if ("data".equals(key)) {
                    data = value;
                    break;
                }
            }
            if (data == null || data.isEmpty()) {
                runOnUiThread(() -> statusView.setText(
                        "QR is missing the data field — try another code."));
                return false;
            }
            byte[] decoded = Base64.decode(
                    data,
                    Base64.URL_SAFE | Base64.NO_PADDING | Base64.NO_WRAP);
            JSONObject payload = new JSONObject(new String(decoded, "UTF-8"));

            JSONArray addresses = payload.optJSONArray("addresses");
            if (addresses == null || addresses.length() == 0) {
                runOnUiThread(() -> statusView.setText(
                        "QR has no listen addresses — try regenerating it."));
                return false;
            }
            String bind = addresses.getString(0);
            String pairingToken = payload.optString("pairing_token", "");
            String desktopId = payload.optString("desktop_id", "");
            String desktopName = payload.optString("desktop_name", "");

            if (!handled.compareAndSet(false, true)) {
                return true;
            }

            Intent result = new Intent();
            result.putExtra(EXTRA_BIND, bind);
            result.putExtra(EXTRA_PAIRING_CODE, pairingToken);
            result.putExtra(EXTRA_DESKTOP_ID, desktopId);
            result.putExtra(EXTRA_DESKTOP_NAME, desktopName);
            // The native side also accepts a full URI for traceability.
            result.setData(Uri.parse(uri));
            setResult(RESULT_OK, result);
            finish();
            return true;
        } catch (RuntimeException | java.io.UnsupportedEncodingException
                 | org.json.JSONException error) {
            Log.w(TAG, "QR parse failed", error);
            runOnUiThread(() -> statusView.setText(
                    "Couldn't parse QR code — make sure it's from AndroidConnect."));
            return false;
        }
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
