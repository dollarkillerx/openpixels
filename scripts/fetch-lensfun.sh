#!/usr/bin/env bash
# Downloads the Lensfun lens database (CC BY-SA 3.0) at a fixed commit and converts
# it into web/public/lensfun. Only the data is used, not Lensfun's LGPL code.
set -euo pipefail
cd "$(dirname "$0")/.."

COMMIT=52cf1a7b0e82e07e0ecaa3391ee5106a6dd1ee54
DIR=.tools/lensfun
mkdir -p "$DIR"

curl -sS --fail -A 'openpixels-lensfun-fetch' "https://api.github.com/repos/lensfun/lensfun/contents/data/db?ref=$COMMIT" |
  node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{for(const f of JSON.parse(s))if(f.name.endsWith(".xml"))console.log(f.name)})' \
  >"$DIR/names.txt"

while read -r name; do
  [ -f "$DIR/$name" ] && continue
  curl -sS --fail --retry 5 -A 'openpixels-lensfun-fetch' -o "$DIR/$name" \
    "https://raw.githubusercontent.com/lensfun/lensfun/$COMMIT/data/db/$name"
done <"$DIR/names.txt"

node scripts/build-lensfun.mjs "$DIR" web/public/lensfun "$COMMIT"
