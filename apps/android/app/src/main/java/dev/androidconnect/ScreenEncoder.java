package dev.androidconnect;

import android.content.Context;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.media.projection.MediaProjection;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.util.DisplayMetrics;
import android.util.Log;
import android.view.Surface;

import android.os.Build;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.concurrent.atomic.AtomicBoolean;

public final class ScreenEncoder {
    private static final String TAG = "AndroidConnectEncoder";
    private static final int I_FRAME_INTERVAL_SECONDS = 1;

    // Emulators have no hardware H264 encoder; reduce load so the desktop decoder can keep up.
    private static final boolean IS_EMULATOR =
            Build.HARDWARE.equals("goldfish") || Build.HARDWARE.equals("ranchu")
            || Build.FINGERPRINT.startsWith("generic") || Build.FINGERPRINT.startsWith("unknown")
            || Build.MODEL.contains("Emulator") || Build.MODEL.contains("Android SDK built for x86");
    private static final int FRAME_RATE = IS_EMULATOR ? 15 : 30;
    private static final int MAX_SHORT_EDGE = IS_EMULATOR ? 720 : 1080;

    private final Context context;
    private final MediaProjection projection;
    private final Runnable onProjectionStopped;
    private final AtomicBoolean stopped = new AtomicBoolean(true);

    private MediaProjection.Callback projectionCallback;
    private MediaCodec codec;
    private Surface inputSurface;
    private VirtualDisplay virtualDisplay;
    private HandlerThread drainThread;

    public ScreenEncoder(Context context, MediaProjection projection, Runnable onProjectionStopped) {
        this.context = context.getApplicationContext();
        this.projection = projection;
        this.onProjectionStopped = onProjectionStopped;
    }

    public void start() throws IOException {
        if (!stopped.compareAndSet(true, false)) {
            return;
        }

        DisplayMetrics metrics = displayMetrics();
        int[] scaled = scaleToMax(metrics.widthPixels, metrics.heightPixels, MAX_SHORT_EDGE);
        int width = scaled[0];
        int height = scaled[1];
        int dpi = metrics.densityDpi;

        NativeBridge.inputScaleX = (float) metrics.widthPixels / width;
        NativeBridge.inputScaleY = (float) metrics.heightPixels / height;

        Log.i(TAG, (IS_EMULATOR ? "[emulator] " : "") + "display: "
                + width + "x" + height + " dpi=" + dpi + " fps=" + FRAME_RATE
                + " inputScale=" + NativeBridge.inputScaleX + "x" + NativeBridge.inputScaleY);
        if (width <= 0 || height <= 0) {
            throw new IllegalStateException("invalid display dimensions: " + width + "x" + height);
        }

        MediaFormat format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC,
                width, height);
        format.setInteger(MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface);
        format.setInteger(MediaFormat.KEY_BIT_RATE, bitrateFor(width, height));
        format.setInteger(MediaFormat.KEY_FRAME_RATE, FRAME_RATE);
        format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, I_FRAME_INTERVAL_SECONDS);

        codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC);
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
        inputSurface = codec.createInputSurface();
        codec.start();

        projectionCallback = new MediaProjection.Callback() {
            @Override
            public void onStop() {
                Log.i(TAG, "MediaProjection stopped by the system");
                stop();
                onProjectionStopped.run();
            }
        };
        projection.registerCallback(projectionCallback, new Handler(Looper.getMainLooper()));

        virtualDisplay = projection.createVirtualDisplay(
                "AndroidConnect Mirror",
                width,
                height,
                dpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                inputSurface,
                null,
                null);

        NativeBridge.pushVideoFormat(width, height, dpi, FRAME_RATE, 0);

        drainThread = new HandlerThread("AndroidConnectEncoderDrain");
        drainThread.start();
        new Handler(drainThread.getLooper()).post(this::drainEncoder);
    }

    public void stop() {
        if (!stopped.compareAndSet(false, true)) {
            return;
        }

        if (virtualDisplay != null) {
            virtualDisplay.release();
            virtualDisplay = null;
        }
        if (drainThread != null) {
            HandlerThread thread = drainThread;
            drainThread = null;
            thread.quitSafely();
            try {
                thread.join(500);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
            }
        }
        if (codec != null) {
            try {
                codec.stop();
            } catch (IllegalStateException ignored) {
            }
            codec.release();
            codec = null;
        }
        if (inputSurface != null) {
            inputSurface.release();
            inputSurface = null;
        }
        if (projectionCallback != null) {
            try {
                projection.unregisterCallback(projectionCallback);
            } catch (RuntimeException ignored) {
            }
            projectionCallback = null;
        }
        try {
            projection.stop();
        } catch (RuntimeException ignored) {
        }
    }

    private void drainEncoder() {
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();

        while (!stopped.get()) {
            MediaCodec activeCodec = codec;
            if (activeCodec == null) {
                return;
            }

            int index;
            try {
                index = activeCodec.dequeueOutputBuffer(info, 10_000);
            } catch (IllegalStateException error) {
                if (!stopped.get()) {
                    Log.w(TAG, "encoder stopped while draining", error);
                }
                return;
            }

            if (index == MediaCodec.INFO_TRY_AGAIN_LATER) {
                continue;
            }
            if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                Log.i(TAG, "output format changed: " + activeCodec.getOutputFormat());
                continue;
            }
            if (index < 0) {
                continue;
            }

            try {
                ByteBuffer output = activeCodec.getOutputBuffer(index);
                if (output != null && info.size > 0) {
                    byte[] bytes = new byte[info.size];
                    output.position(info.offset);
                    output.limit(info.offset + info.size);
                    output.get(bytes);
                    NativeBridge.pushVideoFrame(bytes, info.presentationTimeUs, info.flags);
                }
            } finally {
                activeCodec.releaseOutputBuffer(index, false);
            }
        }
    }

    private DisplayMetrics displayMetrics() {
        DisplayMetrics metrics = new DisplayMetrics();
        DisplayManager displayManager =
                (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);
        if (displayManager != null) {
            android.view.Display display =
                    displayManager.getDisplay(android.view.Display.DEFAULT_DISPLAY);
            if (display != null) {
                display.getRealMetrics(metrics);
                return metrics;
            }
        }
        metrics.setTo(context.getResources().getDisplayMetrics());
        return metrics;
    }

    private int bitrateFor(int width, int height) {
        long pixels = (long) width * (long) height;
        long bitrate = pixels * FRAME_RATE / 4;
        return (int) Math.max(2_000_000L, Math.min(16_000_000L, bitrate));
    }

    private int[] scaleToMax(int w, int h, int maxShortEdge) {
        int shortEdge = Math.min(w, h);
        if (shortEdge <= maxShortEdge) {
            return new int[]{ even(w), even(h) };
        }
        float scale = (float) maxShortEdge / shortEdge;
        return new int[]{ even(Math.round(w * scale)), even(Math.round(h * scale)) };
    }

    private int even(int value) {
        return value % 2 == 0 ? value : value - 1;
    }
}
