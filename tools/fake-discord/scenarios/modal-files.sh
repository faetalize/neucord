# An app form's file field: run with FAKE_INTERACTION=modal (and FAKE_UPLOAD_FAIL=too-large for a refused upload).
# Picture (8,0) takes images: a .txt is refused, a picture uploads as it is added, one taken back is deleted, the
# submit names the rest by index, and a success deletes them. An empty file is never uploaded and fails the submit.
python3 -c 'import base64,sys; open(sys.argv[1], "wb").write(base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=="))' $OUT/cat.png
cp $OUT/cat.png $OUT/dog.png
echo "made-up notes" > $OUT/notes.txt
: > $OUT/empty.png
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "press 1456074464952320041 open-form"
waitreq POST /api/v9/interactions 1
sleep 1
cmd "modal-files 8,0"
cmd "modal-file 8,0 $OUT/notes.txt"
sleep 1
cmd "shot $OUT/wrong-type.png"
cmd "confirm"
cmd "modal-file 8,0 $OUT/cat.png"
waitreq PUT /api/v9/upload/ 1
cmd "modal-file 8,0 $OUT/dog.png"
waitreq PUT /api/v9/upload/ 2
sleep 1
cmd "modal-files 8,0 $OUT/files.png"
cmd "modal-unfile 8,0 1"
waitreq DELETE /api/v9/attachments/ 1
cmd "modal-files 8,0"
cmd "modal-set 0,0 Ann"
cmd "modal-set 1,0 hello there"
cmd "modal-check 4,0 1"
cmd "modal-submit"
waitreq POST /api/v9/interactions 2
waitreq DELETE /api/v9/attachments/ 2
sleep 2
cmd "modal-fields"
# Again, with an empty file: refused before any request, so the submit fails at once.
cmd "press 1456074464952320041 open-form"
waitreq POST /api/v9/interactions 3
sleep 1
cmd "modal-file 8,0 $OUT/empty.png"
sleep 1
cmd "modal-files 8,0 $OUT/empty-file.png"
cmd "modal-set 0,0 Ann"
cmd "modal-set 1,0 hello there"
cmd "modal-check 4,0 1"
cmd "modal-submit"
sleep 2
cmd "modal-fields"
cmd "modal-shot $OUT/empty-failed.png"
cmd "modal-unfile 8,0 0"
cmd "modal-files 8,0"
cmd "quit"
sleep 1
