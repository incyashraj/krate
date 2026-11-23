package tech.krate.player;

import android.graphics.ImageFormat;
import android.media.Image;
import android.media.ImageReader;
import android.media.MediaMetadataRetriever;
import android.media.MediaPlayer;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;

import java.util.HashMap;

/**
 * Video playback (krate:gfx/video, IC-922): the platform's MediaPlayer
 * decodes and plays the sound, its pictures go into an ImageReader, and
 * each newest frame is packed as tight I420 for Rust to turn to RGBA, which
 * the app draws by handle. Each video has a thread of its own.
 */
final class Video {
    private Video() {}

    private static final class Player {
        HandlerThread thread;
        Handler handler;
        MediaPlayer media;
        ImageReader reader;
        int width;
        int height;
        boolean ready;
        boolean ended;
        boolean looping;
        /** What the app asked for before the player was ready: applied once it is. */
        boolean playWanted;
        boolean muted;
        /** How loud, 0 to 1 (K-1351); muted wins. */
        float volume = 1f;
        long seekWanted = -1;
        String problem = "";
        byte[] latest;
        int latestWidth;
        int latestHeight;
    }

    private static final HashMap<Long, Player> PLAYERS = new HashMap<>();
    private static long next;

    /** Start loading source (a file path or an https address); the video's handle. */
    static long open(final String source) {
        final Player p = new Player();
        final long id;
        synchronized (PLAYERS) {
            id = ++next;
            PLAYERS.put(id, p);
        }
        p.thread = new HandlerThread("krate-video-" + id);
        p.thread.start();
        p.handler = new Handler(p.thread.getLooper());
        p.handler.post(new Runnable() {
            @Override
            public void run() {
                load(p, source);
            }
        });
        return id;
    }

    private static void fail(Player p, String why) {
        Log.w(KrateActivity.TAG, "video: " + why);
        synchronized (p) {
            p.problem = why;
        }
    }

