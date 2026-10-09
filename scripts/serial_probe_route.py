#!/usr/bin/env python3
"""Decode route/safety candidates from a SnapshotV1 JSON (DuinoDCX 00b9d70 offsets).

Unverified transcription offsets outside the reviewed allowlist: a probe
precondition aid only, never evidence. Prints JSON; never prints raw frames or
identity bytes."""
import json, sys
snap = json.load(open(sys.argv[1]))
d0, d1 = snap["dump0"]["frame"], snap["dump1"]["frame"]
def b(part, off): return (d0 if part == 0 else d1)[off]
mutes = {f"O{n+1}": b(p, o) for n, (p, o) in enumerate([(0,715),(0,885),(1,54),(1,223),(1,392),(1,561)])}
src_names = {0:"A",1:"B",2:"C",3:"SUM"}
sources = {f"O{n+1}": src_names.get(b(p,o), f"raw{b(p,o)}") for n,(p,o) in enumerate([(0,857),(1,26),(1,195),(1,365),(1,534),(1,703)])}
sum_names = ["off","A","B","C","A+B","A+C","B+C"]
out = {
  "snapshot_digest": snap["snapshot_digest"],
  "device_id": snap["device_id"],
  "mapping": "DuinoDCX 00b9d70 setupLocations/outputLocations; frame offsets incl. header",
  "input_c_gain_raw": b(0,121),
  "input_c_gain": {0:"line",1:"mic"}.get(b(0,121), "unknown"),
  "input_sum_type_raw": b(0,117),
  "input_sum_type": sum_names[b(0,117)] if b(0,117) < 7 else "unknown",
  "setup_mute_outs_raw": b(0,57),
  "output_mute_raw": mutes,
  "output_muted": {k: (v == 1) if v in (0,1) else "unknown" for k, v in mutes.items()},
  "output_source": sources,
  "auto_align_and_phantom": "no direct parameter or dump field exists (protocol setup params 0x02-0x0B,0x14-0x18); +15V is engaged only during an Auto Align run, which forces Input C to mic",
}
json.dump(out, sys.stdout, indent=2); print()
