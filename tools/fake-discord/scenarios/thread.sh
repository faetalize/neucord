# /thread (#329), with FAKE_OWNER=1 so threads may be started: the thread is
# made from the command (its body in fake.log), then its first message goes
# to it, and the command leaves the box once the thread is here.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /thread "
sleep 1
cmd "type Plans"
cmd "key Tab"
cmd "type first words"
cmd "line"
cmd "shot $OUT/thread-command.png"
cmd "key Return"
waitreq POST /api/v9/channels/600/threads 1
sleep 2
sleep 1
cmd "line"
cmd "shot $OUT/thread-made.png"
cmd "quit"
sleep 1
