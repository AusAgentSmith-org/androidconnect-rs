package dev.androidconnect;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.provider.Settings;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.util.EnumMap;
import java.util.Map;

/**
 * First-run permission walkthrough. Shown automatically before the user reaches
 * {@link MainActivity} the first time and re-openable from MainActivity at any
 * point. Renders one card per permission with a live status chip and a button
 * that either triggers the runtime prompt or deep-links to the right Settings
 * screen.
 */
public final class OnboardingActivity extends Activity {
    public static final String PREFS_NAME = "androidconnect_onboarding";
    public static final String KEY_COMPLETED = "completed";

    private static final int REQUEST_RUNTIME_PERMISSION = 9101;

    private PermissionStatusRepository repository;
    private final Map<PermissionStatusRepository.PermissionId, CardViews> cards =
            new EnumMap<>(PermissionStatusRepository.PermissionId.class);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setTitle(getString(R.string.onboarding_title));
        repository = new PermissionStatusRepository(this);
        setContentView(buildContentView());
        refresh();
    }

    @Override
    protected void onResume() {
        super.onResume();
        refresh();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
                                            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        refresh();
    }

    private View buildContentView() {
        int padding = dp(20);

        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(padding, padding, padding, padding);

        TextView title = new TextView(this);
        title.setText(getString(R.string.onboarding_title));
        title.setTextSize(24);
        title.setGravity(Gravity.CENTER_HORIZONTAL);
        content.addView(title, matchWrap());

        TextView blurb = new TextView(this);
        blurb.setText("Grant the permissions you want AndroidConnect to use. "
                + "Required permissions are needed for the core mirror, input, and notification features. "
                + "Optional permissions unlock individual companion features.");
        blurb.setTextSize(14);
        blurb.setPadding(0, dp(8), 0, dp(16));
        content.addView(blurb, matchWrap());

        Button refresh = new Button(this);
        refresh.setText("Re-check permissions");
        refresh.setOnClickListener(v -> refresh());
        content.addView(refresh, matchWrap());

        for (PermissionStatusRepository.PermissionState ps : repository.snapshot().values()) {
            CardViews card = addPermissionCard(content, ps);
            cards.put(ps.id, card);
        }

        Button finish = new Button(this);
        finish.setText("Continue to AndroidConnect");
        finish.setOnClickListener(v -> completeOnboarding());
        content.addView(finish, matchWrap());

        ScrollView scroll = new ScrollView(this);
        scroll.addView(content, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));
        return scroll;
    }

    private CardViews addPermissionCard(LinearLayout parent,
                                         PermissionStatusRepository.PermissionState state) {
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(dp(16), dp(16), dp(16), dp(16));

        GradientDrawable bg = new GradientDrawable();
        bg.setColor(0xFFF1F3F4);
        bg.setCornerRadius(dp(8));
        card.setBackground(bg);

        LinearLayout titleRow = new LinearLayout(this);
        titleRow.setOrientation(LinearLayout.HORIZONTAL);
        titleRow.setGravity(Gravity.CENTER_VERTICAL);

        TextView title = new TextView(this);
        title.setText(state.title + (state.required ? "  (required)" : "  (optional)"));
        title.setTextSize(16);
        title.setTypeface(title.getTypeface(), android.graphics.Typeface.BOLD);
        title.setTextColor(Color.BLACK);
        LinearLayout.LayoutParams titleParams = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        titleRow.addView(title, titleParams);

        TextView chip = new TextView(this);
        chip.setPadding(dp(10), dp(4), dp(10), dp(4));
        chip.setTextSize(12);
        chip.setTextColor(Color.WHITE);
        styleChip(chip, state.state);
        chip.setText(state.stateLabel());
        titleRow.addView(chip, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        card.addView(titleRow, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView description = new TextView(this);
        description.setText(state.description);
        description.setTextSize(13);
        description.setTextColor(0xFF333333);
        description.setPadding(0, dp(6), 0, dp(8));
        card.addView(description, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView warning = new TextView(this);
        warning.setTextSize(12);
        warning.setTextColor(0xFFB00020);
        warning.setVisibility(View.GONE);
        card.addView(warning, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        Button action = new Button(this);
        action.setText(actionLabelFor(state));
        action.setOnClickListener(v -> launchAction(state.id));
        card.addView(action, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));

        LinearLayout.LayoutParams cardParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        cardParams.setMargins(0, dp(8), 0, dp(8));
        parent.addView(card, cardParams);

        return new CardViews(chip, warning, action);
    }

    private void refresh() {
        Map<PermissionStatusRepository.PermissionId, PermissionStatusRepository.PermissionState>
                snapshot = repository.snapshot();
        for (Map.Entry<PermissionStatusRepository.PermissionId, CardViews> entry
                : cards.entrySet()) {
            PermissionStatusRepository.PermissionState ps = snapshot.get(entry.getKey());
            if (ps == null) {
                continue;
            }
            CardViews views = entry.getValue();
            views.chip.setText(ps.stateLabel());
            styleChip(views.chip, ps.state);
            views.action.setText(actionLabelFor(ps));

            boolean warn = ps.required
                    && ps.state != PermissionStatusRepository.State.GRANTED;
            if (warn) {
                views.warning.setText("Required for core features — please grant before continuing.");
                views.warning.setVisibility(View.VISIBLE);
            } else {
                views.warning.setVisibility(View.GONE);
            }
        }
    }

    private void launchAction(PermissionStatusRepository.PermissionId id) {
        switch (id) {
            case NOTIFICATION_LISTENER:
                safelyStart(new Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS));
                return;
            case ACCESSIBILITY:
                safelyStart(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS));
                return;
            default:
                String[] permissions = PermissionStatusRepository.runtimePermissionsFor(id);
                if (permissions.length == 0) {
                    Toast.makeText(this, "No permission to request on this Android version",
                            Toast.LENGTH_SHORT).show();
                    return;
                }
                requestPermissions(permissions, REQUEST_RUNTIME_PERMISSION);
        }
    }

    private void safelyStart(Intent intent) {
        try {
            startActivity(intent);
        } catch (RuntimeException error) {
            Toast.makeText(this, "Settings page not available: " + error.getMessage(),
                    Toast.LENGTH_LONG).show();
        }
    }

    private void completeOnboarding() {
        SharedPreferences prefs = getSharedPreferences(PREFS_NAME, MODE_PRIVATE);
        prefs.edit().putBoolean(KEY_COMPLETED, true).apply();
        Intent intent = new Intent(this, MainActivity.class);
        intent.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        startActivity(intent);
        finish();
    }

    private String actionLabelFor(PermissionStatusRepository.PermissionState state) {
        if (state.state == PermissionStatusRepository.State.GRANTED) {
            switch (state.id) {
                case NOTIFICATION_LISTENER:
                case ACCESSIBILITY:
                    return "Open settings";
                default:
                    return "Re-check";
            }
        }
        switch (state.id) {
            case NOTIFICATION_LISTENER:
                return "Open notification access";
            case ACCESSIBILITY:
                return "Open accessibility settings";
            default:
                return "Grant permission";
        }
    }

    private void styleChip(TextView chip, PermissionStatusRepository.State state) {
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(12));
        switch (state) {
            case GRANTED:
                bg.setColor(0xFF1B873F);
                break;
            case NEEDS_SETTINGS_PAGE:
                bg.setColor(0xFFB45309);
                break;
            case NOT_GRANTED:
            default:
                bg.setColor(0xFF9CA3AF);
        }
        chip.setBackground(bg);
    }

    private LinearLayout.LayoutParams matchWrap() {
        LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        p.setMargins(0, 0, 0, dp(12));
        return p;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    /** True if the user has previously completed onboarding. */
    public static boolean isCompleted(Context context) {
        return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getBoolean(KEY_COMPLETED, false);
    }

    private static final class CardViews {
        final TextView chip;
        final TextView warning;
        final Button action;

        CardViews(TextView chip, TextView warning, Button action) {
            this.chip = chip;
            this.warning = warning;
            this.action = action;
        }
    }
}
