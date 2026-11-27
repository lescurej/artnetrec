# CueCore2 vManager backup

Visual Productions backups are XML. Each `<track>` body is a base64 blob of one DMX recording, up to 512 channels.

`chezphilippe.xml` is one 40 FPS Art-Net track on channels 1–8. `chezphilippe2.xml` is the same device with ten labeled tracks used to pin the codec down.

## Track blob

A logical byte is raw, unless it is `c0`, `d0`, or `e0`. Those three are written as `c0` plus the byte, so a tick or the end marker cannot appear inside an address, a count, or a level. DMX 192, 208, and 224 are the levels that take the escape.

The opcode byte is `kind | page`. `page` is 0 for channels 1–256 and 1 for channels 257–512. The address byte is the channel inside that page, starting at 0.

| Kind | Bytes after the opcode | Effect |
| --- | --- | --- |
| `10` | address, level | Set one channel. |
| `20` | address | Set one channel to 0. |
| `30` | address, count, `count` levels | Set a contiguous range. |
| `40` | address, count | Set a contiguous range to 0. |
| `d0` |  | Emit one sample at the current levels. |
| `e0` |  | End of the blob. Nothing follows it. |

`count` is at most 255, and a command never crosses the 256-channel page. A hold is just more `d0` bytes. Channels that are not written keep their level, which starts at 0, so an all-zero track is only ticks.

`<track frames>` is the tick count, `filesize` is the decoded byte length, and `sample_rate` is the FPS. Editor time is `tick_index * 1000 / sample_rate` milliseconds.

Opening a backup asks which track to load, including a file that has only one. Export writes that track by itself at 40 FPS:

```xml
<?xml version="1.0" ?>
<tracks source="Art-Net">
    <track index="1" version="1.46" frames="…" filesize="…" sample_rate="40">…</track>
</tracks>
```
