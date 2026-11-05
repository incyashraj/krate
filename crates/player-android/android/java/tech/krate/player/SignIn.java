package tech.krate.player;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;

import java.util.List;

/**
 * Signing the person in through the browser (krate:ui/sign-in, IC-923). The
 * password goes into the service's own page in the browser, never the app.
 *
 * Android hands a redirect back only to a scheme an app's install manifest
 * names, and Krate cannot name each app's own. So Krate names one,
 * krate-signin, which any app may use as its redirect
 * (krate-signin://<app id>/...), opened in a Custom Tab of whatever browser
 * the phone has. A shared scheme is safe because of PKCE: the code that comes
 * back is useless without the verifier only the app that asked holds, so a
 * code caught by the wrong hands cannot be traded for a token.
 *
 * An app that insists on its own scheme gets Chrome's Auth Tab, which
 * returns any scheme to the app that opened it, where the installed Chrome
 * has one (132 and later); elsewhere it hears plainly that the browser
 * cannot return to it.
 */
final class SignIn {
    private SignIn() {}

    static final String SCHEME = "krate-signin";
    /** Krate's own sign-in: krate.tech hands it back as krate://signed-in. */
    static final String KRATE = "krate";
    static final int AUTH_TAB = 0x4b05;

    private static final String CUSTOM_TABS_SERVICE =
            "android.support.customtabs.action.CustomTabsService";
    private static final String AUTH_TAB_CATEGORY = "androidx.browser.auth.category.AuthTab";
    private static final String EXTRA_SESSION = "android.support.customtabs.extra.SESSION";
    private static final String EXTRA_LAUNCH_AUTH_TAB = "androidx.browser.auth.extra.LAUNCH_AUTH_TAB";
    private static final String EXTRA_REDIRECT_SCHEME = "androidx.browser.auth.extra.REDIRECT_SCHEME";

    /** 0 idle, 1 waiting for the browser, 2 answered. */
    private static int step;
    private static String answer = "";
    private static String waitingFor = "";
    private static String app = "";
    private static boolean left;

    /** The browser that would open a web page, if any. */
    private static String browser(Context context) {
        Intent web = new Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com"));
        ResolveInfo found = context.getPackageManager()
                .resolveActivity(web, PackageManager.MATCH_DEFAULT_ONLY);
        if (found == null || found.activityInfo == null
                || "android".equals(found.activityInfo.packageName)) {
            List<ResolveInfo> all = context.getPackageManager().queryIntentActivities(web, 0);
            return all.isEmpty() ? null : all.get(0).activityInfo.packageName;
        }
        return found.activityInfo.packageName;
    }

    /** Whether the browser package offers Chrome's Auth Tab. */
    static boolean authTabSupported(Context context, String pkg) {
        if (pkg == null) {
            return false;
        }
        Intent service = new Intent(CUSTOM_TABS_SERVICE).setPackage(pkg)
                .addCategory(AUTH_TAB_CATEGORY);
        List<ResolveInfo> found = context.getPackageManager().queryIntentServices(service, 0);
        return found != null && !found.isEmpty();
    }

    /** "<browser> <version>, Auth Tab yes|no", for the log and the proof. */
    static String describe(Context context) {
        String pkg = browser(context);
        String version = "?";
        try {
            if (pkg != null) {
                version = context.getPackageManager().getPackageInfo(pkg, 0).versionName;
            }
        } catch (PackageManager.NameNotFoundException e) {
            version = "?";
        }
        return pkg + " " + version + ", Auth Tab " + (authTabSupported(context, pkg) ? "yes" : "no");
    }

