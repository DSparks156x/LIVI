# SDLink firmware screen

The SDLink firmware screen shows what carpid reports for the RNS-E and the Teensy and
starts carpid's firmware jobs. It is reached from Software Update and A/V over SD, with
Back returning to the originating screen. A/V over SD also shows the installed RNS-E
identity and the protocol currently in use.

## Wire contract (carpi-proto)

| Ask | carpid answers | Lands in |
| --- | --- | --- |
| `Firmware { action: status }` | `Response::Firmware { status }` at once | `Snapshot::firmware` |
| `Firmware { action: probe }` | `JobStarted`, then `Job` events | `Snapshot::firmware_job` |
| `Firmware { action: install_rnse { confirm: true } }` | same | same |
| `Firmware { action: install_teensy { confirm: true } }` | same | same |

Job events are `Stage`, `Progress`, `Firmware(status)` and finally `Finished { ok, message }`.
Every status reply and every `JobEvent::Firmware` replaces `Snapshot::firmware` as a whole;
the UI never merges fields from two statuses. One firmware job at a time: the client does not
send a second while one runs, and carpid refuses one anyway. When carpid goes away the status
is cleared and a running job is shown as not finished.

Opening the screen asks for the status, and starts a probe unless a firmware job is running
or one was asked for or ended in the last 60 s. "Check versions" starts a probe by hand.

## Fields

| UI field | Backend authority |
| --- | --- |
| `rnse.installed` | Verified RNS-E module header / handshake identity, or explicitly detected `Legacy`; unknown on failed/missing discovery |
| `teensy.installed` | Verified Teensy firmware identity; unknown until discovered |
| `protocol` | Protocol actually selected for the current SDLink connection, including explicit legacy fallback |
| `*.bundled` | Validated matching image manifest from the bundled SDLink Actions artifacts |
| `*.comparison` | Backend decision: bundle newer, same, older, incompatible, or unknown |
| `*.detail` | carpid's one line about the installed side; shown as is |
| `*.blocked` | Why carpid would refuse an install now; shown as "Can't install" |
| `busy` | carpid is running a firmware job, possibly one another client started |

Version labels and build IDs are display strings. Build hashes and required table
versions do not establish chronological ordering. The UI never derives a comparison
from those strings, from the negotiated protocol, or from the sector 107 CRC read.
That CRC read remains under A/V over SD and is labeled **Diagnostic only**.
A legacy installation can receive an offer if the backend establishes `Newer` by
authoritative metadata; a failed probe must remain `Unknown`, not `Legacy`.

carpid publishes installed identity, bundled identity, and comparison as one consistent
status, invalidates comparison whenever either identity changes, and clears
connection-scoped protocol state on disconnect/reconnect. A known older firmware stays
usable when the negotiated link is compatible; update availability does not gate streaming.

## Installing

An install button is shown for a unit only when all of these hold:

- `bundled` is present,
- `blocked` is `None`,
- no firmware job is running (`busy` is false and this client has none running),
- `comparison` is not `Same` or `Incompatible`.

`Newer` shows **Install update**. `Unknown` and `Older` show **Replace with bundled**, which
says it overwrites whatever is installed with the bundled image. The UI does not second-guess
carpid beyond these rules; carpid checks its own prerequisites and reports them in `blocked`.

Every install takes two pushes, the same pattern as Move to NVMe: the first push shows what
will happen and turns the button into "Push again to …"; only the second sends the request
with `confirm: true`. Any other act, or leaving the screen, disarms it.

- RNS-E: ignition on, engine off, don't turn the car off or cut power; the RNS-E screen goes
  dark and it restarts; it takes several minutes.
- Teensy: video stops briefly while the Teensy reflashes.

While a job runs the screen shows its stage, message and percent (and, for an install, the
same warnings); after it ends, the final message, marked as an alert when it failed. Firmware
installs are separate from the system-image updater; do not route them through it.

## Advanced: Flash Sector

- Source: `Status::uploads`, the portal's `[prefix]sector_<N>.bin` files (carpid: sector,
  size, CRC, `problem`).
- Section shown only when there are uploads. First row: **Raw Write**, "No content checks".
- One row per upload: name, "Sector N · CRC X". With a `problem`: alert row, no button.
- **Flash**: two pushes like an install. First shows the RNS-E warnings and relabels to
  "Push again to flash sector N"; second sends `flash_sector { name, confirm: true }`.
- Job title **Flash Sector**, with the RNS-E warnings while running. No buttons while busy.

## Fixtures

`fixtures/firmware-newer.json` and `fixtures/firmware-legacy.json` contain synthetic
presentation data, not release metadata or real firmware. Deserialize them with
`client::Fixture::read`, then use `firmware::view(&fixture.firmware, &FirmwareJob::default(),
None)` for a standalone view, or populate a snapshot and use `Pages::goto(CarScreen::Firmware)`.
Fixture mode answers status from the file and refuses probe and install jobs.
