# A command's options as pills in the message box (#291): picked and typed
# activation, the options list and its headers, choices, a member search
# (op 8 with a query), running it, the app's choices asked as typed, what
# Enter refuses, the one-option rule, Backspace out of a pill and out of the
# name, and a reply or a channel switch ending it. `line` prints the box.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /ro"
sleep 1
cmd "suggestions $OUT/suggestions.png"
cmd "key Return"
cmd "line"
cmd "shot $OUT/roll.png"
cmd "type 3"
cmd "key Tab"
cmd "line"
cmd "suggestions $OUT/options.png"
cmd "type si"
cmd "suggestions"
cmd "key Tab"
cmd "line"
cmd "suggestions $OUT/choices.png"
cmd "type D2"
cmd "suggestions"
cmd "key Return"
cmd "line"
cmd "type wh"
cmd "key Tab"
cmd "type ali"
sleep 1
cmd "suggestions $OUT/members.png"
cmd "pick 0"
cmd "line"
cmd "shot $OUT/filled.png"
cmd "key Return"
waitreq POST /api/v9/interactions 1
cmd "line"
# The app's choices, asked as the pill is entered and as it is typed in, untrimmed.
cmd "type /pick "
sleep 1
cmd "line"
cmd "type ap "
sleep 2
cmd "suggestions $OUT/autocomplete.png"
cmd "line"
cmd "set "
sleep 1
cmd "line"
# Enter with the required option empty, then the caret leaving it.
cmd "type /roll "
sleep 1
cmd "key Return"
cmd "line"
cmd "shot $OUT/required.png"
cmd "key Tab"
cmd "line"
cmd "shot $OUT/errored.png"
cmd "set "
sleep 1
# The one-option rule, then the option's own check.
cmd "type /echo "
sleep 1
cmd "type h"
cmd "line"
cmd "key Return"
cmd "line"
cmd "shot $OUT/echo.png"
cmd "set "
sleep 1
# Backspace takes the empty pill, then the name's space ends the command.
cmd "type /roll "
sleep 1
cmd "key BackSpace"
cmd "line"
cmd "backspace"
cmd "line"
cmd "type  "
sleep 1
cmd "line"
# A reply keeps the line as text.
cmd "reply"
cmd "line"
cmd "shot $OUT/replied.png"
cmd "key Escape"
cmd "set "
sleep 1
# A channel switch keeps it as text where the command isn't, and names it again where it is.
cmd "type /roll count:4"
sleep 1
cmd "line"
cmd "channel 201"
sleep 1
cmd "line"
cmd "channel 600"
sleep 2
cmd "line"
cmd "shot $OUT/back.png"
cmd "quit"
sleep 1
