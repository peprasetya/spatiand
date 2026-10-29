// Draws SpatiKey's extension icons (key-16.png ... key-128.png, beside this file): a white key on
// a blue rounded square. Kept as code rather than only as pictures so they can be made again at
// any size, with nothing installed: node itself writes the PNGs.
//
//   node extensions/spatikey/icons/make-icons.js
'use strict';
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const BLUE = [37, 99, 235];
const WHITE = [255, 255, 255];

// In a unit square: is (x, y) inside the background, and inside the key?
function inRoundedSquare(x, y) {
  const r = 0.22;
  const cx = Math.min(Math.max(x, r), 1 - r), cy = Math.min(Math.max(y, r), 1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}

function inKey(x, y) {
  // The bow: a ring on the left.
  const bx = 0.33, by = 0.5, d = Math.hypot(x - bx, y - by);
  if (d <= 0.17 && d >= 0.075) return true;
  // The shaft, and two teeth hanging from its end.
  if (x >= 0.47 && x <= 0.84 && y >= 0.455 && y <= 0.545) return true;
  if (x >= 0.74 && x <= 0.84 && y >= 0.5 && y <= 0.66) return true;
  if (x >= 0.6 && x <= 0.68 && y >= 0.5 && y <= 0.62) return true;
  return false;
}

function pixels(size) {
  const out = Buffer.alloc(size * size * 4);
  const n = 4;
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let bg = 0, key = 0;
      for (let sy = 0; sy < n; sy++) {
        for (let sx = 0; sx < n; sx++) {
          const x = (px + (sx + 0.5) / n) / size, y = (py + (sy + 0.5) / n) / size;
          if (inRoundedSquare(x, y)) {
            bg++;
            if (inKey(x, y)) key++;
          }
        }
      }
      const i = (py * size + px) * 4, a = bg / (n * n), k = bg ? key / bg : 0;
      for (let c = 0; c < 3; c++) out[i + c] = Math.round(BLUE[c] * (1 - k) + WHITE[c] * k);
      out[i + 3] = Math.round(a * 255);
    }
  }
  return out;
}

const CRC = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});

function crc32(bytes) {
  let c = -1;
  for (const b of bytes) c = CRC[(c ^ b) & 255] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}

function png(size) {
  const rgba = pixels(size), rows = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    rows[y * (size * 4 + 1)] = 0;
    rgba.copy(rows, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;  // bits per channel
  ihdr[9] = 6;  // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(rows, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

for (const size of [16, 32, 48, 128]) fs.writeFileSync(path.join(__dirname, `key-${size}.png`), png(size));
