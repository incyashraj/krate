package tech.krate.player;

import android.app.AlertDialog;
import android.content.DialogInterface;
import android.util.Log;

/**
 * An app's message or yes-or-no question (krate:ui/dialog) as the phone's own
 * dialog, over the app that asked. Krate's thread asks for the answer until
 * the person gives it.
 */
final class Dialog {
    private Dialog() {}

    static final int OPEN = -1;
    static final int YES = 0;
    static final int NO = 1;

    private static int answer = NO;

    /** Show title and body: OK alone for a message, Yes and No for a question. */
    static void show(final KrateActivity activity, final String title, final String body,
            final boolean question) {
        synchronized (Dialog.class) {
            answer = OPEN;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    AlertDialog.Builder builder = new AlertDialog.Builder(activity)
                            .setTitle(title)
                            .setMessage(body);
                    builder.setOnDismissListener(new DialogInterface.OnDismissListener() {
                        @Override
                        public void onDismiss(DialogInterface dialog) {
                            // Back, a tap outside, or No: whatever closed it
                            // and was not Yes.
                            finish(NO);
                        }
                    });
                    if (question) {
                        builder.setPositiveButton("Yes", new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                finish(YES);
                            }
                        });
                        builder.setNegativeButton("No", null);
                    } else {
                        builder.setPositiveButton("OK", null);
                    }
                    builder.show();
                } catch (RuntimeException failed) {
                    Log.w(KrateActivity.TAG, "dialog: " + failed);
                    finish(NO);
                }
            }
        });
    }

    /** The first answer wins: Yes, then the dismissal that follows it. */
    private static void finish(int value) {
        synchronized (Dialog.class) {
            if (answer == OPEN) {
                answer = value;
            }
        }
    }

    static synchronized int answer() {
        return answer;
    }
}