    /**
     * Open the sign-in page. Null once open (the answer comes through
     * answer()); otherwise why it cannot open.
     */
    static String start(final KrateActivity activity, final String url, final String scheme,
            String appId) {
        String pkg = browser(activity);
        Log.i(KrateActivity.TAG, "sign-in: " + describe(activity) + ", returning to " + scheme);
        if (pkg == null) {
            return "This phone has no browser to sign in with.";
        }
        // krate-signin (apps') and krate (Krate's own) are named in the
        // manifest, so a plain Custom Tab can return to them; any other
        // scheme needs Chrome's Auth Tab.
        final boolean auth = authTabSupported(activity, pkg);
        final boolean shared = SCHEME.equals(scheme) || (KRATE.equals(scheme) && !auth);
        if (!shared && !auth) {
            return "This phone's browser cannot return to " + scheme
                    + ": use krate-signin as the redirect.";
        }
        synchronized (SignIn.class) {
            step = 1;
            answer = "";
            waitingFor = scheme;
            app = appId;
            left = false;
        }
        final Intent open = new Intent(Intent.ACTION_VIEW, Uri.parse(url)).setPackage(pkg);
        // The state only: a nonce, worth nothing without the app's verifier.
        Log.d(KrateActivity.TAG, "sign-in: waiting for " + appId + ", state "
                + Uri.parse(url).getQueryParameter("state"));
        // A Custom Tab: the browser's own page over Krate, its address bar
        // showing where the person really is.
        Bundle session = new Bundle();
        session.putBinder(EXTRA_SESSION, null);
        open.putExtras(session);
        if (!shared) {
            open.putExtra(EXTRA_LAUNCH_AUTH_TAB, true);
            open.putExtra(EXTRA_REDIRECT_SCHEME, scheme);
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    if (shared) {
                        activity.startActivity(open);
                    } else {
                        activity.startActivityForResult(open, AUTH_TAB);
                    }
                } catch (ActivityNotFoundException e) {
                    finish("");
                }
            }
        });
        return null;
    }

    /** Krate left the screen for the browser. */
    static synchronized void paused() {
        if (step == 1) {
            left = true;
        }
    }

    /**
     * Krate is back. Back from the browser with no redirect is the person
     * closing the page: a cancel. A redirect arrives before this (onNewIntent
     * or onActivityResult), so it is never mistaken for one.
     */
    static synchronized void resumed() {
        if (step == 1 && left) {
            step = 2;
            answer = "";
            Log.i(KrateActivity.TAG, "sign-in: closed without signing in");
        }
    }

    /**
     * A krate-signin:// address arrived. Taken only while the running app is
     * waiting, and only for that app's id; anything else is ignored.
     */
    static boolean redirected(Intent intent) {
        Uri data = intent == null ? null : intent.getData();
        if (data != null && KRATE.equals(data.getScheme()) && "signed-in".equals(data.getHost())) {
            synchronized (SignIn.class) {
                if (step != 1 || !KRATE.equals(waitingFor)) {
                    Log.w(KrateActivity.TAG, "sign-in: a krate://signed-in nothing is waiting for");
                    return true;
                }
            }
            finish(data.toString());
            return true;
        }
        if (data == null || !SCHEME.equals(data.getScheme())) {
            return false;
        }
        synchronized (SignIn.class) {
            if (step != 1 || !SCHEME.equals(waitingFor) || !app.equals(data.getHost())) {
                Log.w(KrateActivity.TAG, "sign-in: a krate-signin address for "
                        + data.getHost() + " that nothing is waiting for");
                return true;
            }
        }
        finish(data.toString());
        return true;
    }

    /** Chrome's Auth Tab answered. */
    static void authTabResult(int result, Intent data) {
        Uri back = data == null ? null : data.getData();
        finish(result == Activity.RESULT_OK && back != null ? back.toString() : "");
    }

    private static synchronized void finish(String back) {
        step = 2;
        answer = back;
        Log.i(KrateActivity.TAG, "sign-in: " + (back.isEmpty() ? "cancelled" : "came back"));
    }

    /** Null while waiting; then the address the service sent back, "" for cancelled. */
    static synchronized String answer() {
        if (step != 2) {
            return null;
        }
        step = 0;
        return answer;
    }
}
