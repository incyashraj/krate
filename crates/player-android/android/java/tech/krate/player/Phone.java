package tech.krate.player;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.speech.tts.TextToSpeech;
import android.speech.tts.UtteranceProgressListener;
import android.speech.tts.Voice;
import android.util.Log;

import java.util.ArrayList;
import java.util.Locale;
import java.util.Set;

/**
 * What a desktop app takes for granted, on an Android phone (IC-918): the
 * clipboard, a link handed to the app that owns it, a notification, and
 * speaking aloud with the phone's own voices. The camera is Cam.java; the
 * microphone is cpal's, behind the phone's prompt asked here.
 */
final class Phone {
    private Phone() {}

    // The clipboard.

    static String readClip(Context context) {
        ClipboardManager clip =
                (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clip == null || !clip.hasPrimaryClip()) {
            return "";
        }
        ClipData data = clip.getPrimaryClip();
        if (data == null || data.getItemCount() == 0) {
            return "";
        }
        CharSequence text = data.getItemAt(0).coerceToText(context);
        return text == null ? "" : text.toString();
    }

    static boolean writeClip(Context context, String text) {
        ClipboardManager clip =
                (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clip == null) {
            return false;
        }
        clip.setPrimaryClip(ClipData.newPlainText("Krate", text));
        return true;
    }

    // A link.

    /** Null when the link went to the app that opens it; else why not. */
    static String openUrl(final KrateActivity activity, String url) {
        final Intent intent = new Intent(Intent.ACTION_VIEW, Uri.parse(url));
        intent.addCategory(Intent.CATEGORY_BROWSABLE);
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            activity.startActivity(intent);
            Log.i(KrateActivity.TAG, "opened a link");
            return null;
        } catch (ActivityNotFoundException e) {
            return "nothing on this phone opens that link";
        } catch (RuntimeException e) {
            return "the link could not be opened: " + e.getMessage();
        }
    }

    // Notifications.

    private static final String CHANNEL = "krate-apps";
    private static int nextId = 1;

    static String[] notifyPermissions() {
        return Build.VERSION.SDK_INT >= 33
                ? new String[] {"android.permission.POST_NOTIFICATIONS"}
                : new String[0];
    }

    /**
     * Null once posted; else why not. The app's name is the sender line. A
     * non-empty tag (the app's id and its own, IC-271) replaces whatever
     * shows under it; an empty one stands alone.
     */
    static synchronized String notify(
            Context context, String app, String title, String body, String tag) {
        NotificationManager manager =
                (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) {
            return "this phone has no notifications";
        }
        if (!manager.areNotificationsEnabled()) {
            return "notifications are switched off for Krate: Settings > Apps > Krate";
        }
        if (manager.getNotificationChannel(CHANNEL) == null) {
            NotificationChannel channel = new NotificationChannel(CHANNEL,
                    "From your apps", NotificationManager.IMPORTANCE_HIGH);
            channel.setDescription("What the apps you open in Krate tell you");
            manager.createNotificationChannel(channel);
        }
        Notification note = new Notification.Builder(context, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setContentTitle(title)
                .setContentText(body)
                .setStyle(new Notification.BigTextStyle().bigText(body))
                .setSubText(app)
                .setAutoCancel(true)
                .build();
        if (tag == null || tag.isEmpty()) {
            manager.notify(nextId++, note);
        } else {
            manager.notify(tag, 0, note);
        }
        Log.i(KrateActivity.TAG, "notification posted");
        return null;
    }

    /** Take back the notification under a tag; nothing if none shows. */
    static synchronized void withdraw(Context context, String tag) {
        NotificationManager manager =
                (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager != null && tag != null && !tag.isEmpty()) {
            manager.cancel(tag, 0);
        }
    }

    // Speaking aloud.

    private static TextToSpeech tts;
    /** 0 starting, 1 ready, 2 no engine. */
    private static volatile int ready;
    private static volatile boolean speaking;

    /** Start the phone's speech engine once; 0 while it starts, 1 ready, 2 none. */
    static synchronized int speechReady(final Context context) {
        if (tts == null) {
            ready = 0;
            tts = new TextToSpeech(context.getApplicationContext(),
                    new TextToSpeech.OnInitListener() {
                        @Override
                        public void onInit(int status) {
                            ready = status == TextToSpeech.SUCCESS ? 1 : 2;
                            Log.i(KrateActivity.TAG, "speech engine: "
                                    + (ready == 1 ? "ready" : "none"));
                        }
                    });
            tts.setOnUtteranceProgressListener(new UtteranceProgressListener() {
                @Override
                public void onStart(String id) {
                    speaking = true;
                    Log.i(KrateActivity.TAG, "speech: started");
                }

                @Override
                public void onDone(String id) {
                    speaking = false;
                    Log.i(KrateActivity.TAG, "speech: done");
                }

                @Override
                @SuppressWarnings("deprecation")
                public void onError(String id) {
                    speaking = false;
                    Log.w(KrateActivity.TAG, "speech: failed");
                }

                @Override
                public void onStop(String id, boolean interrupted) {
                    speaking = false;
                }
            });
        }
        return ready;
    }

    /** Each voice as id, name and language, tab separated. */
    static synchronized String[] voices() {
        ArrayList<String> out = new ArrayList<>();
        if (tts == null || ready != 1) {
            return new String[0];
        }
        Set<Voice> all = tts.getVoices();
        if (all != null) {
            for (Voice v : all) {
                if (v.isNetworkConnectionRequired()) {
                    continue;
                }
                Locale l = v.getLocale();
                out.add(v.getName() + "\t" + l.getDisplayName() + "\t" + l.toLanguageTag());
            }
        }
        return out.toArray(new String[0]);
    }

    /** Null once speaking; else why not. Replaces anything still being said. */
    static synchronized String say(String text, String voice, float rate) {
        if (tts == null || ready != 1) {
            return "this phone has no speech engine";
        }
        if (voice != null && !voice.isEmpty() && tts.getVoices() != null) {
            for (Voice v : tts.getVoices()) {
                if (v.getName().equals(voice)) {
                    tts.setVoice(v);
                }
            }
        }
        tts.setSpeechRate(rate);
        speaking = true;
        int queued = tts.speak(text, TextToSpeech.QUEUE_FLUSH, new Bundle(), "krate");
        if (queued != TextToSpeech.SUCCESS) {
            speaking = false;
            return "the phone would not speak (" + queued + ")";
        }
        return null;
    }

    static synchronized void stopSpeaking() {
        if (tts != null) {
            tts.stop();
            speaking = false;
        }
    }

    static boolean isSpeaking() {
        return speaking || (tts != null && tts.isSpeaking());
    }
}
