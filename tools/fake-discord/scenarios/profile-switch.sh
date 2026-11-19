# Going from one name to the next takes one click, as in the client (#249),
# with a real pointer (pointer.py, through the compositor's remote desktop):
# a click on an avatar opens its popout, a click on another avatar with the
# popout open closes it and opens the other's (fake.log has the two profile
# requests, Alice's then Bob's), a click on the same one closes it, a click
# on nothing in particular closes it, and so does Escape; the note box in the
# popout still takes a click and typing.
python3 $HERE/pointer.py serve $OUT/pointer.fifo > $OUT/pointer.log 2>&1 &
POINTER=$!
# A line to the pointer's fifo, and time for it to be done.
ptr() { echo "$*" > $OUT/pointer.fifo; sleep 2; }
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
ptr move 650 450
ptr click 350 221
waitreq GET "/api/v9/users/301/profile?type=popout" 1
sleep 1
cmd "shot-profile $OUT/first.png"
# The note box in the popout takes a click and keys, as it did when the popout grabbed the keyboard.
ptr click 350 467
ptr key h
ptr key e
ptr key y

cmd "shot-profile $OUT/note.png"
ptr click 350 165
waitreq GET "/api/v9/users/302/profile?type=popout" 1
sleep 1
cmd "shot-profile $OUT/second.png"
ptr click 350 165
sleep 1
cmd "shot-profile $OUT/same-closed.png"
ptr click 350 221
sleep 1
ptr click 900 600
sleep 1
cmd "shot-profile $OUT/outside-closed.png"
ptr click 350 221
sleep 1
ptr key Escape
sleep 1
cmd "shot-profile $OUT/escape-closed.png"
cmd "quit"
echo quit > $OUT/pointer.fifo
sleep 1
