# One look at the app under the compositor the run was started with
# (COMPOSITOR=mutter|gnome-shell|niri, #20), with FAKE_NOTIFY=10: the window
# as the app and as the compositor shoot it, a channel's menu from the
# keyboard, the notification the fake provokes (its calls to
# org.freedesktop.Notifications are in notifications.log; the replies are not,
# a server's name in bus-names.txt says there is one), and what the app
# put on the session bus (its tray item, if the app made one).
dbus-monitor --session "interface='org.freedesktop.Notifications'" > $OUT/notifications.log 2>&1 &
MONITOR=$!
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "shot $OUT/window.png"
cshot $OUT/window-compositor.png
cmd "menu-key row 0 $OUT/menu.png"
sleep 8
cshot $OUT/after-notification.png
gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus -m org.freedesktop.DBus.ListNames > $OUT/bus-names.txt
kill $MONITOR
cmd "quit"
sleep 1
