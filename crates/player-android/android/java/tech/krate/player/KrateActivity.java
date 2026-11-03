package tech.krate.player;

import android.app.NativeActivity;
import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.SystemClock;
import android.util.Log;

import java.io.File;
import java.util.ArrayList;
import java.util.HashMap;

/**
 * The Krate player's one activity: NativeActivity, which loads
 * libkrate_player.so and runs android_main, plus the few things only an
 * activity can be told.
 *
 * Everything Krate does happens in Rust. This class exists because some
 * answers Android gives only to an activity -- what the person picked in the
 * system's file picker (onActivityResult) -- and a NativeActivity drops them.
 * The native side never waits on the UI thread: every method here returns at
 * once, and Rust asks again (while it keeps the event loop running) until the
 * answer is in.
 */
public class KrateActivity extends NativeActivity {
    static final String TAG = "krate";

    /** The request code of Krate's picker; the low 16 bits are all Android keeps. */
    private static final int PICK = 0x4b01;
    /** The request code of the phone's permission prompts. */
    private static final int ASK = 0x4b02;
    /** The request code of the share sheet. */
    private static final int SHARE = 0x4b03;
    /** The request code of the photo picker. */
    private static final int PHOTOS = 0x4b04;

    /** The photo picker: up, and what it answered (URIs; empty for cancelled). */
    private boolean pickingPhotos;
    private String[] photosPicked = new String[0];
    /** What the share sheet tells Krate when the person picks a target. */
    private static final String SHARED = "tech.krate.player.SHARED";

    /** The share sheet: up, then closed at this time, and whether a target was picked. */
    private boolean sharing;
    private long shareClosedAt;
    private boolean shareSent;
    private BroadcastReceiver shareReceiver;

    /** krateAsk's answers. */
    static final int GRANTED = 0;
    static final int REFUSED = 1;
    static final int ASKING = 2;

    /** Permission sets asked for this run, and whether the prompt has answered. */
    private final HashMap<String, Boolean> asked = new HashMap<>();

    private final Object lock = new Object();
    /** True while the system's picker is up. */
    private boolean picking;
    /** What the picker answered: a content URI, or "" for cancelled. */
    private String picked = "";

    /** Pixels the status bar, gesture bar and camera cutout cover: top, right, bottom, left. */
    private volatile int[] covered = new int[4];
    /** The invisible text editor the on-screen keyboard talks to (K-1024). */
    private volatile Typing typing;

    @Override
    protected void onCreate(android.os.Bundle saved) {
        super.onCreate(saved);
        // What covers the window's edges, kept as Android says, so an app
        // keeps its text out from under the clock and the gesture bar (K-1039).
        getWindow().getDecorView().setOnApplyWindowInsetsListener(
                new android.view.View.OnApplyWindowInsetsListener() {
                    @Override
                    @SuppressWarnings("deprecation")
                    public android.view.WindowInsets onApplyWindowInsets(android.view.View view,
                            android.view.WindowInsets insets) {
                        int[] edges;
                        if (android.os.Build.VERSION.SDK_INT >= 30) {
                            android.graphics.Insets bars = insets.getInsets(
                                    android.view.WindowInsets.Type.systemBars()
                                    | android.view.WindowInsets.Type.displayCutout());
                            // The keyboard, while it is up, covers the
                            // bottom too: an app lays its fields out above
                            // it (K-1024). Before Android 11 the system
                            // window inset below already counts it
                            // (windowSoftInputMode adjustResize).
                            android.graphics.Insets typing = insets.getInsets(
                                    android.view.WindowInsets.Type.ime());
                            edges = new int[] {bars.top, bars.right,
                                    Math.max(bars.bottom, typing.bottom), bars.left};
                        } else {
                            edges = new int[] {insets.getSystemWindowInsetTop(),
                                    insets.getSystemWindowInsetRight(),
                                    insets.getSystemWindowInsetBottom(),
                                    insets.getSystemWindowInsetLeft()};
                            android.view.DisplayCutout cut = android.os.Build.VERSION.SDK_INT >= 28
                                    ? insets.getDisplayCutout() : null;
                            if (cut != null) {
                                edges[0] = Math.max(edges[0], cut.getSafeInsetTop());
                                edges[1] = Math.max(edges[1], cut.getSafeInsetRight());
                                edges[2] = Math.max(edges[2], cut.getSafeInsetBottom());
                                edges[3] = Math.max(edges[3], cut.getSafeInsetLeft());
                            }
                        }
                        covered = edges;
                        return view.onApplyWindowInsets(insets);
                    }
                });
    }

