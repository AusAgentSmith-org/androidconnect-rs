package dev.androidconnect;

import android.app.NotificationManager;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.media.AudioManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

/**
 * Bridges OS state changes (WiFi connectivity, audio volume, Bluetooth, DND)
 * to {@link AndroidUtilityBridge#pushDeviceStatus(Context)} so the desktop sees
 * fresh DeviceStatus updates without us polling. Lifecycle has two owners:
 * the authenticated control connection and the optional mirror session.
 */
final class DeviceStateMonitor {
    private static final Object LOCK = new Object();
    private static DeviceStateMonitor INSTANCE;
    private static boolean connectionActive;
    private static boolean sessionActive;
    private static final long STORAGE_REFRESH_MS = 5L * 60L * 1000L;

    private final Context appContext;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private BroadcastReceiver volumeReceiver;
    private BroadcastReceiver bluetoothReceiver;
    private BroadcastReceiver dndReceiver;
    private BroadcastReceiver storageReceiver;
    private ConnectivityManager connectivityManager;
    private ConnectivityManager.NetworkCallback networkCallback;
    private final Runnable storageRefresh = new Runnable() {
        @Override
        public void run() {
            AndroidUtilityBridge.pushStorageStatus(appContext);
            handler.postDelayed(this, STORAGE_REFRESH_MS);
        }
    };

    private DeviceStateMonitor(Context context) {
        this.appContext = context.getApplicationContext();
    }

    static void startForConnection(Context context) {
        if (context == null) {
            return;
        }
        synchronized (LOCK) {
            connectionActive = true;
            ensureStartedLocked(context);
        }
    }

    static void stopForConnection() {
        synchronized (LOCK) {
            connectionActive = false;
            stopIfUnusedLocked();
        }
    }

    static void startForSession(Context context) {
        if (context == null) {
            return;
        }
        synchronized (LOCK) {
            sessionActive = true;
            ensureStartedLocked(context);
        }
    }

    static void stopForSession() {
        synchronized (LOCK) {
            sessionActive = false;
            stopIfUnusedLocked();
        }
    }

    private static void ensureStartedLocked(Context context) {
        if (INSTANCE == null) {
            INSTANCE = new DeviceStateMonitor(context);
            INSTANCE.register();
        }
    }

    private static void stopIfUnusedLocked() {
        if (!connectionActive && !sessionActive && INSTANCE != null) {
            INSTANCE.unregister();
            INSTANCE = null;
        }
    }

    private void register() {
        registerVolumeReceiver();
        registerBluetoothReceiver();
        registerDndReceiver();
        registerStorageReceiver();
        registerNetworkCallback();
        handler.postDelayed(storageRefresh, STORAGE_REFRESH_MS);
    }

    private void unregister() {
        handler.removeCallbacks(storageRefresh);
        if (volumeReceiver != null) {
            safeUnregister(volumeReceiver);
            volumeReceiver = null;
        }
        if (bluetoothReceiver != null) {
            safeUnregister(bluetoothReceiver);
            bluetoothReceiver = null;
        }
        if (dndReceiver != null) {
            safeUnregister(dndReceiver);
            dndReceiver = null;
        }
        if (storageReceiver != null) {
            safeUnregister(storageReceiver);
            storageReceiver = null;
        }
        if (connectivityManager != null && networkCallback != null) {
            try {
                connectivityManager.unregisterNetworkCallback(networkCallback);
            } catch (RuntimeException ignored) {
            }
        }
        networkCallback = null;
        connectivityManager = null;
    }

    private void safeUnregister(BroadcastReceiver receiver) {
        try {
            appContext.unregisterReceiver(receiver);
        } catch (RuntimeException ignored) {
        }
    }

    private void registerVolumeReceiver() {
        volumeReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }
        };
        IntentFilter filter = new IntentFilter("android.media.VOLUME_CHANGED_ACTION");
        registerCompat(volumeReceiver, filter);
    }

    private void registerBluetoothReceiver() {
        bluetoothReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }
        };
        IntentFilter filter = new IntentFilter();
        filter.addAction(BluetoothAdapter.ACTION_STATE_CHANGED);
        filter.addAction(BluetoothDevice.ACTION_ACL_CONNECTED);
        filter.addAction(BluetoothDevice.ACTION_ACL_DISCONNECTED);
        registerCompat(bluetoothReceiver, filter);
    }

    private void registerDndReceiver() {
        dndReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }
        };
        IntentFilter filter = new IntentFilter(
                NotificationManager.ACTION_INTERRUPTION_FILTER_CHANGED);
        registerCompat(dndReceiver, filter);
    }

    private void registerStorageReceiver() {
        storageReceiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context context, Intent intent) {
                AndroidUtilityBridge.pushStorageStatus(appContext);
            }
        };
        IntentFilter filter = new IntentFilter();
        filter.addAction(Intent.ACTION_MEDIA_MOUNTED);
        filter.addAction(Intent.ACTION_MEDIA_UNMOUNTED);
        filter.addDataScheme("file");
        registerCompat(storageReceiver, filter);
    }

    private void registerNetworkCallback() {
        connectivityManager =
                (ConnectivityManager) appContext.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connectivityManager == null) {
            return;
        }
        networkCallback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }

            @Override
            public void onLost(Network network) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }

            @Override
            public void onCapabilitiesChanged(
                    Network network,
                    android.net.NetworkCapabilities capabilities) {
                AndroidUtilityBridge.pushDeviceStatus(appContext);
            }
        };
        try {
            connectivityManager.registerDefaultNetworkCallback(networkCallback);
        } catch (RuntimeException ignored) {
            networkCallback = null;
        }
    }

    private void registerCompat(BroadcastReceiver receiver, IntentFilter filter) {
        if (Build.VERSION.SDK_INT >= 33) {
            appContext.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            appContext.registerReceiver(receiver, filter);
        }
    }
}
