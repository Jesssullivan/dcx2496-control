#!/usr/bin/env python3
"""Build a dcx.desired-profile/v2 envelope (digest per peq_bank.rs bank_digest).

Usage: serial_probe_v2.py OUT PROFILE_ID REVISION OUTPUT ACTIONS_JSON
dcxctl fully revalidates it (O4 only, domain, cut-only, reviewed addresses,
no newly active stored boost) on `control diff`."""
import hashlib, json, sys
out, profile_id, revision, output, actions = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), json.loads(sys.argv[5])
channel = 4 + output
h = hashlib.sha256(b"dcx2496.desired-profile/v2\0")
for t in (profile_id, revision):
    h.update(bytes([len(t)])); h.update(t.encode())
h.update(bytes([output, channel, len(actions)]))
for a in actions:
    h.update(bytes([a["channel"], a["parameter"]])); h.update(int(a["value"]).to_bytes(2, "big"))
doc = {"schemaVersion": "dcx.desired-profile/v2", "profileID": profile_id, "revision": revision,
       "digest": "sha256/" + h.hexdigest(),
       "document": {"target_output": output, "parameter_channel": channel, "actions": actions}}
json.dump(doc, open(out, "w"), indent=2)
