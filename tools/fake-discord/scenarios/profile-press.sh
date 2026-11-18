# A profile is asked for when the pointer goes down on a name, not when the
# click opens the popout (#249): fake.log has the request after the press,
# and the click after it sends none of its own.
waitfor identify 1
sleep 3
sleep 2
cmd "profile-press 301"
# Before any click: a timeout here is written to run.log.
waitreq GET "/api/v9/users/301/profile?type=popout" 1
cmd "profile-click 301"
sleep 2
cmd "shot $OUT/popout.png"
cmd "quit"
sleep 1
