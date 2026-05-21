package dev.androidconnect;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.GestureDescription;
import android.graphics.Path;
import android.view.accessibility.AccessibilityEvent;

public final class RemoteControlAccessibilityService extends AccessibilityService {
    private static volatile RemoteControlAccessibilityService instance;

    @Override
    protected void onServiceConnected() {
        instance = this;
    }

    @Override
    public void onDestroy() {
        if (instance == this) {
            instance = null;
        }
        super.onDestroy();
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {
    }

    @Override
    public void onInterrupt() {
    }

    public static boolean isRunning() {
        return instance != null;
    }

    public static boolean tap(int x, int y) {
        RemoteControlAccessibilityService service = instance;
        if (service == null) {
            return false;
        }

        Path path = new Path();
        path.moveTo(x, y);
        GestureDescription gesture = new GestureDescription.Builder()
                .addStroke(new GestureDescription.StrokeDescription(path, 0, 1))
                .build();
        return service.dispatchGesture(gesture, null, null);
    }

    public static boolean drag(int startX, int startY, int endX, int endY, long durationMs) {
        RemoteControlAccessibilityService service = instance;
        if (service == null) {
            return false;
        }

        Path path = new Path();
        path.moveTo(startX, startY);
        path.lineTo(endX, endY);
        GestureDescription gesture = new GestureDescription.Builder()
                .addStroke(new GestureDescription.StrokeDescription(path, 0,
                        Math.max(1L, durationMs)))
                .build();
        return service.dispatchGesture(gesture, null, null);
    }

    public static boolean systemBack() {
        return performGlobal(GLOBAL_ACTION_BACK);
    }

    public static boolean systemHome() {
        return performGlobal(GLOBAL_ACTION_HOME);
    }

    public static boolean systemRecents() {
        return performGlobal(GLOBAL_ACTION_RECENTS);
    }

    public static boolean lockScreen() {
        return performGlobal(GLOBAL_ACTION_LOCK_SCREEN);
    }

    private static boolean performGlobal(int action) {
        RemoteControlAccessibilityService service = instance;
        return service != null && service.performGlobalAction(action);
    }
}
