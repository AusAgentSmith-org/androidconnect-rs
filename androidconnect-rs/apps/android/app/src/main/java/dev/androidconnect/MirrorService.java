package dev.androidconnect;

import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.IBinder;
import android.util.Log;

public final class MirrorService extends Service {
    public static final String ACTION_STOP = "dev.androidconnect.action.STOP";
    public static final String EXTRA_RESULT_CODE = "dev.androidconnect.extra.RESULT_CODE";
    public static final String EXTRA_RESULT_DATA = "dev.androidconnect.extra.RESULT_DATA";

    private static final String TAG = "AndroidConnectMirror";
    private static final String CHANNEL_ID = "mirroring";
    private static final int NOTIFICATION_ID = 17;

    private ScreenEncoder screenEncoder;

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stopSelf();
            return START_NOT_STICKY;
        }

        startAsForegroundService();

        int resultCode = intent != null
                ? intent.getIntExtra(EXTRA_RESULT_CODE, Activity.RESULT_CANCELED)
                : Activity.RESULT_CANCELED;
        Intent resultData = intent != null ? intent.getParcelableExtra(EXTRA_RESULT_DATA) : null;
        if (resultCode != Activity.RESULT_OK || resultData == null) {
            Log.w(TAG, "missing MediaProjection grant");
            stopSelf();
            return START_NOT_STICKY;
        }

        startMirroring(resultCode, resultData);
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        stopMirroring();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private void startMirroring(int resultCode, Intent resultData) {
        stopMirroring();

        MediaProjectionManager manager =
                (MediaProjectionManager) getSystemService(MEDIA_PROJECTION_SERVICE);
        if (manager == null) {
            Log.e(TAG, "MediaProjectionManager unavailable");
            stopSelf();
            return;
        }

        MediaProjection projection = manager.getMediaProjection(resultCode, resultData);
        if (projection == null) {
            Log.e(TAG, "MediaProjection grant rejected");
            stopSelf();
            return;
        }

        NativeBridge.startSession(this);
        screenEncoder = new ScreenEncoder(this, projection, this::stopSelf);
        try {
            screenEncoder.start();
        } catch (Exception error) {
            Log.e(TAG, "failed to start screen encoder", error);
            stopSelf();
        }
    }

    private void stopMirroring() {
        if (screenEncoder != null) {
            screenEncoder.stop();
            screenEncoder = null;
        }
        NativeBridge.stopSession();
    }

    private void startAsForegroundService() {
        createNotificationChannel();
        Notification notification = createNotification();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
        } else {
            startForeground(NOTIFICATION_ID, notification);
        }
    }

    private void createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return;
        }
        NotificationManager manager =
                (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) {
            return;
        }
        NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID,
                getString(R.string.notification_channel),
                NotificationManager.IMPORTANCE_LOW);
        manager.createNotificationChannel(channel);
    }

    private Notification createNotification() {
        Intent stop = new Intent(this, MirrorService.class);
        stop.setAction(ACTION_STOP);

        int pendingFlags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            pendingFlags |= PendingIntent.FLAG_IMMUTABLE;
        }
        PendingIntent stopIntent = PendingIntent.getService(this, 0, stop, pendingFlags);

        Notification.Builder builder = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
                ? new Notification.Builder(this, CHANNEL_ID)
                : new Notification.Builder(this);
        return builder
                .setSmallIcon(android.R.drawable.presence_video_online)
                .setContentTitle(getString(R.string.notification_title))
                .setContentText(getString(R.string.notification_text))
                .setOngoing(true)
                .addAction(android.R.drawable.ic_menu_close_clear_cancel,
                        getString(R.string.stop_mirroring), stopIntent)
                .build();
    }
}
