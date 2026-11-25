# DMX Recorder show file

`.jsonl` is the application file. A player format such as Showtec TR-512 `.NCOD` is an import and an export. The editor works on this file, then writes a device file when a recorder needs one.

The encoding is [JSON Lines](https://jsonlines.org/): UTF-8, one JSON object per line, LF or CRLF. A text editor or an AI can change a look by editing that line.

## Layout

The first line is the header. Every later line is one look, at an absolute time, for one universe of 512 channels.

```json
{"dmx-recorder":1}
{"at_ms":0,"channels":{"2":255}}
{"at_ms":500,"channels":{"1":64,"2":192}}
{"at_ms":1000,"channels":{"1":128,"2":128}}
{"at_ms":1500,"channels":{"1":192,"2":64}}
{"at_ms":2000,"channels":{"1":255}}
```

`samples/example.jsonl` is that show.

| Field | Meaning |
| --- | --- |
| `dmx-recorder` | Format version. This version is `1`. |
| `at_ms` | Milliseconds from the start of the show. An integer. Times do not go backwards. |
| `channels` | DMX levels that are on. The key is the channel, `1` to `512`. The value is `0` to `255`. |

A missing channel is `0`. The writer leaves out zeros, so a look stores the channels that are up, not all 512. `"4":0` is legal and means off. Unknown keys are ignored.

A look that does not change is not repeated. A scene held for several hours is one line. The file grows with the number of changes, not with the length of the show and not with the channels that stay at zero.

A show that rewrites all 512 channels on every line for hours is large in any text format. That is the recording itself. This file does not also pay for the channels and the time where nothing happened.

## Version

A file with a version other than `1` is refused. Adding a key does not require a new version. Changing the meaning of `at_ms` or `channels` does.
