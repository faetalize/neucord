# A forced password change (#310): the form over the app, then what it sends,
# PATCH /users/@me with only the current and the new password: refused for
# a wrong current password with Discord's own words, and answered with the
# new token for the right one. fake.log has both requests.
waitfor identify 1
sleep 3
cmd "ask-password"
sleep 1
cmd "shot $OUT/password.png"
cmd "password-send wrong new-password"
waitreq PATCH /api/v9/users/@me 1
sleep 1
cmd "password-send old-password new-password"
waitreq PATCH /api/v9/users/@me 2
sleep 1
cmd "quit"
sleep 1
