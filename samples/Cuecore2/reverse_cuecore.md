# CueCore2 track format (reverse engineering)

Status: opcode set below is confirmed by `chezphilippe.xml` and every track in `chezphilippe2.xml`. The decoder matches each track's `frames` attribute. There is still no published specification, and nothing here has been restored onto a CueCore2.

Implementation: `src/devices/cuecore2/track.rs`, `src/devices/cuecore2/xml.rs`.

## Backup XML

- Root: `<core device="CueCore2" …>`.
- `<tracks intensity_map="…">` then one `<track …>base64</track>` per recording.
- `intensity_map` in `chezphilippe2.xml` decodes to 256 zero bytes. The importer ignores it.
- `filesize` is the decoded blob length. `sample_rate` is the recording FPS. `frames` is the number of `d0` ticks.
- Time: `at_ms = tick_index × 1000 / sample_rate`.

## Track blob

```
opcode = kind | page
page 0 → DMX 1..256, page 1 → DMX 257..512
address = channel index inside the page (0 = first channel of the page)
```

| Opcode kind | Payload | Meaning |
| --- | --- | --- |
| `0x10` | address, level | Set one channel. |
| `0x20` | address | Set one channel to 0. |
| `0x30` | address, count, levels | Set `count` channels from that address. |
| `0x40` | address, count | Set `count` channels to 0. No level bytes follow. |
| `0xd0` |  | One playback tick at the levels held so far. |
| `0xe0` |  | End. Last byte of every blob in both samples. |

`0xc0` is an escape for the next logical byte. It is used for an address, a count, or a level whose value is `0xc0`, `0xd0`, or `0xe0`. Any other byte, including `0x10`, `0x30`, `0x40`, and `0xff`, is stored raw. The earlier “five extra `0xc0` filler” and “`40 00` is a hold” readings were this escape and the zero-range opcode seen through an 8-channel window.

`chezphilippe.xml` therefore does go to black: channels 1–8 are 0 from tick 1214 through 1318, then full, then 0 again from tick 1414 through the last tick 1546. Skipping `40 00 08` hid both clears.

The encoder in this repo writes the same opcodes, including the escape, but it groups changed channels into its own runs. A decoded timeline round-trips. The bytes do not match a device blob.

## What `chezphilippe2.xml` confirms

All ten tracks are `version="1.46"`, `sample_rate="40"`.

| Track | Label | Check |
| --- | --- | --- |
| 1 | Off | 189 ticks, no keyframe, levels stay 0. |
| 2 | Moving | 13231 ticks, channels 1–8 only, variable run lengths, single-channel `0x10`. |
| 3 | Static | One 8-channel keyframe, then only ticks. Levels `137,137,137,53,137,137,137,137`. |
| 17 | all512 | Whole universe steps together, including real zero frames where the wave crosses 0 (`0x40` on every 32-channel block). |
| 18 | value208 | All 512 channels held at 208. Every level is the escape `c0 d0`. Addresses 192 and 224 are escaped too. |
| 19 | nochangeandacha | 5.2 s of ticks at 0, then all channels 255, back to 0, 255 again, then 0. |
| 20 | single channel | Only channel 1. Zero uses `0x20`. Level 192 is `c0 c0`. |
| 21 | 400to512 | Page bit set (`0x31`). Channels 400–512 move, 1–399 stay 0. |
| 22 | fan512 | Narrow moving lobe. In this file it never leaves channels 1–256. |
| 23 | fan512quantized | Exactly one channel at 255, walking from channel 387 up across the page boundary through channel 512. |

The app asks which track to import. Export writes a single `<tracks>` document at 40 FPS, not a full device backup.

## Still open

- Restore onto hardware, and a binary diff against what vManager writes back (was M, N).
- `sample_rate` other than 40. Both files are 40 FPS.
- A `<track version>` other than 1.46.
- What `intensity_map` is when it is not 256 zeros.
- Whether a console scene is a different blob than a one-keyframe recorder track.
- Byte-identical output. Logical levels match; chunk sizes do not.
