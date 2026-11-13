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
    /** Which dialog the answer is for. A dialog closes after its button's
     *  click, so a Yes is followed by its own dismissal -- which, once the
     *  app has asked again, landed on the next dialog and answered it before
     *  anyone saw it. Each close now only answers its own dialog. */
    private static int shown = 0;

    /** Show title and body: OK alone for a message, Yes and No for a question. */
    static void show(final KrateActivity activity, final String title, final String body,
            final boolean question) {
        final int mine;
        synchronized (Dialog.class) {
            answer = OPEN;
            mine = ++shown;
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
                            finish(mine, NO);
                        }
                    });
                    if (question) {
                        builder.setPositiveButton("Yes", new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                finish(mine, YES);
                            }
                        });
                        builder.setNegativeButton("No", null);
                    } else {
                        builder.setPositiveButton("OK", null);
                    }
                    builder.show();
                } catch (RuntimeException failed) {
                    Log.w(KrateActivity.TAG, "dialog: " + failed);
                    finish(mine, NO);
                }
            }
        });
    }

    /** The first answer wins: Yes, then the dismissal that follows it. Only
     *  the dialog on screen may answer. */
    private static void finish(int dialog, int value) {
        synchronized (Dialog.class) {
            if (dialog == shown && answer == OPEN) {
                answer = value;
            }
        }
    }

    static synchronized int answer() {
        return answer;
    }
}
