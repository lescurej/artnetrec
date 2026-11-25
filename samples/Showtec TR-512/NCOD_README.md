# Showtec TR-512 `.NCOD` format

A `.NCOD` file is a DMX show. The TR-512 records one, and plays it back from a trigger or from the idle show named in a `.COF`. It is not a configuration file. The manual names the files. It does not document the bytes.

Multi-byte integers are big-endian. A frame is packed tight against the next one. There is no checksum and no file footer.

Sample: `Raw_files/RECORDER/SH-001.NCOD`. 641,418 bytes, 1,207 frames, last timestamp 43,739 ms. Channels 1–4 carry the show. The rest of the universe stays at `0`. A short synthetic show is `example.NCOD`.

## File layout

| Offset | Size | Field |
| --- | ---: | --- |
| `0x00` | 4 | `0` in the sample |
| `0x04` | 4 | End time, milliseconds. Equals the last frame timestamp |
| `0x08` | 4 | Same end time again |
| `0x0C` | 2 | Universe width. `512` in the sample |
| `0x0E` | 498 | `0` in the sample |
| `0x200` | — | Frames, back to back, until EOF |

`SH-001` stores `00 00 AA DB` at both `0x04` and `0x08`. `0x0000AADB` is 43,739, the timestamp of frame 1,207. `0x0C` is `02 00`, which is 512. A frame may store fewer than 512 slots. The header width stays 512.

## Frame

| Offset | Size | Field |
| --- | ---: | --- |
| `0` | 8 | Magic `framedmx` |
| `8` | 1 | `0` in every frame of the sample |
| `9` | 2 | Slot count, `1` .. `512`. Number of DMX bytes that follow |
| `11` | 4 | Gap since the previous frame, in units of 0.01 ms |
| `15` | 4 | Absolute time, milliseconds from the start of the show |
| `19` | *n* | DMX slots. Slot 1 is the first byte |

`n` is the slot count at offset 9. The next magic starts at `19 + n`. In `SH-001` that count is 512 for 1,200 frames, 511 for 3, and 510 for 4. Every shorter frame still contains channels 1–4, and the dropped tail is `0`. Missing slots are zeros.

The gap and the absolute time describe the same step. For every frame, including the first:

```text
gap = (at_ms - previous_at_ms) * 100
```

The first frame has no predecessor, so its previous time is 0. Frame 1 is at 22 ms with gap `2200`. Frame 2 is at 45 ms with gap `2300`. Gaps in this recording run from 22 ms to 364 ms. Times only increase.

First frame of `SH-001`, at file offset `0x200`:

```text
66 72 61 6d 65 64 6d 78   framedmx
00                        0
02 00                     512 slots
00 00 08 98               gap 2200 = 22.00 ms
00 00 00 16               at 22 ms
68 ad ff a0 00 00 ...     DMX
```

## Playback

Wait until the absolute timestamp, then emit the 512 slots. The gap is the same information and is not required to play the file. This repo does that in `src/ncod.rs`: it checks `framedmx`, reads the slot count and the timestamp, and zero-fills any slot the frame did not store. It does not read the 512-byte header or the gap.

## Still unknown

- The 4 bytes at `0x00`, the 498 bytes after the universe width, and the byte at frame offset 8. All of them are `0` in `SH-001`.
- Why the end time is stored twice.
- Whether a short frame may drop a non-zero slot. This recording never does.
