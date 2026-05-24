package dev.androidconnect;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.GestureDescription;
import android.graphics.Path;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

public final class RemoteControlAccessibilityService extends AccessibilityService {
    private static volatile RemoteControlAccessibilityService instance;
    private static final Handler MAIN_HANDLER = new Handler(Looper.getMainLooper());

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
        float sx = NativeBridge.inputScaleX;
        float sy = NativeBridge.inputScaleY;
        float dx = x * sx;
        float dy = y * sy;
        return runOnServiceThread(service -> {
            Path path = new Path();
            path.moveTo(dx, dy);
            GestureDescription gesture = new GestureDescription.Builder()
                    .addStroke(new GestureDescription.StrokeDescription(path, 0, 1))
                    .build();
            return service.dispatchGesture(gesture, null, null);
        });
    }

    public static boolean drag(int startX, int startY, int endX, int endY, long durationMs) {
        float sx = NativeBridge.inputScaleX;
        float sy = NativeBridge.inputScaleY;
        float dsx = startX * sx;
        float dsy = startY * sy;
        float dex = endX * sx;
        float dey = endY * sy;
        return runOnServiceThread(service -> {
            Path path = new Path();
            path.moveTo(dsx, dsy);
            path.lineTo(dex, dey);
            GestureDescription gesture = new GestureDescription.Builder()
                    .addStroke(new GestureDescription.StrokeDescription(path, 0,
                            Math.max(1L, durationMs)))
                    .build();
            return service.dispatchGesture(gesture, null, null);
        });
    }

    public static boolean scroll(int deltaY) {
        if (deltaY == 0) {
            return false;
        }

        return runOnServiceThread(service -> {
            AccessibilityNodeInfo root = service.getRootInActiveWindow();
            if (root == null) {
                return false;
            }

            int action = deltaY > 0
                    ? AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
                    : AccessibilityNodeInfo.ACTION_SCROLL_FORWARD;
            try {
                return performScroll(root, action);
            } finally {
                root.recycle();
            }
        });
    }

    public static boolean inputText(String input) {
        if (input == null || input.isEmpty()) {
            return false;
        }

        return runOnServiceThread(service -> {
            AccessibilityNodeInfo focus = service.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
            if (focus == null) {
                return false;
            }

            try {
                if (!focus.isEditable()) {
                    return false;
                }
                CharSequence currentText = focus.getText();
                StringBuilder nextText = new StringBuilder(
                        currentText == null ? "" : currentText.toString());
                appendInput(nextText, input);

                Bundle args = new Bundle();
                args.putCharSequence(
                        AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                        nextText.toString());
                return focus.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args);
            } finally {
                focus.recycle();
            }
        });
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
        return runOnServiceThread(service -> service.performGlobalAction(action));
    }

    private static boolean performScroll(AccessibilityNodeInfo node, int action) {
        if ((node.getActions() & action) != 0 && node.performAction(action)) {
            return true;
        }

        for (int i = 0; i < node.getChildCount(); i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child == null) {
                continue;
            }
            try {
                if (performScroll(child, action)) {
                    return true;
                }
            } finally {
                child.recycle();
            }
        }
        return false;
    }

    private static void appendInput(StringBuilder text, String input) {
        for (int offset = 0; offset < input.length(); ) {
            int codePoint = input.codePointAt(offset);
            offset += Character.charCount(codePoint);

            if (codePoint == '\b' || codePoint == 0x7f) {
                deleteLastCodePoint(text);
            } else if (codePoint == '\r') {
                text.append('\n');
            } else {
                text.appendCodePoint(codePoint);
            }
        }
    }

    private static void deleteLastCodePoint(StringBuilder text) {
        int length = text.length();
        if (length == 0) {
            return;
        }
        int previous = text.offsetByCodePoints(length, -1);
        text.delete(previous, length);
    }

    private static boolean runOnServiceThread(ServiceAction action) {
        RemoteControlAccessibilityService service = instance;
        if (service == null) {
            return false;
        }

        if (Looper.myLooper() == Looper.getMainLooper()) {
            return action.run(service);
        }

        AtomicBoolean result = new AtomicBoolean(false);
        CountDownLatch done = new CountDownLatch(1);
        MAIN_HANDLER.post(() -> {
            try {
                RemoteControlAccessibilityService current = instance;
                if (current != null) {
                    result.set(action.run(current));
                }
            } finally {
                done.countDown();
            }
        });

        try {
            return done.await(2, TimeUnit.SECONDS) && result.get();
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            return false;
        }
    }

    private interface ServiceAction {
        boolean run(RemoteControlAccessibilityService service);
    }
}
