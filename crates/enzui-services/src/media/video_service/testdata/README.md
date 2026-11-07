`paused-seek.flv` is synthetic test media: four seconds of generated video and
sine-wave audio, 176×144 at 25 fps, Sorenson Spark and MP3. It contains a keyframe
index and one-second keyframe spacing. No third-party media is included.

An equivalent fixture can be generated with:

```sh
ffmpeg -f lavfi -i testsrc2=size=176x144:rate=25 -f lavfi -i sine=frequency=440:sample_rate=44100 -t 4 -c:v flv -g 25 -c:a libmp3lame -flvflags add_keyframe_index paused-seek.flv
```

Some FLV backends accept an accurate seek and complete at a different position.
Tests require a truthful outcome and a subsequent usable seek; a backend that
supports precise FLV seeking is also valid.
