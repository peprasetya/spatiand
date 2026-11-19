#!/bin/sh
# How the encrypted PDFs SpatiLoupe's tests open, mark up and save were made: by qpdf, from a plain
# two-page PDF (the second page /Rotate 90) written here. Run from this folder; needs qpdf.
#   rc4-128.pdf           RC4 128 (R3), an owner's password only: opens without asking, all allowed
#   rc4-128-user.pdf      RC4 128, opened with "user"
#   aes-128-noannot.pdf   AES 128 (R4), an owner's password only, notes not allowed
#   aes-128-user.pdf      AES 128, opened with "user"
#   aes-256-nomodify.pdf  AES 256 (R6), an owner's password only, notes allowed, pages not changed
#   aes-256-user.pdf      AES 256, opened with "pässwörd"
# The owner's password of each is "owner".
set -eu
python3 - plain.pdf <<'EOF'
import sys
objects = ["<< /Type /Catalog /Pages 2 0 R >>", "", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
kids = []
for words, rotate in (("Locked first page", 0), ("Locked second page", 90)):
    content = f"BT /F1 24 Tf 60 700 Td ({words}) Tj ET"
    objects.append(f"<< /Length {len(content)} >>\nstream\n{content}\nendstream")
    contents = len(objects)
    objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Rotate {rotate} /Contents {contents} 0 R /Resources << /Font << /F1 3 0 R >> >> >>")
    kids.append(f"{len(objects)} 0 R")
objects[1] = f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {len(kids)} >>"
out = bytearray(b"%PDF-1.4\n")
at = []
for i, o in enumerate(objects):
    at.append(len(out))
    out += f"{i + 1} 0 obj\n{o}\nendobj\n".encode()
xref = len(out)
out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
for offset in at:
    out += f"{offset:010} 00000 n \n".encode()
out += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
open(sys.argv[1], "wb").write(out)
EOF
q() { qpdf --allow-weak-crypto --static-id "$@"; }
q --encrypt --user-password= --owner-password=owner --bits=128 --use-aes=n -- plain.pdf rc4-128.pdf
q --encrypt --user-password=user --owner-password=owner --bits=128 --use-aes=n -- plain.pdf rc4-128-user.pdf
q --encrypt --user-password= --owner-password=owner --bits=128 --use-aes=y --annotate=n -- plain.pdf aes-128-noannot.pdf
q --encrypt --user-password=user --owner-password=owner --bits=128 --use-aes=y -- plain.pdf aes-128-user.pdf
q --encrypt --user-password= --owner-password=owner --bits=256 --modify-other=n -- plain.pdf aes-256-nomodify.pdf
q --encrypt --user-password='pässwörd' --owner-password=owner --bits=256 -- plain.pdf aes-256-user.pdf
rm plain.pdf
