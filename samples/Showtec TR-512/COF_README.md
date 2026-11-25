# Showtec TR-512 `.COF` format

A `.COF` file is a configuration backup written by the TR-512 **Save Configuration** menu. It is not a DMX show. Shows are `.NCOD` files. The manual documents the settings. It does not document the bytes.

Every file seen so far is 382 bytes. Multi-byte integers are little-endian. Strings are ASCII, NUL-padded.

Samples: `Raw_files/CONFIG/Lamp-OK.COF`, `Raw_files/CONFIG/Conf-002.COF` (factory defaults) through `Raw_files/CONFIG/Conf-011.COF`. A short synthetic configuration is `example.COF`.

Official manual: [TR-512 V3, order code 50825](https://www.techni-lux.com/files/documents/CT-SHTR512_MANUAL_GB_V3.pdf).

## File layout

| Offset | Size | Field |
| --- | ---: | --- |
| `0x00` | 8 | Magic `[Config]` |
| `0x08` | 16 | Firmware id. This device writes `YmK7003` |
| `0x0F` | 1 | `0x0B` on this firmware. Not a NUL. Unchanged by settings |
| `0x18` | 2 | Payload length = file size minus 8 (`374`) |
| `0x1A` | 32 | Device label |
| `0x3A` | 1 | Always `0` in these samples |
| `0x3B` | 1 | Display. `0` at factory, `1` in Conf-005. See below |
| `0x3C` | 1 | Display timer, in steps of 5 seconds |
| `0x3D` | 1 | Display. `0` at factory, `1` in Conf-005. See below |
| `0x3E` | 1 | Always `1` in these samples |
| `0x3F` | 1 | Always `0` in these samples |
| `0x40` | 1 | DMX port. `0` = SINGLE, `1` = HTP |
| `0x41` | 1 | DMX trigger. `0` off, `1` on |
| `0x42` | 2 | DMX trigger address, `1` .. `512` |
| `0x44` | 2 | Always `0` in these samples |
| `0x46` | 288 | 6 trigger records, 48 bytes each |
| `0x166` | 22 | Idle show file name. `No File` when empty |
| `0x17C` | 2 | Idle mode. Observed values `0`, `1`, `2` |

`0x3C` counts 5-second steps. Factory `6` is 30 seconds. Conf-007 changes only this byte, from `6` to `12` (60 seconds). Conf-005 also stores `12` there, and sets both `0x3B` and `0x3D` to `1`. Those two bytes are the other display settings (orientation and the second timer). Which byte is which is still open.

DMX trigger examples: Conf-008 is `01 01 00` (enabled, address 1). Conf-006 is `01 FD 01` (enabled, address 509). Address 509 made the byte order ambiguous; address 1 does not. The address is little-endian and the enable flag is the byte in front of it.

## Trigger record (48 bytes)

| Offset | Size | Field |
| --- | ---: | --- |
| `0` | 22 | Trigger name |
| `22` | 1 | Index, `0` .. `5` |
| `23` | 1 | Mode. `0` NO_TR, `1` NC_TR, `2` TOGGLE_TR, `3` AUTO_TR |
| `24` | 1 | Delay in seconds, `0` .. `120` |
| `25` | 1 | Status. `0` DISABLE, `1` ENABLE |
| `26` | 22 | Show file name, including `.NCOD`. `No File` when empty |

Conf-003, compared with the programmed matrix:

| Trigger | Bytes | Mode | Delay | Status | File |
| --- | --- | --- | ---: | --- | --- |
| 1 | `00 00 00 01` | NO_TR | 0 | ENABLE | `SH-001.NCOD` |
| 2 | `01 01 0A 01` | NC_TR | 10 | ENABLE | `SH-001.NCOD` |
| 3 | `02 02 14 01` | TOGGLE_TR | 20 | ENABLE | `No File` |
| 4 | `03 03 1E 00` | AUTO_TR | 30 | DISABLE | `SH-001.NCOD` |
| 5 | `04 00 78 00` | NO_TR | 120 | DISABLE | `No File` |
| 6 | `05 00 00 00` | NO_TR | 0 | DISABLE | `No File` |

Trigger 5 was requested as ENABLE and was stored as DISABLE. Mode and delay still match. Status is independent of whether a show is assigned: trigger 3 is enabled with `No File`, trigger 4 is disabled with a show.

`Lamp-OK.COF` trigger 1 is `00 01 00 01`: NC_TR, no delay, enabled, show `LAMP-OK.NCOD`.

## What each capture changed

| File | Only bytes that differ from factory Conf-002 |
| --- | --- |
| Conf-003 | The six trigger records |
| Conf-004 | `0x40` = `1` (HTP) |
| Conf-005 | `0x3B` = `1`, `0x3C` = `12`, `0x3D` = `1` |
| Conf-006 | DMX trigger on, address 509 |
| Conf-007 | `0x3C` = `12` (60 s) |
| Conf-008 | DMX trigger on, address 1 |
| Conf-009 | Idle file `SH-001.NCOD`, mode `0` |
| Conf-010 | Idle file `SH-001.NCOD`, mode `2` |
| Conf-011 | Idle file `SH-001.NCOD`, mode `1` |

Factory idle mode is `0` with `No File`. `Lamp-OK.COF` is mode `1` with `No File`. The names Scene, Show, and Passthru are not pinned to `0`, `1`, and `2` by these captures. `0` is the factory mode.

## Still unknown

- `0x0F` = `0x0B` after `YmK7003`.
- `0x3A`, `0x3E` (`1`), `0x3F`, `0x44`..`0x45`. None of them moved.
- Which of `0x3B` and `0x3D` is orientation, and which is the other display timer.
- Which idle mode value is Show, and which is Passthru.

## What a `.COF` does not contain

Recorded DMX lives in `.NCOD` files. Restoring a `.COF` does not restore a missing show. `Lamp-OK.COF` points at `LAMP-OK.NCOD`, which is not in this tree. The show present here is `SH-001.NCOD`.
