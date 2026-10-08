# ShanWan HID replay fixtures

Unchanged hardware recordings; original files in `dumps` are preserved.

| Fixture | Original | SHA-256 |
|---|---|---|
| [shanwan-2563-0555.bin](/home/idlesign/dev/1/imanlib/tests/fixtures/shanwan-2563-0555.bin) | [hid-report-descriptor.bin](/home/idlesign/dev/1/dumps/traces/receiver-pc-bd4ney0a/hid-report-descriptor.bin) | `cabd2b0847144b2596be53a7fdc5d281512357ca1bb3b44ee29816b126b5cf26` |
| [shanwan-two-pads.bin](/home/idlesign/dev/1/imanlib/tests/fixtures/shanwan-two-pads.bin) | [hidraw-02.bin](/home/idlesign/dev/1/dumps/traces/receiver-pc-bd4ney0a/hidraw-02.bin) | `7549b90b5d7c91a10c17b7c6d5b983adcd2247483eab1055a5472f0ee4a9ec0b` |
| [shanwan-four-pads.jsonl](/home/idlesign/dev/1/imanlib/tests/fixtures/shanwan-four-pads.jsonl) | [capture-02.jsonl](/home/idlesign/dev/1/dumps/traces/receiver-hub-pc-o4d1ltv3/capture-02.jsonl) | `91b11b640ab799002b1c7a03d034ca33dadef3ddec9e300bf925d3613ac2a01b` |

The 202-byte descriptor was moved from `igui/src/evdev` into fixtures: it is no
longer embedded in the production executable. The two-pad capture contains 2850
8-byte reports, including 247 consecutive records with pad 1 WEST and pad 2 EAST
held simultaneously. It has no timestamps; packet framing follows the descriptor.
The four-pad JSONL stores initial states and changes, not every repeated packet.
Start counts by `(node, report ID)` are `(hidraw3,1)=1`, `(hidraw4,1)=2`,
`(hidraw3,2)=3`, `(hidraw4,2)=4`. These node names are historical, not stable IDs.

Replay tests do not open real devices. Transport failures, queue flooding,
reconnects, relocated descriptor fields and signed-axis samples are synthetic.
The fixtures do not verify the new runtime acquisition code or complete physical
mapping of every controller. See the [two-pad report](/home/idlesign/dev/1/docs/hard/ShanWan/receiver-pc-01.md)
and [four-pad report](/home/idlesign/dev/1/docs/hard/ShanWan/receiver-pc-02.md).
