# A thread's member list (#326), with FAKE_THREADS=1 FAKE_THREAD_MEMBERS=1 FAKE_OWNER=1: opening an active thread
# with the members pane showing asks for its list in op 37 (`thread_member_lists`), and the pane lists who is in
# it by role, then online, then offline (one the client has no member of isn't listed); Remove from Thread in a
# person's menu, which the owner has, sends DELETE .../thread-members/<id>?location=Context%20Menu and the list
# loses them.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "channel 7201"
waitfor gateway 1
sleep 2
cmd "shot $OUT/thread-members.png"
cmd "member-menu 302"
sleep 1
cmd "shot-popover members_pane $OUT/thread-user-menu.png"
cmd "member-menu-item remove-from-thread"
waitreq DELETE /api/v9/channels/7201/thread-members/302 1
sleep 2
cmd "shot $OUT/thread-members-after.png"
cmd "quit"
sleep 1
