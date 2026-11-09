package tech.krate.player;

import android.app.Dialog;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.util.Log;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.LinearLayout;
import android.widget.PopupWindow;
import android.widget.TextView;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Krate's menu over a running app (IC-934). Nothing sits over an app: a
 * touch and hold on the strip under the clock, or the phone's back gesture,
 * drops Krate's own panel from the top -- the app's name, then Close,
 * Restart, Share this app, What it can use and Report a problem. It is
 * Krate's, drawn on the UI thread, so it opens whatever the app is doing.
 *
 * Close and Restart ask the app to close as a desktop close button does; an
 * app that does not answer within two seconds is closed anyway.
 */
final class Menu {
    private Menu() {}

    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    /** The running app, as the player says: null between apps. */
    private static String name;
    private static String id;
    private static String[] uses = new String[0];
    private static String opens = "";

    /** What the app's thread should do next: 0 nothing, 1 close. */
    private static int action;
    /** Open the app again once it has closed. */
    private static boolean restart;
    /** Counts the apps started, so a late fallback knows its app has gone. */
    private static int started;

    private static Dialog panel;

    static synchronized void setApp(KrateActivity activity, String appName, String appId,
            String[] appUses, String appOpens) {
        name = appName;
        id = appId;
        uses = appUses == null ? new String[0] : appUses;
        if (appName != null) {
            // A new app starts with nothing asked; the end of one keeps a
            // Restart asked, for the player to act on.
            opens = appOpens == null ? "" : appOpens;
            action = 0;
            restart = false;
            started++;
            MAIN.post(new Runnable() {
                @Override
                public void run() {
                    showGrab(activity);
                }
            });
        }
    }

    /** The running app's id, or "". */
    static synchronized String appId() {
        return id == null ? "" : id;
    }

    /** The running app's name, or "An app". */
    static synchronized String appName() {
        return name == null ? "An app" : name;
    }

    static synchronized boolean running() {
        return name != null;
    }

    static synchronized int takeAction() {
        int out = action;
        action = 0;
        return out;
    }

    static synchronized boolean restartAsked() {
        return restart;
    }

