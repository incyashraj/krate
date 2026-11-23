# Portable player fixtures (K-1348)

Tiny files the unit tests in `crates/runtime/src/video_portable/` decode.
Made with ffmpeg 8 (Homebrew) on 2026-11-22; 44 KB together.

    clip.mp4   64x64 H.264 High, 30 fps, 2 B-frames, key frame every 15, 1 s,
               with a 1 s 440 Hz AAC mono sound:
      ffmpeg -f lavfi -i "testsrc2=size=64x64:rate=30:duration=1" \
             -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" \
             -c:v libx264 -profile:v high -pix_fmt yuv420p -bf 2 -g 15 -crf 30 \
             -c:a aac -b:a 32k -ac 1 -map_metadata -1 -fflags +bitexact \
             -flags:v +bitexact -flags:a +bitexact -movflags +faststart clip.mp4
    tone.mp3   1 s 440 Hz, 22.05 kHz mono: -c:a libmp3lame -b:a 32k -ac 1
    tone.wav   1 s 440 Hz, 8 kHz mono 16-bit: -c:a pcm_s16le -ac 1
    tone.ogg   1 s 440 Hz, 22.05 kHz stereo Vorbis: -c:a vorbis -strict -2 -ac 2
    hevc.mp4   64x64 HEVC tagged hvc1, as an iPhone writes it, no sound:
      ffmpeg -f lavfi -i "testsrc2=size=64x64:rate=30:duration=1" -c:v libx265 \
             -pix_fmt yuv420p -crf 35 -tag:v hvc1 -an -map_metadata -1 hevc.mp4

The first frame's reference colours in picture.rs come from
`ffmpeg -i clip.mp4 -frames:v 1 -f rawvideo -pix_fmt rgba -`.