    private static void load(final Player p, String source) {
        int w = 0;
        int h = 0;
        MediaMetadataRetriever probe = new MediaMetadataRetriever();
        try {
            if (source.startsWith("https://")) {
                probe.setDataSource(source, new HashMap<String, String>());
            } else {
                probe.setDataSource(source);
            }
            w = parse(probe.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH));
            h = parse(probe.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT));
        } catch (RuntimeException e) {
            fail(p, "this video cannot be played: " + e.getMessage());
            return;
        } finally {
            try {
                probe.release();
            } catch (java.io.IOException | RuntimeException e) {
                Log.w(KrateActivity.TAG, "video: " + e);
            }
        }
        if (w <= 0 || h <= 0) {
            fail(p, "this file has no picture");
            return;
        }
        synchronized (p) {
            p.width = w;
            p.height = h;
        }
        p.reader = ImageReader.newInstance(w, h, ImageFormat.YUV_420_888, 3);
        p.reader.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
            @Override
            public void onImageAvailable(ImageReader r) {
                Image image = r.acquireLatestImage();
                if (image == null) {
                    return;
                }
                try {
                    byte[] packed = Cam.pack(image);
                    synchronized (p) {
                        p.latest = packed;
                        p.latestWidth = image.getWidth();
                        p.latestHeight = image.getHeight();
                    }
                } finally {
                    image.close();
                }
            }
        }, p.handler);
        p.media = new MediaPlayer();
        try {
            p.media.setDataSource(source);
            p.media.setSurface(p.reader.getSurface());
            p.media.setOnPreparedListener(new MediaPlayer.OnPreparedListener() {
                @Override
                public void onPrepared(MediaPlayer mp) {
                    boolean play;
                    long seek;
                    synchronized (p) {
                        p.ready = true;
                        play = p.playWanted;
                        seek = p.seekWanted;
                        mp.setLooping(p.looping);
                        float v = p.muted ? 0f : p.volume;
                        mp.setVolume(v, v);
                    }
                    if (seek >= 0) {
                        mp.seekTo(seek, MediaPlayer.SEEK_CLOSEST);
                    }
                    if (play) {
                        mp.start();
                    }
                    Log.i(KrateActivity.TAG, "video: ready, " + mp.getDuration() + " ms, "
                            + p.width + "x" + p.height);
                }
            });
            p.media.setOnCompletionListener(new MediaPlayer.OnCompletionListener() {
                @Override
                public void onCompletion(MediaPlayer mp) {
                    synchronized (p) {
                        p.ended = !p.looping;
                    }
                }
            });
            p.media.setOnErrorListener(new MediaPlayer.OnErrorListener() {
                @Override
                public boolean onError(MediaPlayer mp, int what, int extra) {
                    fail(p, "this video stopped (" + what + ", " + extra + ")");
                    return true;
                }
            });
            p.media.prepareAsync();
        } catch (java.io.IOException | RuntimeException e) {
            fail(p, "this video cannot be played: " + e.getMessage());
        }
    }

    private static int parse(String text) {
        try {
            return text == null ? 0 : Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    private static Player get(long id) {
        synchronized (PLAYERS) {
            return PLAYERS.get(id);
        }
    }

    /** kind 0 play, 1 pause, 2 seek to value ms, 3 loop (value 1/0), 4 mute (value 1/0). */
    static String control(long id, final int kind, final long value) {
        final Player p = get(id);
        if (p == null) {
            return "no such video";
        }
        p.handler.post(new Runnable() {
            @Override
            public void run() {
                boolean ready;
                synchronized (p) {
                    // Remembered either way: a call before the player is
                    // ready is applied when it is (MediaPlayer refuses it
                    // until then, and the refusal read as "stopped").
                    switch (kind) {
                        case 0:
                            p.playWanted = true;
                            break;
                        case 1:
                            p.playWanted = false;
                            break;
                        case 2:
                            p.seekWanted = value;
                            break;
                        case 3:
                            p.looping = value != 0;
                            break;
                        case 5:
                            // Thousandths, 0 to 1000.
                            p.volume = Math.max(0f, Math.min(1f, value / 1000f));
                            break;
                        default:
                            p.muted = value != 0;
                            break;
                    }
                    ready = p.ready;
                }
                Log.d(KrateActivity.TAG, "video: control " + kind + " " + value
                        + (ready ? "" : " (held until ready)"));
                if (p.media == null || !ready) {
                    return;
                }
                try {
                    switch (kind) {
                        case 0:
                            synchronized (p) {
                                p.ended = false;
                            }
                            p.media.start();
                            break;
                        case 1:
                            p.media.pause();
                            break;
                        case 2:
                            synchronized (p) {
                                p.ended = false;
                            }
                            p.media.seekTo(value, MediaPlayer.SEEK_CLOSEST);
                            break;
                        case 3:
                            synchronized (p) {
                                p.looping = value != 0;
                            }
                            p.media.setLooping(value != 0);
                            break;
                        default:
                            float v;
                            synchronized (p) {
                                v = p.muted ? 0f : p.volume;
                            }
                            p.media.setVolume(v, v);
                            break;
                    }
                } catch (IllegalStateException e) {
                    Log.w(KrateActivity.TAG, "video: not yet: " + e);
                }
            }
        });
        return null;
    }

    /** {position, duration, width, height, ready, playing, ended}; null for no such video. */
    static long[] state(long id) {
        Player p = get(id);
        if (p == null) {
            return null;
        }
        long position = 0;
        long duration = 0;
        boolean playing = false;
        synchronized (p) {
            if (p.ready && p.media != null) {
                try {
                    position = p.media.getCurrentPosition();
                    duration = p.media.getDuration();
                    playing = p.media.isPlaying();
                } catch (IllegalStateException e) {
                    Log.w(KrateActivity.TAG, "video: " + e);
                }
            }
            return new long[] {position, Math.max(0, duration), p.width, p.height,
                p.ready ? 1 : 0, playing ? 1 : 0, p.ended ? 1 : 0};
        }
    }

    static String problem(long id) {
        Player p = get(id);
        if (p == null) {
            return "no such video";
        }
        synchronized (p) {
            return p.problem;
        }
    }

    /** {width, height} of the newest frame, then its I420 bytes; null when none is new. */
    static byte[] frame(long id, int[] size) {
        Player p = get(id);
        if (p == null) {
            return null;
        }
        synchronized (p) {
            byte[] out = p.latest;
            p.latest = null;
            size[0] = p.latestWidth;
            size[1] = p.latestHeight;
            return out;
        }
    }

    static void close(long id) {
        final Player p;
        synchronized (PLAYERS) {
            p = PLAYERS.remove(id);
        }
        if (p == null) {
            return;
        }
        p.handler.post(new Runnable() {
            @Override
            public void run() {
                if (p.media != null) {
                    p.media.release();
                }
                if (p.reader != null) {
                    p.reader.close();
                }
                p.thread.quitSafely();
                Log.i(KrateActivity.TAG, "video: closed");
            }
        });
    }
}