    private static int dp(Context context, float value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value,
                context.getResources().getDisplayMetrics()));
    }

    private static boolean dark(Context context) {
        return (context.getResources().getConfiguration().uiMode
                & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES;
    }

    /**
     * Krate's menu: one glass row of five buttons, each an icon over a word
     * -- Close, Restart, Share, Access, Report -- like the phone's own Copy,
     * Move and Share row, sliding in under the status bar with a light tap.
     * The window behind it is blurred where Android can blur (12 and up).
     * UI thread.
     */
    static void show(final KrateActivity activity) {
        if (!running() || (panel != null && panel.isShowing())) {
            return;
        }
        final boolean dark = dark(activity);
        final int ink = dark ? Color.WHITE : Color.rgb(20, 20, 24);
        final String[] words = {"Close", "Restart", "Share", "Access", "Report"};
        final String[] said = {"Close", "Restart", "Share this app", "What it can use",
            "Report a problem"};

        final Dialog dialog = new Dialog(activity);
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
        final LinearLayout row = new LinearLayout(activity);
        row.setOrientation(LinearLayout.HORIZONTAL);
        for (int i = 0; i < words.length; i++) {
            final int which = i;
            LinearLayout cell = new LinearLayout(activity);
            cell.setOrientation(LinearLayout.VERTICAL);
            cell.setGravity(Gravity.CENTER);
            cell.setContentDescription(said[i]);
            TypedValue ripple = new TypedValue();
            activity.getTheme().resolveAttribute(android.R.attr.selectableItemBackgroundBorderless,
                    ripple, true);
            cell.setBackgroundResource(ripple.resourceId);
            cell.addView(new MenuIcon(activity, i, i == 0 ? Color.rgb(230, 60, 60) : ink),
                    new LinearLayout.LayoutParams(dp(activity, 26), dp(activity, 26)));
            TextView word = new TextView(activity);
            word.setText(words[i]);
            word.setTextColor(ink);
            word.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
            word.setGravity(Gravity.CENTER);
            word.setPadding(0, dp(activity, 6), 0, 0);
            cell.addView(word);
            cell.setOnClickListener(new View.OnClickListener() {
                @Override
                public void onClick(View v) {
                    dialog.dismiss();
                    pick(activity, which);
                }
            });
            row.addView(cell, new LinearLayout.LayoutParams(0, dp(activity, 80), 1f));
        }
        dialog.setContentView(row);
        Window window = dialog.getWindow();
        if (window != null) {
            int screen = activity.getResources().getDisplayMetrics().widthPixels;
            int width = Math.min(screen - dp(activity, 32), dp(activity, 380));
            GradientDrawable glass = new GradientDrawable();
            glass.setCornerRadius(dp(activity, 26));
            boolean blur = android.os.Build.VERSION.SDK_INT >= 31
                    && activity.getWindowManager().isCrossWindowBlurEnabled();
            // Frosted where the phone blurs, nearly solid where it cannot.
            int alpha = blur ? 200 : 240;
            glass.setColor(dark ? Color.argb(alpha, 40, 40, 46) : Color.argb(alpha, 250, 250, 252));
            glass.setStroke(1, dark ? Color.argb(60, 255, 255, 255) : Color.argb(40, 0, 0, 0));
            window.setBackgroundDrawable(glass);
            if (blur) {
                window.setBackgroundBlurRadius(dp(activity, 30));
            }
            window.setGravity(Gravity.TOP | Gravity.CENTER_HORIZONTAL);
            window.setLayout(width, ViewGroup.LayoutParams.WRAP_CONTENT);
            WindowManager.LayoutParams lp = window.getAttributes();
            // A dialog's window already starts below the status bar.
            lp.y = dp(activity, 6);
            window.setAttributes(lp);
            window.setDimAmount(0.12f);
        }
        dialog.setOnShowListener(new android.content.DialogInterface.OnShowListener() {
            @Override
            public void onShow(android.content.DialogInterface d) {
                row.setTranslationY(-dp(activity, 24));
                row.setAlpha(0f);
                row.animate().translationY(0).alpha(1f).setDuration(200).start();
            }
        });
        panel = dialog;
        dialog.show();
        // A light tap: the hold was heard.
        Haptics.play(activity, Haptics.LIGHT, 1f);
        Log.i(KrateActivity.TAG, "menu: open over " + name);
    }

    private static void pick(KrateActivity activity, int which) {
        switch (which) {
            case 0:
                close(activity, false);
                break;
            case 1:
                close(activity, true);
                break;
            case 2:
                shareApp(activity);
                break;
            case 3:
                showUses(activity);
                break;
            default:
                reportProblem(activity);
                break;
        }
    }

    /** Whether the app started as `run` is still the one running. */
    private static synchronized boolean stillRunning(int run) {
        return name != null && started == run;
    }

    /** Ask the app to close (and open again); close it anyway after 2 s. */
    static void close(final KrateActivity activity, boolean again) {
        final int run;
        synchronized (Menu.class) {
            action = 1;
            restart = again;
            run = started;
        }
        Log.i(KrateActivity.TAG, "menu: " + (again ? "restart" : "close"));
        MAIN.postDelayed(new Runnable() {
            @Override
            public void run() {
                // The app closed: Krate is back on its home screen, or on
                // another app the person opened since, and stays.
                if (!stillRunning(run)) {
                    return;
                }
                // Still here: the app did not answer. Krate closes it.
                Log.i(KrateActivity.TAG, "menu: the app did not answer; closing it");
                if (restartAsked()) {
                    relaunch(activity);
                } else {
                    activity.finishAndRemoveTask();
                    Process.killProcess(Process.myPid());
                }
            }
        }, 2000);
    }

    /** Start Krate on the same app in a fresh process (Restarter.java). */
    static void relaunch(KrateActivity activity) {
        Intent again = new Intent(activity, KrateActivity.class);
        synchronized (Menu.class) {
            if (opens != null && !opens.isEmpty()) {
                again.setAction(Intent.ACTION_VIEW);
                again.setDataAndType(Uri.parse(opens), "application/octet-stream");
            } else {
                again.setAction(Intent.ACTION_MAIN);
            }
        }
        Intent restarter = new Intent(activity, Restarter.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(Restarter.OPEN, again)
                .putExtra(Restarter.PID, Process.myPid());
        activity.startActivity(restarter);
        // finish, not finishAndRemoveTask: the restarter opens in this task,
        // and removing the task took it too, so nothing opened again.
        activity.finish();
        Runtime.getRuntime().exit(0);
    }

    /** The share sheet with the app's own .krate; Krate's address for the built-in one. */
    private static void shareApp(KrateActivity activity) {
        String from;
        synchronized (Menu.class) {
            from = opens;
        }
        if (from == null || !from.startsWith("file://")) {
            activity.krateShare("Krate: open apps anyone can make. https://krate.tech",
                    new String[0]);
            return;
        }
        File source = new File(Uri.parse(from).getPath());
        File folder = new File(new File(activity.getCacheDir(), "krate-share"), "menu");
        if (!folder.isDirectory() && !folder.mkdirs()) {
            return;
        }
        File copy = new File(folder, Documents.safeName(name) + ".krate");
        try (InputStream in = new FileInputStream(source);
             OutputStream out = new FileOutputStream(copy)) {
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) {
                out.write(buf, 0, n);
            }
        } catch (IOException e) {
            Log.w(KrateActivity.TAG, "menu: could not stage the app: " + e);
            return;
        }
        activity.krateShare("", new String[] {copy.getAbsolutePath()});
    }

    /** This run's grants in the permission sheet's words, with Ask again. */
    private static void showUses(final KrateActivity activity) {
        StringBuilder text = new StringBuilder();
        String[] list;
        synchronized (Menu.class) {
            list = uses;
        }
        if (list.length == 0) {
            text.append("Only its own window: nothing on this phone beyond it.");
        } else {
            for (String use : list) {
                if (text.length() > 0) {
                    text.append('\n');
                }
                text.append("• ").append(use);
            }
        }
        new android.app.AlertDialog.Builder(activity)
                .setTitle(name + " can use")
                .setMessage(text.toString())
                .setPositiveButton("Ask again",
                        new android.content.DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(android.content.DialogInterface d, int w) {
                                close(activity, true);
                            }
                        })
                .setNegativeButton("Done", null)
                .show();
    }

    /** A mail to Krate about the running app, its name and id filled in. */
    private static void reportProblem(KrateActivity activity) {
        String subject = "A problem with " + name;
        String body = "App: " + name + " (" + id + ")\n\nWhat happened:\n";
        Intent mail = new Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:yashraj@krate.tech"
                + "?subject=" + Uri.encode(subject) + "&body=" + Uri.encode(body)));
        try {
            activity.startActivity(mail);
        } catch (android.content.ActivityNotFoundException e) {
            android.widget.Toast.makeText(activity,
                    "No mail app here. Write to yashraj@krate.tech about " + name + ".",
                    android.widget.Toast.LENGTH_LONG).show();
        }
    }

    /**
     * The grab bar at the top centre over a newly opened app, fading after two
     * seconds, and on the first three apps a person opens a line saying how
     * to reach the menu. UI thread.
     */
    private static void showGrab(final KrateActivity activity) {
        final View decor = activity.getWindow().getDecorView();
        if (decor.getWindowToken() == null) {
            MAIN.postDelayed(new Runnable() {
                @Override
                public void run() {
                    showGrab(activity);
                }
            }, 200);
            return;
        }
        int top = activity.krateCovered()[0];
        LinearLayout box = new LinearLayout(activity);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setGravity(Gravity.CENTER_HORIZONTAL);
        View bar = new View(activity);
        GradientDrawable pill = new GradientDrawable();
        pill.setColor(Color.argb(217, 140, 140, 140));
        pill.setCornerRadius(dp(activity, 2.5f));
        bar.setBackground(pill);
        box.addView(bar, new LinearLayout.LayoutParams(dp(activity, 36), dp(activity, 5)));
        SharedPreferences prefs =
                activity.getSharedPreferences("krate-menu", Context.MODE_PRIVATE);
        int shown = prefs.getInt("hints", 0);
        if (shown < 3) {
            prefs.edit().putInt("hints", shown + 1).apply();
            TextView hint = new TextView(activity);
            hint.setText("Touch and hold the top for Krate's menu");
            hint.setTextColor(Color.WHITE);
            hint.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13);
            int p = dp(activity, 12);
            hint.setPadding(p, dp(activity, 6), p, dp(activity, 6));
            GradientDrawable chip = new GradientDrawable();
            chip.setColor(Color.argb(184, 0, 0, 0));
            chip.setCornerRadius(dp(activity, 15));
            hint.setBackground(chip);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            lp.topMargin = dp(activity, 8);
            box.addView(hint, lp);
        }
        final PopupWindow popup = new PopupWindow(box, ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, false);
        popup.setTouchable(false);
        popup.setClippingEnabled(false);
        try {
            popup.showAtLocation(decor, Gravity.TOP | Gravity.CENTER_HORIZONTAL, 0,
                    top + dp(activity, 6));
        } catch (RuntimeException e) {
            return;
        }
        final long hold = shown < 3 ? 3500 : 2000;
        MAIN.postDelayed(new Runnable() {
            @Override
            public void run() {
                box.animate().alpha(0f).setDuration(400).withEndAction(new Runnable() {
                    @Override
                    public void run() {
                        popup.dismiss();
                    }
                }).start();
            }
        }, hold);
    }
}
