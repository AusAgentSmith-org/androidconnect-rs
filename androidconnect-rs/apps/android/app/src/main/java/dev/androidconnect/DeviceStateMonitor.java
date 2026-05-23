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

/**
 * Bridges OS state changes (WiFi connectivity, audio volume, Bluetooth, DND)
 * to {@link AndroidUtilityBridge#pushDeviceStatus(Context)} so the desktop sees
 * fresh DeviceStatus updates without us polling. Lifecycle is owned by
 * {@link NativeBridge#startSession(Context)} / {@link NativeBridge#stopSession()}.
 */
final class DeviceStateMonitor {
    private static final Object LOCK = new Object();
    private static DeviceStateMonitor INSTANCE;

    private final Context appContext;
    private BroadcastReceiver volumeReceiver;
    private BroadcastReceiver bluetoothReceiver;
    private BroadcastReceiver dndReceiver;
    private ConnectivityManager connectivityManager;
    private ConnectivityManager.NetworkCallback networkCallback;

    private DeviceStateMonitor(Context context) {
        this.appContext = context.getApplicationContext();
    }

    static void start(Context context) {
        if (context == null) {
            return;
        }
        synchronized (LOCK) {
            if (INSTANCE != null) {
                return;
            }
            INSTANCE = new DeviceStateMonitor(context);
            INSTANCE.register();
        }
    }

    static void stop() {
        synchronized (LOCK) {
            if (INSTANCE != null) {
                INSTANCE.unregister();
                INSTANCE = null;
            }
        }
    }

    private void register() {
        registerVolumeReceiver();
        registerBluetoothReceiver();
        registerDndReceiver();
        registerNetworkCallback();
    }

    private void unregister() {
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
