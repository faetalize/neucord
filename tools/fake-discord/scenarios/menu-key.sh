# The menus of a sidebar row and of a rail button, opened from the keyboard
# (#243): `menu-key` focuses the nth one and runs its Menu / Shift+F10
# shortcut, which says whether a menu opened and shoots it.
waitfor identify 1
sleep 3
cmd "guild 500"
sleep 2
cmd "menu-key row 0 $OUT/channel-menu.png"
sleep 1
cmd "menu-key row 2 $OUT/channel-menu-2.png"
sleep 1
cmd "menu-key rail 1"
sleep 2
cmd "shot $OUT/rail-menu.png"
cmd "quit"
sleep 1