    /** Pixels covered at the top, right, bottom and left edges. */
    public int[] krateCovered() {
        return covered;
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == SignIn.AUTH_TAB) {
            SignIn.authTabResult(result, data);
            return;
        }
        if (request == PHOTOS) {
            ArrayList<String> picked = new ArrayList<>();
            if (result == RESULT_OK && data != null) {
                if (data.getClipData() != null) {
                    for (int i = 0; i < data.getClipData().getItemCount(); i++) {
                        picked.add(data.getClipData().getItemAt(i).getUri().toString());
                    }
                } else if (data.getData() != null) {
                    picked.add(data.getData().toString());
                }
            }
            synchronized (lock) {
                photosPicked = picked.toArray(new String[0]);
                pickingPhotos = false;
            }
            return;
        }
        if (request == SHARE) {
            synchronized (lock) {
                sharing = false;
                shareClosedAt = SystemClock.elapsedRealtime();
            }
            return;
        }
        if (request != PICK) {
            return;
        }
        Uri uri = (result == RESULT_OK && data != null) ? data.getData() : null;
        synchronized (lock) {
            picked = uri == null ? "" : uri.toString();
            picking = false;
        }
    }

    @Override
    public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request != ASK) {
            return;
        }
        synchronized (lock) {
            asked.put(String.join(",", permissions), Boolean.TRUE);
        }
    }

    /**
     * Whether the phone lets Krate use these permissions (any of them, or
     * all when all is true), asking the person the first time this run.
     * The app's own grant was already checked on Krate's sheet; this is the
     * phone's prompt behind it. GRANTED, REFUSED once the prompt answered
     * no, or ASKING while it is up.
     */
    public int krateAsk(final String[] permissions, boolean all) {
        int granted = 0;
        for (String permission : permissions) {
            if (checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) {
                granted++;
            }
        }
        if (all ? granted == permissions.length : granted > 0) {
            return GRANTED;
        }
        String key = String.join(",", permissions);
        synchronized (lock) {
            Boolean answered = asked.get(key);
            if (answered != null) {
                return answered ? REFUSED : ASKING;
            }
            asked.put(key, Boolean.FALSE);
        }
        Log.i(TAG, "asking the phone for " + key);
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                requestPermissions(permissions, ASK);
            }
        });
        return ASKING;
    }

    /**
     * Show the system's own picker. kind 0 opens a file, 1 opens a folder,
     * 2 asks where to save a new file called name. Answered through
     * {@link #kratePickAnswer()}.
     */
    public void kratePick(final int kind, final String mime, final String name) {
        synchronized (lock) {
            picking = true;
            picked = "";
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Intent intent;
                if (kind == 1) {
                    intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                } else if (kind == 2) {
                    intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                    intent.addCategory(Intent.CATEGORY_OPENABLE);
                    intent.setType(mime == null || mime.isEmpty() ? Documents.mimeOf(name) : mime);
                    intent.putExtra(Intent.EXTRA_TITLE, name);
                } else {
                    intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                    intent.addCategory(Intent.CATEGORY_OPENABLE);
                    intent.setType(mime == null || mime.isEmpty() ? "*/*" : mime);
                }
                try {
                    startActivityForResult(intent, PICK);
                } catch (ActivityNotFoundException e) {
                    Log.w(TAG, "no picker on this phone: " + e);
                    synchronized (lock) {
                        picking = false;
                        picked = "";
                    }
                }
            }
        });
    }

    /** Null while the picker is up; then the picked URI, or "" for cancelled. */
    public String kratePickAnswer() {
        synchronized (lock) {
            return picking ? null : picked;
        }
    }

    /** Copy a picked file into dir under its own name; the path, or null. */
    public String krateCopyIn(String uri, String dir) {
        return Documents.copyIn(this, Uri.parse(uri), dir);
    }

    /** Copy a picked folder into dir; the copy's path, or null. */
    public String krateMirror(String tree, String dir, long maxBytes) {
        return Documents.mirror(this, Uri.parse(tree), dir, maxBytes);
    }

    /** Put what the app changed in a mirrored folder back where it came from. */
    public int krateSyncBack(String root) {
        return Documents.syncBack(this, root);
    }

    /** Whether the phone can tap back (krate:ui/haptics). */
    public boolean krateHapticsAvailable() {
        return Haptics.available(this);
    }

    /** Tap back with feel, or an impact at strength when feel is -1. */
    public boolean krateHaptic(int feel, float strength) {
        return Haptics.play(this, feel, strength);
    }

    /** Whether the phone can tell how it is held (krate:ui/motion). */
    public boolean krateMotionAvailable() {
        return Motion.available(this);
    }

    /** The latest motion reading, starting the sensors on the first ask. */
    public float[] krateMotion() {
        return Motion.read(this);
    }

    /** Stop the motion sensors. */
    public void krateMotionStop() {
        Motion.stop();
    }

    /** Whether the phone can tell where it is (krate:ui/location). */
    public boolean krateLocationAvailable() {
        return Where.available(this);
    }

    /** The latest fix, asking and starting on the first call; see Where.read. */
    public double[] krateLocation() {
        return Where.read(this);
    }

    /** Stop looking for where the phone is. */
    public void krateLocationStop() {
        Where.stop();
    }

    /** Whether the phone has Bluetooth (krate:ui/bluetooth). */
    public boolean krateBtAvailable() {
        return Ble.available(this);
    }

    /** Start Krate's device list; see Ble.choose. */
    public byte[] krateBtChoose(String service, String title) {
        return Ble.choose(this, service, title);
    }

    /** Null while the person is choosing; then the pick; see Ble.chooseAnswer. */
    public byte[] krateBtChooseAnswer() {
        return Ble.chooseAnswer();
    }

    /** Start a read (0), write (1) or subscribe (2) on a picked device. */
    public void krateBtOp(long device, int kind, String service, String characteristic,
            byte[] data, boolean withResponse) {
        Ble.op(device, kind, service, characteristic, data, withResponse);
    }

    /** Null while the exchange runs; then its answer. */
    public byte[] krateBtOpAnswer() {
        return Ble.opAnswer();
    }

    /** Values a device sent since the last ask. */
    public byte[][] krateBtNotifications(long device) {
        return Ble.notifications(device);
    }

    /** Whether a picked device is still connected. */
    public boolean krateBtConnected(long device) {
        return Ble.connected(device);
    }

    /** Let a picked device go. */
    public void krateBtDisconnect(long device) {
        Ble.disconnect(device);
    }

    /** The app closed: let every device go. */
    public void krateBtDisconnectAll() {
        Ble.disconnectAll();
    }

    // IC-918: the clipboard, links, notifications, speaking aloud, the
    // microphone's and camera's prompts, and the camera itself.

    public String krateClipRead() {
        return Phone.readClip(this);
    }

    public boolean krateClipWrite(String text) {
        return Phone.writeClip(this, text);
    }

    public String krateOpenUrl(String url) {
        return Phone.openUrl(this, url);
    }

    /** The phone's prompt for posting notifications (Android 13 and up). */
    public int krateNotifyAsk() {
        String[] permissions = Phone.notifyPermissions();
        return permissions.length == 0 ? GRANTED : krateAsk(permissions, true);
    }

    public String krateNotify(String app, String title, String body) {
        return Phone.notify(this, app, title, body);
    }

    public int krateMicAsk() {
        return krateAsk(new String[] {"android.permission.RECORD_AUDIO"}, true);
    }

    public int krateCameraAsk() {
        return krateAsk(new String[] {"android.permission.CAMERA"}, true);
    }

    public int krateSpeechReady() {
        return Phone.speechReady(this);
    }

    public String[] krateVoices() {
        return Phone.voices();
    }

    public String krateSay(String text, String voice, float rate) {
        return Phone.say(text, voice, rate);
    }

    public void krateStopSpeaking() {
        Phone.stopSpeaking();
    }

    public boolean krateSpeaking() {
        return Phone.isSpeaking();
    }

    public String[] krateCameras() {
        return Cam.devices(this);
    }

    public String krateCameraOpen(String id, int width, int height, int fps) {
        return Cam.open(this, id, width, height, fps);
    }

    public int[] krateCameraInfo() {
        return Cam.info();
    }

    public byte[] krateCameraFrame() {
        return Cam.frame();
    }

    public String krateCameraError() {
        return Cam.error();
    }

    public void krateCameraClose() {
        Cam.close();
    }

    /** The display's rotation, 0..3 (ROTATION_0 to ROTATION_270). */
    @SuppressWarnings("deprecation")
    public int krateDisplayRotation() {
        return getWindowManager().getDefaultDisplay().getRotation();
    }

    /**
     * Open the system's share sheet (krate:ui/share) with text (may be empty)
     * and files already written to Krate's share folder. Answered through
     * krateShareAnswer.
     */
    public void krateShare(final String text, final String[] paths) {
        synchronized (lock) {
            sharing = true;
            shareSent = false;
            shareClosedAt = 0;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                openShareSheet(text, paths);
            }
        });
    }

    private void openShareSheet(String text, String[] paths) {
        ArrayList<Uri> uris = new ArrayList<>();
        for (String path : paths) {
            uris.add(Shared.uriFor(new File(path)));
        }
        Intent send;
        if (uris.size() > 1) {
            send = new Intent(Intent.ACTION_SEND_MULTIPLE);
            send.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
            send.setType("*/*");
        } else {
            send = new Intent(Intent.ACTION_SEND);
            if (uris.size() == 1) {
                send.putExtra(Intent.EXTRA_STREAM, uris.get(0));
                send.setType(Documents.mimeOf(paths[0]));
            } else {
                send.setType("text/plain");
            }
        }
        if (text != null && !text.isEmpty()) {
            send.putExtra(Intent.EXTRA_TEXT, text);
        }
        if (!uris.isEmpty()) {
            // The grant rides on the clip: the one app picked may read these
            // files, and only these.
            ClipData clip = ClipData.newRawUri("", uris.get(0));
            for (int i = 1; i < uris.size(); i++) {
                clip.addItem(new ClipData.Item(uris.get(i)));
            }
            send.setClipData(clip);
            send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        }
        if (shareReceiver == null) {
            shareReceiver = new BroadcastReceiver() {
                @Override
                public void onReceive(Context context, Intent intent) {
                    synchronized (lock) {
                        shareSent = true;
                    }
                    Log.i(TAG, "share: sent to " + intent.getParcelableExtra(
                            Intent.EXTRA_CHOSEN_COMPONENT));
                }
            };
            if (android.os.Build.VERSION.SDK_INT >= 33) {
                registerReceiver(shareReceiver, new IntentFilter(SHARED), RECEIVER_NOT_EXPORTED);
            } else {
                registerReceiver(shareReceiver, new IntentFilter(SHARED));
            }
        }
        Intent chosen = new Intent(SHARED).setPackage(getPackageName());
        PendingIntent told = PendingIntent.getBroadcast(this, 0, chosen,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_MUTABLE);
        try {
            startActivityForResult(Intent.createChooser(send, null, told.getIntentSender()), SHARE);
            Log.i(TAG, "share: sheet open, " + uris.size() + " files");
        } catch (ActivityNotFoundException e) {
            synchronized (lock) {
                sharing = false;
                shareClosedAt = 1;
            }
        }
    }

    /**
     * -1 while the sheet is up; then 1 when the person sent it to an app, 0
     * when they closed it. The "sent" word can trail the sheet closing, so
     * a closed sheet waits a moment for it.
     */
    public int krateShareAnswer() {
        synchronized (lock) {
            if (shareSent) {
                return 1;
            }
            if (sharing || shareClosedAt == 0) {
                return -1;
            }
            return SystemClock.elapsedRealtime() - shareClosedAt > 1500 ? 0 : -1;
        }
    }

    /**
     * The system's photo picker for up to limit photos (krate:ui/photos):
     * Android 13's own picker, which needs no permission, or the document
     * picker narrowed to pictures before it. Answered by kratePhotosAnswer.
     */
    public void kratePickPhotos(final int limit) {
        synchronized (lock) {
            pickingPhotos = true;
            photosPicked = new String[0];
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Intent intent;
                if (android.os.Build.VERSION.SDK_INT >= 33) {
                    intent = new Intent(android.provider.MediaStore.ACTION_PICK_IMAGES);
                    int most = Math.min(limit,
                            android.provider.MediaStore.getPickImagesMaxLimit());
                    if (most > 1) {
                        intent.putExtra(android.provider.MediaStore.EXTRA_PICK_IMAGES_MAX, most);
                    }
                } else {
                    intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                    intent.addCategory(Intent.CATEGORY_OPENABLE);
                    intent.setType("image/*");
                    intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, limit > 1);
                }
                try {
                    startActivityForResult(intent, PHOTOS);
                } catch (ActivityNotFoundException e) {
                    Log.w(TAG, "no photo picker on this phone: " + e);
                    synchronized (lock) {
                        pickingPhotos = false;
                    }
                }
            }
        });
    }

    /** Null while the photo picker is up; then the URIs picked (empty for cancelled). */
    public String[] kratePhotosAnswer() {
        synchronized (lock) {
            return pickingPhotos ? null : photosPicked;
        }
    }

    /** Copy a picked photo into dir (a HEIC as a JPEG); the path, or null. */
    public String kratePhotoCopy(String uri, String dir) {
        return Photos.copyIn(this, Uri.parse(uri), dir);
    }

    /** Android 9 and before need storage access to add to the library. */
    public int krateSavePhotoAsk() {
        if (android.os.Build.VERSION.SDK_INT >= 29) {
            return GRANTED;
        }
        return krateAsk(new String[] {"android.permission.WRITE_EXTERNAL_STORAGE"}, true);
    }

    /** Add a PNG or JPEG to the library: null once added, else why not. */
    public String krateSavePhoto(byte[] data) {
        return Photos.save(this, data);
    }

    /** The on-time question: 0 none, 1 sheet up, 2 in Settings, 3 answered. */
    private int exactStep;

    @Override
    protected void onPause() {
        super.onPause();
        SignIn.paused();
    }

    @Override
    protected void onResume() {
        super.onResume();
        SignIn.resumed();
        synchronized (lock) {
            if (exactStep == 2) {
                exactStep = 3;
            }
        }
    }

    /**
     * Whether app's reminders come on time (krate:ui/reminders). On Android
     * 12 and up that needs "Alarms & reminders" for Krate: the first time an
     * app sets a reminder without it, Krate's own sheet asks, in the app's
     * name, and Allow opens the system's page for Krate. Asked once per app.
     * 0 on time, 1 near the time (may be late), 2 while the person decides.
     */
    public int krateExactAsk(String app, final String name) {
        synchronized (lock) {
            if (exactStep == 3) {
                exactStep = 0;
                return Reminder.exactAllowed(this) ? 0 : 1;
            }
            if (exactStep != 0) {
                return 2;
            }
        }
        if (Reminder.exactAllowed(this)) {
            return 0;
        }
        if (Reminder.askedBefore(this, app)) {
            return 1;
        }
        synchronized (lock) {
            exactStep = 1;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                showExactSheet(name);
            }
        });
        return 2;
    }

    private void showExactSheet(final String name) {
        final boolean[] allowed = {false};
        new android.app.AlertDialog.Builder(this)
                .setTitle(name + " wants to remind you on time.")
                .setMessage("Allow Krate to set alarms?")
                .setPositiveButton("Allow", new android.content.DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(android.content.DialogInterface dialog, int which) {
                        allowed[0] = true;
                        Intent page = new Intent(
                                android.provider.Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
                                Uri.parse("package:" + getPackageName()));
                        try {
                            synchronized (lock) {
                                exactStep = 2;
                            }
                            startActivity(page);
                            Log.i(TAG, "reminders: opened Alarms & reminders for Krate");
                        } catch (ActivityNotFoundException e) {
                            synchronized (lock) {
                                exactStep = 3;
                            }
                        }
                    }
                })
                .setNegativeButton("Not now", null)
                .setOnDismissListener(new android.content.DialogInterface.OnDismissListener() {
                    @Override
                    public void onDismiss(android.content.DialogInterface dialog) {
                        if (allowed[0]) {
                            return;
                        }
                        synchronized (lock) {
                            exactStep = 3;
                        }
                        android.widget.Toast.makeText(KrateActivity.this,
                                "Reminders from " + name + " may arrive late.",
                                android.widget.Toast.LENGTH_LONG).show();
                        Log.i(TAG, "reminders: not now, near the time");
                    }
                })
                .show();
    }

    // Video (krate:gfx/video, IC-922).

    public long krateVideoOpen(String source) {
        return Video.open(source);
    }

    public String krateVideoControl(long id, int kind, long value) {
        return Video.control(id, kind, value);
    }

    public long[] krateVideoState(long id) {
        return Video.state(id);
    }

    public String krateVideoProblem(long id) {
        return Video.problem(id);
    }

    public byte[] krateVideoFrame(long id, int[] size) {
        return Video.frame(id, size);
    }

    public void krateVideoClose(long id) {
        Video.close(id);
    }

    // A web page inside an app (krate:ui/web-page, IC-927).

    /** Web.class, so Krate can give it its one rule for where pages go. */
    public Class<?> krateWebClass() {
        return Web.class;
    }

    /** box: x, y, width, height in screen pixels. */
    public void krateWebOpen(long id, String address, int[] box) {
        Web.open(this, id, address, box);
    }

    public void krateWebPlace(long id, int[] box) {
        Web.place(this, id, box);
    }

    public void krateWebGo(long id, String address) {
        Web.go(this, id, address);
    }

    public void krateWebBack(long id) {
        Web.back(this, id);
    }

    public String[] krateWebState(long id) {
        return Web.state(id);
    }

    public void krateWebClose(long id) {
        Web.close(this, id);
    }

    public void krateWebCloseAll() {
        Web.closeAll(this);
    }

    // Appearance (krate:ui/appearance, IC-925).

    /**
     * How the person asked things to look: {dark, more contrast, text scale}
     * from the configuration (night mode, font scale) and the high-contrast
     * text setting.
     */
    public float[] krateAppearance() {
        android.content.res.Configuration c = getResources().getConfiguration();
        boolean dark = (c.uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK)
                == android.content.res.Configuration.UI_MODE_NIGHT_YES;
        int contrast = android.provider.Settings.Secure.getInt(getContentResolver(),
                "high_text_contrast_enabled", 0);
        return new float[] {dark ? 1f : 0f, contrast != 0 ? 1f : 0f, c.fontScale};
    }

    // Sign-in (krate:ui/sign-in, IC-923).

    /** Open the sign-in page in the browser: null once open, else why not. */
    public String krateSignIn(String url, String scheme) {
        return SignIn.start(this, url, scheme, Menu.appId());
    }

    /** Null while waiting; then the address sent back, "" for cancelled. */
    public String krateSignInAnswer() {
        return SignIn.answer();
    }

    // Unlock (krate:ui/unlock, IC-924).

    public boolean krateUnlockAvailable() {
        return Unlock.available(this);
    }

    public void krateUnlockCheck(String reason) {
        Unlock.check(this, reason, Menu.appName());
    }

    /** -1 while the sheet is up; 0 yes, 1 no, 2 cannot check. */
    public int krateUnlockAnswer() {
        return Unlock.answer();
    }

    public String krateUnlockWhy() {
        return Unlock.why();
    }

    // Krate's menu (IC-934).

    /** The app now running, for the menu; a null name when none is. */
    public void krateSetApp(String name, String id, String[] uses, String opens) {
        Menu.setApp(this, name, id, uses, opens);
    }

    /** Open Krate's menu over the app: a hold on the strip under the clock. */
    public void krateShowMenu() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                Menu.show(KrateActivity.this);
            }
        });
    }

    /** What the menu asked the app's thread to do: 0 nothing, 1 close. */
    public int krateMenuAction() {
        return Menu.takeAction();
    }

    /** Whether the menu asked for the app to open again once closed. */
    public boolean krateRestartAsked() {
        return Menu.restartAsked();
    }

    /** Open Krate again on the same app, in a fresh process. */
    public void krateRelaunch() {
        Menu.relaunch(this);
    }

    /** Back, while an app runs, opens Krate's menu rather than ending it. */
    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (Menu.running()) {
            Menu.show(this);
            return;
        }
        super.onBackPressed();
    }

    /** Set a reminder (krate:ui/reminders); null once set, else why not. */
    public String krateRemind(String id, String sender, String title, String body,
            long afterSeconds, boolean daily, String opens) {
        return Reminder.schedule(this, id, sender, title, body, afterSeconds, daily, opens);
    }

    public void krateRemindCancel(String id) {
        Reminder.cancel(this, id);
    }

    public String[] krateRemindPending(String prefix) {
        return Reminder.pending(this, prefix);
    }

    /**
     * Krate runs one app at a time in one activity (singleTask): a link, a
     * file or a reminder tapped while it runs comes here rather than starting
     * a second player beside the first.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        if (SignIn.redirected(intent)) {
            return;
        }
        Log.i(TAG, "opened again while running: " + intent.getAction());
    }

    /** Write the file at path into the document at uri; true when it all went. */
    public boolean krateCopyOut(String path, String uri) {
        return Documents.copyOut(this, path, Uri.parse(uri));
    }

    // Dialogs (krate:ui/dialog message and confirm).

    /** Show an app's message (OK) or question (Yes, No); see Dialog. */
    public void krateDialog(String title, String body, boolean question) {
        Dialog.show(this, title, body, question);
    }

    /** -1 while the dialog is up; then 0 for Yes or OK, 1 for anything else. */
    public int krateDialogAnswer() {
        return Dialog.answer();
    }

    // The on-screen keyboard (K-1024).

    /** Bring the keyboard up for a field (one line or many), or put it away. */
    public boolean krateTextEntry(final boolean on, final boolean many) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (typing == null) {
                    if (!on) {
                        return;
                    }
                    Typing made = new Typing(KrateActivity.this);
                    addContentView(made, new android.view.ViewGroup.LayoutParams(1, 1));
                    typing = made;
                }
                typing.set(on, many);
            }
        });
        return on;
    }

    /** What the keyboard did since the last ask; see Typing. */
    public String[] krateTyped() {
        Typing now = typing;
        return now == null ? new String[0] : now.take();
    }

    /** Whether TalkBack, or another reader that explores by touch, is on (IC-926). */
    public boolean krateScreenReaderOn() {
        android.view.accessibility.AccessibilityManager readers =
                (android.view.accessibility.AccessibilityManager)
                        getSystemService(ACCESSIBILITY_SERVICE);
        return readers != null && readers.isEnabled() && readers.isTouchExplorationEnabled();
    }
}
