# An app's form: run with FAKE_INTERACTION=modal (and FAKE_MODAL_SUBMIT=fail|silent|429 for the other answers).
# Pressing Formy's button in 600 opens it; Submit first shows what is missing, then sends it filled in.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "press 1456074464952320041 open-form"
waitreq POST /api/v9/interactions 1
sleep 1
cmd "modal-fields"
cmd "modal-shot $OUT/form.png"
cmd "modal-submit"
cmd "modal-fields"
cmd "modal-shot $OUT/form-errors.png"
cmd "modal-set 0,0 Ann"
cmd "modal-set 1,0 hey"
cmd "modal-check 4,0 1"
cmd "modal-check 5,0 0" "modal-check 5,0 2" "modal-check 6,0 0" "modal-pick 2,0 0"
cmd "modal-fields"
cmd "modal-set 1,0 hello there"
cmd "modal-shot $OUT/form-filled.png"
cmd "modal-submit"
waitreq POST /api/v9/interactions 2
sleep 2
cmd "modal-fields"
cmd "modal-shot $OUT/form-answered.png"
sleep 10
cmd "modal-fields"
cmd "shot $OUT/after.png"
cmd "quit"
sleep 1
