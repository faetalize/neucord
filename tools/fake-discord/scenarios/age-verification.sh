# Confirming an age (#277). Run with FAKE_NSFW="status=1,gated=1" (the region asks an account whose age is unknown to
# confirm it, so the gate has Confirm Age) and FAKE_VARIANTS=ageverify=1 (the client's expressive modal). Confirm Age
# lists the methods (GET /age-verification/methods/v2), picking one POSTs `{"method": 1, "vendor": "K_ID"}` to
# /age-verification/verify/v2 and shows the vendor's page, which says it has a result (the methods close and the
# account becomes an adult's: USER_UPDATE) and then that it is done (the page closes); the gate gives way to the
# channel. Without the variant there is no Confirm Age. (The vendor's page is a web view, which can't be photographed
# headless: a shot of it never finishes.)
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "channel 604"
sleep 1
cmd "gate-state"
cmd "shot $OUT/gate.png"
if [ -n "${AGE_FLOW:-}" ]; then
  cmd "gate-press Confirm Age"
  waitreq GET /api/v9/age-verification/methods/v2 1
  sleep 1
  cmd "age-state"
  cmd "shot $OUT/methods.png"
  cmd "age-pick 0"
  waitreq POST /api/v9/age-verification/verify/v2 1
  waitfor age-page 1
  sleep 2
  cmd "age-state"
  waitfor age-done 1
  sleep 2
  cmd "age-state"
  cmd "gate-state"
  cmd "shot $OUT/after-result.png"
  sleep 7
  cmd "age-state"
  cmd "gate-state"
  cmd "shot $OUT/done.png"
fi
cmd "quit"
sleep 1
